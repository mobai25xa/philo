//! End-to-end client orchestration, lifecycle, and observability contracts.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt as _;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::provider::TestOnlyProfile;
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{
    AssistantEvent, FinishReason, GenerateRequest, GenerationOptions, LifecycleErrorCategory,
    LifecycleEvent, LifecycleEventKind, LifecycleObserver, LlmClient, LlmError, Message, ModelRef,
    ProviderRequestId, RequestControl, TraceId, Usage,
};
use serde_json::Value;
use tokio::time::sleep;

const API_KEY: &str = "philo-client-api-key-canary";
const PROMPT_CANARY: &str = "philo-client-prompt-canary";
const OUTPUT_CANARY: &str = "philo-client-output-canary";
const ENDPOINT: &str = "http://127.0.0.1:41991/v1/chat/completions";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn runtime() -> philo::ProviderRuntime {
    TestOnlyProfile::localhost(ENDPOINT, API_KEY)
        .unwrap()
        .build()
        .unwrap()
}

fn request() -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        vec![Message::user(PROMPT_CANARY)],
    )
}

fn response_headers(request_id: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    headers.insert("x-request-id", HeaderValue::from_str(request_id).unwrap());
    headers
}

fn success_sse(generation_id: &str, output: &str) -> Bytes {
    Bytes::from(format!(
        concat!(
            "data: {{\"id\":\"{}\",\"model\":\"gpt-test\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":\"{}\"}},\"finish_reason\":null}}]}}\n\n",
            "data: {{\"id\":\"{}\",\"model\":\"gpt-test\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n",
            "data: {{\"id\":\"{}\",\"model\":\"gpt-test\",\"choices\":[],\"usage\":{{\"prompt_tokens\":2,\"completion_tokens\":1,\"total_tokens\":3}}}}\n\n",
            "data: [DONE]\n\n"
        ),
        generation_id, output, generation_id, generation_id
    ))
}

fn success_exchange(request_id: &str, generation_id: &str, output: &str) -> MockExchange {
    MockExchange::response(MockResponse::new(
        StatusCode::OK,
        response_headers(request_id),
        vec![MockBodyItem::chunk(success_sse(generation_id, output))],
    ))
}

#[tokio::test]
async fn stream_and_complete_form_one_vertical_pipeline() {
    let mock = MockTransport::scripted([
        success_exchange("req-stream", "gen-stream", "hello"),
        success_exchange("req-complete", "gen-complete", "world"),
    ]);
    let client = LlmClient::new(runtime(), mock.clone());

    let events = client
        .stream(request())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(matches!(
        &events[0],
        AssistantEvent::Start {
            provider_request_id: Some(provider),
            generation_id: Some(generation),
            ..
        } if provider.as_str() == "req-stream" && generation.as_str() == "gen-stream"
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        AssistantEvent::TextDelta { delta, .. } if delta == "hello"
    )));
    assert_eq!(
        events[events.len() - 2],
        AssistantEvent::Usage(Usage::new(2, 1, 3).unwrap())
    );
    assert_eq!(
        events.last(),
        Some(&AssistantEvent::Done {
            finish_reason: FinishReason::Stop
        })
    );

    let message = client.complete(request()).await.unwrap();
    assert_eq!(message.text(), "world");
    assert_eq!(message.usage(), Some(Usage::new(2, 1, 3).unwrap()));
    assert_eq!(
        message.provider_request_id().unwrap().as_str(),
        "req-complete"
    );
    assert_eq!(message.generation_id().unwrap().as_str(), "gen-complete");
    assert_eq!(message.model().unwrap(), request().model());

    mock.assert_consumed();
    let captured = mock.captured_requests();
    assert_eq!(captured.len(), 2, "complete must execute transport once");
    assert_ne!(
        captured[0].local_request_id(),
        captured[1].local_request_id()
    );
    for captured in captured {
        assert_eq!(captured.method(), http::Method::POST);
        assert_eq!(captured.endpoint().url().as_str(), ENDPOINT);
        assert_eq!(captured.headers()[header::ACCEPT], "text/event-stream");
        assert_eq!(captured.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(
            captured.headers()[header::AUTHORIZATION],
            format!("Bearer {API_KEY}")
        );
        let body: Value = serde_json::from_slice(captured.body()).unwrap();
        assert_eq!(body["messages"][0]["content"], PROMPT_CANARY);
        assert_eq!(body["stream"], true);
    }
}

#[tokio::test]
async fn validation_and_http_errors_fail_before_sse_with_typed_ids() {
    let mut error_headers = HeaderMap::new();
    error_headers.insert("x-request-id", HeaderValue::from_static("req-http-error"));
    let mut wrong_media_headers = HeaderMap::new();
    wrong_media_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let mock = MockTransport::scripted([
        MockExchange::response(MockResponse::new(
            StatusCode::TOO_MANY_REQUESTS,
            error_headers,
            vec![MockBodyItem::chunk(format!(
                "api_key={API_KEY}{}",
                "x".repeat(20 * 1024)
            ))],
        )),
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            wrong_media_headers,
            vec![MockBodyItem::chunk(success_sse("unused", "unused"))],
        )),
    ]);
    let client = LlmClient::new(runtime(), mock.clone());

    let wrong_provider = GenerateRequest::new(
        ModelRef::new("other-provider", "gpt-test").unwrap(),
        vec![Message::user("valid")],
    );
    assert!(matches!(
        client.stream(wrong_provider).await,
        Err(LlmError::Validation(_))
    ));
    assert!(mock.captured_requests().is_empty());

    let error = client.stream(request()).await.unwrap_err();
    assert!(matches!(
        error,
        LlmError::HttpStatus(ref error)
            if error.status() == 429
                && error.request_id() == Some(&ProviderRequestId::new("req-http-error").unwrap())
                && error.body().as_str() == "<redacted error body>... [truncated]"
    ));
    assert!(!format!("{error:?}").contains(API_KEY));
    assert!(matches!(
        client.stream(request()).await,
        Err(LlmError::Protocol(_))
    ));
    assert_eq!(mock.captured_requests().len(), 2);
    mock.assert_consumed();
}

#[tokio::test]
async fn cancellation_covers_preflight_headers_first_event_and_partial_text() {
    let partial = Bytes::from_static(
        b"data: {\"id\":\"gen-partial\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n",
    );
    let preflight_mock = MockTransport::scripted([success_exchange("unused", "unused", "unused")]);
    let preflight_client = LlmClient::new(runtime(), preflight_mock.clone());
    let preflight = RequestControl::new();
    preflight.cancel();
    assert!(matches!(
        preflight_client
            .stream_with_control(request(), preflight)
            .await,
        Err(LlmError::Cancelled)
    ));
    assert_eq!(preflight_mock.remaining_expectations(), 1);
    assert!(preflight_mock.captured_requests().is_empty());

    let mock = MockTransport::scripted([
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            response_headers("req-headers"),
            vec![MockBodyItem::chunk(success_sse("gen-headers", "late"))],
        ))
        .with_start_delay(Duration::from_secs(1)),
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            response_headers("req-first"),
            vec![MockBodyItem::delayed_chunk(
                Duration::from_secs(1),
                success_sse("gen-first", "late"),
            )],
        )),
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            response_headers("req-partial"),
            vec![
                MockBodyItem::chunk(partial),
                MockBodyItem::delayed_chunk(
                    Duration::from_secs(1),
                    Bytes::from_static(
                        b"data: {\"id\":\"gen-partial\",\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                    ),
                ),
            ],
        )),
    ]);
    let client = LlmClient::new(runtime(), mock.clone());

    let headers_control = RequestControl::new();
    let task_client = client.clone();
    let task_control = headers_control.clone();
    let waiting_headers = tokio::spawn(async move {
        task_client
            .stream_with_control(request(), task_control)
            .await
    });
    sleep(Duration::from_millis(20)).await;
    headers_control.cancel();
    assert!(matches!(
        waiting_headers.await.unwrap(),
        Err(LlmError::Cancelled)
    ));

    let first_control = RequestControl::new();
    let mut first = client
        .stream_with_control(request(), first_control.clone())
        .await
        .unwrap();
    first_control.cancel();
    assert!(matches!(first.next().await, Some(Err(LlmError::Cancelled))));

    let partial_control = RequestControl::new();
    let mut partial_stream = client
        .stream_with_control(request(), partial_control.clone())
        .await
        .unwrap();
    assert!(matches!(
        partial_stream.next().await,
        Some(Ok(AssistantEvent::Start { .. }))
    ));
    assert!(matches!(
        partial_stream.next().await,
        Some(Ok(AssistantEvent::TextStart { .. }))
    ));
    assert!(matches!(
        partial_stream.next().await,
        Some(Ok(AssistantEvent::TextDelta { ref delta, .. })) if delta == "partial"
    ));
    partial_control.cancel();
    assert!(matches!(
        partial_stream.next().await,
        Some(Err(LlmError::Cancelled))
    ));
    assert!(partial_stream.next().await.is_none());
    assert_eq!(mock.body_cancellation_count(), 2);
    mock.assert_consumed();
}

#[tokio::test]
async fn overall_deadline_and_drop_reach_the_transport_body() {
    let mock = MockTransport::scripted([
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            response_headers("req-timeout-headers"),
            vec![MockBodyItem::chunk(success_sse("gen", "late"))],
        ))
        .with_start_delay(Duration::from_secs(1)),
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            response_headers("req-timeout-body"),
            vec![MockBodyItem::delayed_chunk(
                Duration::from_secs(1),
                success_sse("gen", "late"),
            )],
        )),
        MockExchange::response(MockResponse::new(
            StatusCode::OK,
            response_headers("req-drop"),
            vec![MockBodyItem::delayed_chunk(
                Duration::from_secs(1),
                success_sse("gen", "late"),
            )],
        )),
    ]);
    let client = LlmClient::new(runtime(), mock.clone());
    let timed = request().with_options(
        GenerationOptions::new()
            .with_timeout(Duration::from_millis(20))
            .unwrap(),
    );
    assert!(matches!(
        client.stream(timed.clone()).await,
        Err(LlmError::Timeout(_))
    ));

    let mut body_timeout = client.stream(timed).await.unwrap();
    assert!(matches!(
        body_timeout.next().await,
        Some(Err(LlmError::Timeout(_)))
    ));

    let dropped = client.stream(request()).await.unwrap();
    drop(dropped);
    assert_eq!(mock.early_body_drop_count(), 1);
    mock.assert_consumed();
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

fn assert_failed_trace(events: &[LifecycleEvent]) -> String {
    assert!(matches!(
        events.first().map(LifecycleEvent::kind),
        Some(LifecycleEventKind::RequestStarted)
    ));
    let local_id = events[0].identity().local_request_id().as_str();
    assert!(
        events
            .iter()
            .all(|event| event.identity().local_request_id().as_str() == local_id)
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind(), LifecycleEventKind::DoneSeen))
    );
    local_id.to_owned()
}

#[tokio::test]
async fn lifecycle_trace_is_ordered_value_free_and_uses_typed_ids() {
    let observer = RecordingObserver::default();
    let mock = MockTransport::scripted([success_exchange(
        "req-observed",
        "gen-observed",
        OUTPUT_CANARY,
    )]);
    let client = LlmClient::new(runtime(), mock).with_observer(observer.clone());
    let control = RequestControl::new().with_trace_id(TraceId::new("trace-parent").unwrap());
    let message = client
        .complete_with_control(request(), control)
        .await
        .unwrap();
    assert_eq!(message.text(), OUTPUT_CANARY);

    let events = observer.events();
    let kinds: Vec<_> = events.iter().map(LifecycleEvent::kind).collect();
    assert!(matches!(kinds[0], LifecycleEventKind::RequestStarted));
    assert!(
        kinds
            .iter()
            .any(|kind| matches!(kind, LifecycleEventKind::ValidationCompleted))
    );
    assert!(kinds
        .iter()
        .any(|kind| matches!(kind, LifecycleEventKind::HeadersResolved { trace } if trace.iter().any(|entry| entry.name() == header::AUTHORIZATION && entry.is_sensitive()))));
    assert!(kinds.iter().any(|kind| matches!(
        kind,
        LifecycleEventKind::StatusReceived {
            provider_request_id: Some(id),
            ..
        } if id.as_str() == "req-observed"
    )));
    assert!(kinds.iter().any(|kind| matches!(
        kind,
        LifecycleEventKind::FirstSseEvent {
            generation_id: Some(id),
            ..
        } if id.as_str() == "gen-observed"
    )));
    assert!(matches!(
        kinds.last(),
        Some(LifecycleEventKind::RequestCompleted {
            usage_known: true,
            ..
        })
    ));
    let local_ids: BTreeSet<_> = events
        .iter()
        .map(|event| event.identity().local_request_id().as_str())
        .collect();
    assert_eq!(local_ids.len(), 1);
    assert!(
        events.iter().all(|event| {
            event.identity().trace_id().map(TraceId::as_str) == Some("trace-parent")
        })
    );

    let diagnostics = format!("{events:?}\n{client:?}");
    for canary in [API_KEY, PROMPT_CANARY, OUTPUT_CANARY] {
        assert!(!diagnostics.contains(canary));
    }
}

#[tokio::test]
async fn lifecycle_terminal_paths_keep_distinct_local_ids_and_never_forge_done() {
    let validation_observer = RecordingObserver::default();
    let validation_client = LlmClient::new(runtime(), MockTransport::default())
        .with_observer(validation_observer.clone());
    let invalid = GenerateRequest::new(
        ModelRef::new("other-provider", "gpt-test").unwrap(),
        vec![Message::user("valid")],
    );
    assert!(matches!(
        validation_client.stream(invalid).await,
        Err(LlmError::Validation(_))
    ));

    let cancelled_observer = RecordingObserver::default();
    let cancelled_client = LlmClient::new(runtime(), MockTransport::default())
        .with_observer(cancelled_observer.clone());
    let cancelled = RequestControl::new();
    cancelled.cancel();
    assert!(matches!(
        cancelled_client
            .stream_with_control(request(), cancelled)
            .await,
        Err(LlmError::Cancelled)
    ));

    let timeout_observer = RecordingObserver::default();
    let timeout_mock = MockTransport::scripted([
        success_exchange("late", "late", "late").with_start_delay(Duration::from_secs(1))
    ]);
    let timeout_client =
        LlmClient::new(runtime(), timeout_mock).with_observer(timeout_observer.clone());
    let timed = request().with_options(
        GenerationOptions::new()
            .with_timeout(Duration::from_millis(20))
            .unwrap(),
    );
    assert!(matches!(
        timeout_client.stream(timed).await,
        Err(LlmError::Timeout(_))
    ));

    let protocol_observer = RecordingObserver::default();
    let protocol_mock = MockTransport::scripted([MockExchange::response(MockResponse::new(
        StatusCode::OK,
        response_headers("req-protocol"),
        vec![MockBodyItem::chunk("data: not-json\n\n")],
    ))]);
    let protocol_client =
        LlmClient::new(runtime(), protocol_mock).with_observer(protocol_observer.clone());
    let mut protocol_stream = protocol_client.stream(request()).await.unwrap();
    assert!(matches!(
        protocol_stream.next().await,
        Some(Err(LlmError::Protocol(_)))
    ));
    assert!(protocol_stream.next().await.is_none());

    let traces = [
        validation_observer.events(),
        cancelled_observer.events(),
        timeout_observer.events(),
        protocol_observer.events(),
    ];
    assert!(matches!(
        traces[0].last().map(LifecycleEvent::kind),
        Some(LifecycleEventKind::RequestFailed {
            category: LifecycleErrorCategory::Validation,
            partial_output: false,
        })
    ));
    assert!(matches!(
        traces[1].last().map(LifecycleEvent::kind),
        Some(LifecycleEventKind::RequestCancelled {
            partial_output: false,
        })
    ));
    assert!(matches!(
        traces[2].last().map(LifecycleEvent::kind),
        Some(LifecycleEventKind::RequestTimedOut {
            partial_output: false,
        })
    ));
    assert!(matches!(
        traces[3].last().map(LifecycleEvent::kind),
        Some(LifecycleEventKind::RequestFailed {
            category: LifecycleErrorCategory::Protocol(_),
            partial_output: false,
        })
    ));

    let local_ids: BTreeSet<_> = traces
        .iter()
        .map(|events| assert_failed_trace(events))
        .collect();
    assert_eq!(local_ids.len(), traces.len());
}

#[tokio::test]
async fn concurrent_calls_have_distinct_local_ids() {
    const COUNT: usize = 12;
    let mock = MockTransport::scripted(
        (0..COUNT).map(|index| success_exchange("req", &format!("gen-{index}"), "ok")),
    );
    let client = LlmClient::new(runtime(), mock.clone());
    let mut tasks = Vec::new();
    for _ in 0..COUNT {
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            client.complete(request()).await.unwrap()
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let ids: BTreeSet<_> = mock
        .captured_requests()
        .iter()
        .map(|request| request.local_request_id().as_str().to_owned())
        .collect();
    assert_eq!(ids.len(), COUNT);
    mock.assert_consumed();
}
