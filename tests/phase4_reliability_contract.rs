//! Focused executable contracts for phase-four request reliability.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt as _;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::provider::TestOnlyProfile;
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{
    AssistantEvent, ErrorStage, GenerateRequest, GenerationOptions, IdempotencyKey,
    IdempotencyPolicy, LifecycleEvent, LifecycleEventKind, LifecycleObserver, LlmClient, LlmError,
    Message, ModelRef, RateLimitHeaderKind, RateLimitHeaderSpec, RateLimitPolicy, RateLimitUnit,
    RateLimitValue, RequestControl, RetriableHint, RetryPolicy, RetryWaitPolicy, TimeoutPolicy,
    TimeoutStage, TransportError,
};

const ENDPOINT: &str = "http://127.0.0.1:41992/v1/chat/completions";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn runtime() -> philo::ProviderRuntime {
    TestOnlyProfile::localhost(ENDPOINT, "phase-four-key")
        .unwrap()
        .build()
        .unwrap()
}

fn request() -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        vec![Message::user("hello")],
    )
}

fn headers(request_id: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert("x-request-id", HeaderValue::from_str(request_id).unwrap());
    headers
}

fn success_body(generation_id: &str, text: &str) -> Bytes {
    Bytes::from(format!(
        concat!(
            "data: {{\"id\":\"{}\",\"model\":\"gpt-test\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":\"{}\"}},\"finish_reason\":null}}]}}\n\n",
            "data: {{\"id\":\"{}\",\"model\":\"gpt-test\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n",
            "data: [DONE]\n\n"
        ),
        generation_id, text, generation_id
    ))
}

fn success(request_id: &str, generation_id: &str, text: &str) -> MockExchange {
    MockExchange::response(MockResponse::new(
        StatusCode::OK,
        headers(request_id),
        vec![MockBodyItem::chunk(success_body(generation_id, text))],
    ))
}

fn retryable_with_retry_after(seconds: u64) -> MockExchange {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&seconds.to_string()).unwrap(),
    );
    MockExchange::response(MockResponse::new(
        StatusCode::SERVICE_UNAVAILABLE,
        headers,
        vec![MockBodyItem::chunk(Bytes::from_static(b"temporary"))],
    ))
}

#[derive(Clone, Default)]
struct RecordingObserver(Arc<Mutex<Vec<LifecycleEvent>>>);

impl LifecycleObserver for RecordingObserver {
    fn record(&self, event: &LifecycleEvent) {
        lock(&self.0).push(event.clone());
    }
}

impl RecordingObserver {
    fn events(&self) -> Vec<LifecycleEvent> {
        lock(&self.0).clone()
    }
}

struct PanickingObserver;

impl LifecycleObserver for PanickingObserver {
    fn record(&self, _event: &LifecycleEvent) {
        panic!("observer failure must remain isolated");
    }
}

#[tokio::test]
async fn panicking_observer_never_changes_the_request_result() {
    let mock = MockTransport::scripted([success("provider-safe", "generation-safe", "ok")]);
    let message = LlmClient::new(runtime(), mock.clone())
        .with_observer(PanickingObserver)
        .complete(request())
        .await
        .unwrap();

    assert_eq!(message.text(), "ok");
    assert_eq!(mock.captured_requests().len(), 1);
}

#[tokio::test]
async fn connect_failure_then_success_uses_one_logical_request_and_fresh_attempts() {
    let observer = RecordingObserver::default();
    let mock = MockTransport::scripted([
        MockExchange::start_error(
            TransportError::new(ErrorStage::Connect, RetriableHint::Maybe).into(),
        ),
        success("provider-2", "generation-2", "recovered"),
    ]);
    let client = LlmClient::new(runtime(), mock.clone())
        .with_retry_policy(RetryPolicy::standard())
        .with_observer(observer.clone());

    let message = client.complete(request()).await.unwrap();
    assert_eq!(message.text(), "recovered");

    let captured = mock.captured_requests();
    assert_eq!(captured.len(), 2);
    assert_eq!(
        captured[0].local_request_id(),
        captured[1].local_request_id()
    );

    let events = observer.events();
    let attempts = events
        .iter()
        .filter_map(|event| match event.kind() {
            LifecycleEventKind::AttemptStarted { attempt } => Some(attempt.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].number(), 1);
    assert_eq!(attempts[1].number(), 2);
    assert_ne!(attempts[0].id(), attempts[1].id());
    assert!(events.iter().any(|event| matches!(
        event.kind(),
        LifecycleEventKind::RetryDecided {
            reason: Some(_),
            stop_reason: None,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event.kind(),
        LifecycleEventKind::RetryScheduled {
            next_attempt_number: 2,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event.kind(),
        LifecycleEventKind::CredentialResolved { attempt } if attempt.number() == 2
    )));
}

#[tokio::test]
async fn default_lifecycle_events_do_not_copy_request_or_secret_content() {
    const CONTENT_CANARY: &str = "PHILO_CONTENT_CANARY_4f4864";
    const SECRET_CANARY: &str = "phase-four-key";
    let observer = RecordingObserver::default();
    let mock = MockTransport::scripted([success("provider-safe", "generation-safe", "ok")]);
    let canary_request = GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        vec![Message::user(CONTENT_CANARY)],
    );

    LlmClient::new(runtime(), mock)
        .with_observer(observer.clone())
        .complete(canary_request)
        .await
        .unwrap();

    let rendered = format!("{:?}", observer.events());
    assert!(!rendered.contains(CONTENT_CANARY));
    assert!(!rendered.contains(SECRET_CANARY));
    assert!(
        observer
            .events()
            .iter()
            .any(|event| matches!(event.kind(), LifecycleEventKind::FirstDomainEventDelivered))
    );
}

#[tokio::test]
async fn external_finish_labels_and_oversized_headers_fail_value_free_and_bounded() {
    const FINISH_CANARY: &str = "PHILO_FINISH_CANARY_858af1";
    let unknown_finish = Bytes::from(format!(
        concat!(
            "data: {{\"id\":\"generation-bad\",\"model\":\"gpt-test\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"{}\"}}]}}\n\n",
            "data: [DONE]\n\n"
        ),
        FINISH_CANARY
    ));
    let unknown_mock = MockTransport::scripted([MockExchange::response(MockResponse::new(
        StatusCode::OK,
        headers("provider-bad"),
        vec![MockBodyItem::chunk(unknown_finish)],
    ))]);
    let error = LlmClient::new(runtime(), unknown_mock)
        .complete(request())
        .await
        .unwrap_err();
    assert!(matches!(error, LlmError::UnknownFinishReason(_)));
    assert!(!format!("{error}").contains(FINISH_CANARY));
    assert!(!format!("{error:?}").contains(FINISH_CANARY));

    let mut oversized = headers("provider-oversized");
    oversized.insert(
        "x-oversized",
        HeaderValue::from_bytes(&vec![b'x'; 16 * 1024 + 1]).unwrap(),
    );
    let oversized_mock = MockTransport::scripted([MockExchange::response(MockResponse::new(
        StatusCode::OK,
        oversized,
        vec![MockBodyItem::chunk(success_body(
            "generation-unused",
            "unused",
        ))],
    ))]);
    let error = LlmClient::new(runtime(), oversized_mock)
        .complete(request())
        .await
        .unwrap_err();
    assert!(matches!(error, LlmError::Protocol(_)));
    assert_eq!(
        format!("{error}"),
        "protocol error at Protocol: response header resource limit exceeded"
    );
}

#[tokio::test]
async fn retryable_status_reuses_the_exact_overall_deadline() {
    let mock = MockTransport::scripted([
        MockExchange::response(MockResponse::new(
            StatusCode::SERVICE_UNAVAILABLE,
            HeaderMap::new(),
            vec![MockBodyItem::chunk(Bytes::from_static(b"temporary"))],
        )),
        success("provider-ok", "generation-ok", "ok"),
    ]);
    let client = LlmClient::new(runtime(), mock.clone()).with_retry_policy(RetryPolicy::standard());
    let request = request().with_options(
        GenerationOptions::new()
            .with_timeout(Duration::from_secs(2))
            .unwrap(),
    );

    client.complete(request).await.unwrap();
    let captured = mock.captured_requests();
    assert_eq!(captured.len(), 2);
    assert!(captured[0].deadline().is_some());
    assert_eq!(captured[0].deadline(), captured[1].deadline());
}

#[tokio::test]
async fn first_event_timeout_can_retry_without_leaking_the_failed_attempt() {
    let mock = MockTransport::scripted([
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            headers("slow-provider"),
            vec![MockBodyItem::delayed_chunk(
                Duration::from_millis(100),
                success_body("slow-generation", "must-not-leak"),
            )],
        )),
        success("fast-provider", "fast-generation", "visible"),
    ]);
    let timeouts = TimeoutPolicy::new()
        .with_first_event_timeout(Duration::from_millis(10))
        .unwrap();
    let client = LlmClient::new(runtime(), mock.clone())
        .with_timeout_policy(timeouts)
        .with_retry_policy(RetryPolicy::standard());

    let message = client.complete(request()).await.unwrap();
    assert_eq!(message.text(), "visible");
    assert_eq!(mock.captured_requests().len(), 2);
    assert_eq!(mock.early_body_drop_count(), 1);
}

#[tokio::test]
async fn any_delivered_event_permanently_closes_the_retry_boundary() {
    let start = Bytes::from_static(
        b"data: {\"id\":\"generation-start\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
    );
    let mock = MockTransport::scripted([
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            headers("provider-start"),
            vec![
                MockBodyItem::chunk(start),
                MockBodyItem::error(
                    TransportError::new(ErrorStage::Body, RetriableHint::Maybe).into(),
                ),
            ],
        )),
        success("must-not-run", "must-not-run", "duplicate"),
    ]);
    let client = LlmClient::new(runtime(), mock.clone()).with_retry_policy(RetryPolicy::standard());
    let mut stream = client.stream(request()).await.unwrap();

    assert!(matches!(
        stream.next().await,
        Some(Ok(AssistantEvent::Start { .. }))
    ));
    assert!(matches!(
        stream.next().await,
        Some(Err(LlmError::Transport(_)))
    ));
    assert!(stream.next().await.is_none());
    assert_eq!(mock.captured_requests().len(), 1);
    assert_eq!(mock.remaining_expectations(), 1);
}

#[tokio::test]
async fn idle_timeout_after_delivery_is_precise_and_never_retried() {
    let start = Bytes::from_static(
        b"data: {\"id\":\"generation-idle\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
    );
    let mock = MockTransport::scripted([
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            headers("provider-idle"),
            vec![
                MockBodyItem::chunk(start),
                MockBodyItem::delayed_chunk(
                    Duration::from_millis(100),
                    success_body("generation-idle", "late"),
                ),
            ],
        )),
        success("must-not-run", "must-not-run", "duplicate"),
    ]);
    let timeouts = TimeoutPolicy::new()
        .with_idle_stream_timeout(Duration::from_millis(10))
        .unwrap();
    let client = LlmClient::new(runtime(), mock.clone())
        .with_timeout_policy(timeouts)
        .with_retry_policy(RetryPolicy::standard());
    let mut stream = client.stream(request()).await.unwrap();

    assert!(matches!(
        stream.next().await,
        Some(Ok(AssistantEvent::Start { .. }))
    ));
    let Some(Err(LlmError::Timeout(error))) = stream.next().await else {
        panic!("expected idle stream timeout");
    };
    assert_eq!(error.timeout_stage(), TimeoutStage::IdleStream);
    assert!(error.domain_event_delivered());
    assert_eq!(mock.captured_requests().len(), 1);
    assert_eq!(mock.remaining_expectations(), 1);
}

#[test]
fn public_reliability_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<philo::AttemptId>();
    assert_send_sync::<philo::AttemptIdentity>();
    assert_send_sync::<RetryPolicy>();
    assert_send_sync::<RetryWaitPolicy>();
    assert_send_sync::<TimeoutPolicy>();
    assert_send_sync::<philo::RateLimitObservation>();
    assert_send_sync::<philo::IdempotencyKey>();
    assert_send_sync::<philo::NetworkPolicy>();
}

#[tokio::test(start_paused = true)]
async fn retry_after_waits_without_extending_the_overall_deadline() {
    let observer = RecordingObserver::default();
    let mock = MockTransport::scripted([
        retryable_with_retry_after(10),
        success("provider-after-wait", "generation-after-wait", "ok"),
    ]);
    let client = LlmClient::new(runtime(), mock.clone())
        .with_retry_policy(RetryPolicy::standard())
        .with_observer(observer.clone());
    let task = tokio::spawn(async move { client.complete(request()).await });

    while !observer.events().iter().any(|event| {
        matches!(
            event.kind(),
            LifecycleEventKind::RetryScheduled {
                next_attempt_number: 2,
                ..
            }
        )
    }) {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(9)).await;
    tokio::task::yield_now().await;
    assert_eq!(mock.captured_requests().len(), 1);

    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(task.await.unwrap().unwrap().text(), "ok");
    assert_eq!(mock.captured_requests().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn cancel_during_retry_wait_never_starts_the_next_attempt() {
    let observer = RecordingObserver::default();
    let mock = MockTransport::scripted([
        retryable_with_retry_after(30),
        success("must-not-run", "must-not-run", "duplicate"),
    ]);
    let client = LlmClient::new(runtime(), mock.clone())
        .with_retry_policy(RetryPolicy::standard())
        .with_observer(observer.clone());
    let control = RequestControl::new();
    let task_control = control.clone();
    let task =
        tokio::spawn(async move { client.complete_with_control(request(), task_control).await });

    while !observer.events().iter().any(|event| {
        matches!(
            event.kind(),
            LifecycleEventKind::RetryScheduled {
                next_attempt_number: 2,
                ..
            }
        )
    }) {
        tokio::task::yield_now().await;
    }
    control.cancel();
    assert!(matches!(task.await.unwrap(), Err(LlmError::Cancelled)));
    assert_eq!(mock.captured_requests().len(), 1);
    assert_eq!(mock.remaining_expectations(), 1);
}

#[tokio::test(start_paused = true)]
async fn retry_wait_that_cannot_fit_budget_does_not_start_a_doomed_attempt() {
    let mock = MockTransport::scripted([
        retryable_with_retry_after(1),
        success("must-not-run", "must-not-run", "duplicate"),
    ]);
    let client = LlmClient::new(runtime(), mock.clone()).with_retry_policy(RetryPolicy::standard());
    let request = request().with_options(
        GenerationOptions::new()
            .with_timeout(Duration::from_millis(500))
            .unwrap(),
    );

    assert!(matches!(
        client.complete(request).await,
        Err(LlmError::HttpStatus(_))
    ));
    assert_eq!(mock.captured_requests().len(), 1);
    assert_eq!(mock.remaining_expectations(), 1);
}

#[tokio::test]
async fn unpolled_public_stream_does_not_read_response_body() {
    let mock = MockTransport::scripted([success(
        "provider-unpolled",
        "generation-unpolled",
        "not-read",
    )]);
    let client = LlmClient::new(runtime(), mock.clone());

    let stream = client.stream(request()).await.unwrap();
    assert_eq!(mock.body_poll_count(), 0);
    drop(stream);
    assert_eq!(mock.body_poll_count(), 0);
    assert_eq!(mock.early_body_drop_count(), 1);
}

#[tokio::test]
async fn rate_limit_headers_are_typed_without_retaining_raw_values() {
    let policy = RateLimitPolicy::standard_only()
        .with_header(RateLimitHeaderSpec::new(
            http::HeaderName::from_static("x-quota-requests"),
            RateLimitHeaderKind::RemainingRequests,
        ))
        .unwrap()
        .with_header(RateLimitHeaderSpec::new(
            http::HeaderName::from_static("x-quota-tokens"),
            RateLimitHeaderKind::RemainingUnits(RateLimitUnit::Tokens),
        ))
        .unwrap()
        .with_header(RateLimitHeaderSpec::new(
            http::HeaderName::from_static("x-quota-reset"),
            RateLimitHeaderKind::ResetAfterSeconds,
        ))
        .unwrap();
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "phase-four-key")
        .unwrap()
        .with_rate_limit_policy(policy)
        .build()
        .unwrap();
    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::RETRY_AFTER, HeaderValue::from_static("7"));
    response_headers.insert("x-quota-requests", HeaderValue::from_static("12"));
    response_headers.insert("x-quota-tokens", HeaderValue::from_static("345"));
    response_headers.insert(
        "x-quota-reset",
        HeaderValue::from_static("secret-header-canary"),
    );
    let mock = MockTransport::scripted([MockExchange::response(MockResponse::new(
        StatusCode::TOO_MANY_REQUESTS,
        response_headers,
        vec![MockBodyItem::chunk(Bytes::from_static(b"rate limited"))],
    ))]);
    let error = LlmClient::new(runtime, mock)
        .complete(request())
        .await
        .unwrap_err();
    let LlmError::HttpStatus(error) = error else {
        panic!("expected typed HTTP status error");
    };
    let observation = error.rate_limit().unwrap();
    assert!(observation.status_is_rate_limited());
    assert_eq!(
        observation.retry_after(),
        RateLimitValue::Valid(Duration::from_secs(7))
    );
    assert_eq!(observation.remaining_requests(), RateLimitValue::Valid(12));
    let RateLimitValue::Valid(quota) = observation.remaining_units() else {
        panic!("expected typed remaining token quota");
    };
    assert_eq!(quota.remaining(), 345);
    assert_eq!(quota.unit(), RateLimitUnit::Tokens);
    let debug = format!("{error:?}");
    assert!(!debug.contains("secret-header-canary"));
}

#[tokio::test(start_paused = true)]
async fn each_attempt_parses_fresh_rate_limit_headers() {
    let observer = RecordingObserver::default();
    let mut first_headers = HeaderMap::new();
    first_headers.insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    let mock = MockTransport::scripted([
        MockExchange::response(MockResponse::new(
            StatusCode::TOO_MANY_REQUESTS,
            first_headers,
            vec![MockBodyItem::chunk(Bytes::from_static(b"first"))],
        )),
        MockExchange::response(MockResponse::new(
            StatusCode::TOO_MANY_REQUESTS,
            HeaderMap::new(),
            vec![MockBodyItem::chunk(Bytes::from_static(b"second"))],
        )),
    ]);
    let retries = RetryPolicy::standard().with_max_attempts(2).unwrap();
    let result = LlmClient::new(runtime(), mock)
        .with_retry_policy(retries)
        .with_observer(observer.clone())
        .complete(request())
        .await;
    assert!(matches!(result, Err(LlmError::HttpStatus(_))));
    let retry_after_valid = observer
        .events()
        .into_iter()
        .filter_map(|event| match event.kind() {
            LifecycleEventKind::RateLimitObserved {
                retry_after_valid, ..
            } => Some(*retry_after_valid),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(retry_after_valid, vec![true, false]);
}

#[tokio::test]
async fn same_provider_retry_reencodes_the_same_opaque_idempotency_key() {
    let mock = MockTransport::scripted([
        MockExchange::response(MockResponse::new(
            StatusCode::SERVICE_UNAVAILABLE,
            HeaderMap::new(),
            vec![MockBodyItem::chunk(Bytes::from_static(b"temporary"))],
        )),
        success("provider-idempotent", "generation-idempotent", "ok"),
    ]);
    let key = IdempotencyKey::new("caller-key-007").unwrap();
    let client = LlmClient::new(runtime(), mock.clone()).with_retry_policy(RetryPolicy::standard());
    let message = client
        .complete_with_control(
            request(),
            RequestControl::new().with_idempotency_key(key.clone()),
        )
        .await
        .unwrap();
    assert_eq!(message.text(), "ok");
    let captured = mock.captured_requests();
    assert_eq!(captured.len(), 2);
    let first = captured[0].headers().get("idempotency-key").unwrap();
    let second = captured[1].headers().get("idempotency-key").unwrap();
    assert_eq!(first, second);
    assert_eq!(format!("{key:?}"), "IdempotencyKey([REDACTED])");
    assert!(!format!("{:?}", captured[0]).contains("caller-key-007"));
}

#[tokio::test]
async fn unknown_idempotency_capability_fails_closed_before_transport() {
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "phase-four-key")
        .unwrap()
        .with_idempotency_policy(IdempotencyPolicy::unknown())
        .build()
        .unwrap();
    let mock = MockTransport::scripted([success("must-not-run", "must-not-run", "duplicate")]);
    let result = LlmClient::new(runtime, mock.clone())
        .complete_with_control(
            request(),
            RequestControl::new().with_generated_idempotency_key(),
        )
        .await;
    assert!(matches!(result, Err(LlmError::Validation(_))));
    assert!(mock.captured_requests().is_empty());
}

#[test]
fn sdk_generated_idempotency_keys_are_unique_valid_and_redacted() {
    let first = IdempotencyKey::generate();
    let second = IdempotencyKey::generate();
    assert_ne!(first, second);
    assert!(!format!("{first:?}").contains("philo_"));
    assert!(IdempotencyKey::new("bad key with spaces").is_err());
    assert!(IdempotencyKey::new("x".repeat(129)).is_err());
}

#[tokio::test]
async fn ordinary_request_header_cannot_override_protected_idempotency_header() {
    let options = GenerationOptions::new().with_header(
        http::HeaderName::from_static("idempotency-key"),
        HeaderValue::from_static("forged-key"),
    );
    let request = request().with_options(options);
    let mock = MockTransport::scripted([success("must-not-run", "must-not-run", "duplicate")]);
    let result = LlmClient::new(runtime(), mock.clone())
        .complete_with_control(
            request,
            RequestControl::new().with_generated_idempotency_key(),
        )
        .await;
    assert!(matches!(result, Err(LlmError::Validation(_))));
    assert!(mock.captured_requests().is_empty());
}
