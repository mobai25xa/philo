//! Header ownership, identity, and dynamic policy contracts.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use philo::error::{HeaderPolicyError, HeaderPolicyFailure};
use philo::provider::headers::{
    ClientIdentity, ClientIdentityFragment, DynamicHeaderContext, DynamicHeaderFuture,
    DynamicHeaderPolicy, DynamicHeaderSource, DynamicResponseFormat, HeaderLayer, HeaderOperation,
    HeaderPipeline, HeaderSource,
};
use philo::{GenerateRequest, LlmClient, LlmError, Message, ModelRef};
use support::mock_transport::{MockExchange, MockResponse, MockTransport};
use support::provider::TestProvider;

const ENDPOINT: &str = "https://test.invalid/v1/chat/completions";

fn request() -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        vec![Message::user("header policy prompt")],
    )
}

fn response() -> MockExchange {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    MockExchange::response(MockResponse::new(StatusCode::OK, headers, Vec::new()))
}

fn required_layers(extra: HeaderLayer) -> Vec<HeaderLayer> {
    vec![
        HeaderLayer::new(
            HeaderSource::Protocol,
            vec![HeaderOperation::set(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
        ),
        extra,
        HeaderLayer::new(
            HeaderSource::Auth,
            vec![HeaderOperation::set_sensitive(
                header::AUTHORIZATION,
                HeaderValue::from_static("Bearer secret"),
            )],
        ),
    ]
}

#[test]
fn protected_owner_matrix_rejects_cross_layer_writes() {
    for (source, name) in [
        (HeaderSource::Request, header::CONTENT_TYPE),
        (HeaderSource::Provider, header::AUTHORIZATION),
        (HeaderSource::Request, header::HOST),
        (HeaderSource::DynamicPolicy, header::USER_AGENT),
        (HeaderSource::Provider, header::COOKIE),
    ] {
        let result = HeaderPipeline::new().resolve(required_layers(HeaderLayer::new(
            source,
            vec![HeaderOperation::set(
                name,
                HeaderValue::from_static("blocked"),
            )],
        )));
        assert!(matches!(result, Err(LlmError::Validation(_))));
    }
}

#[test]
fn user_agent_is_owned_by_structured_client_identity() {
    let identity = ClientIdentity::new("acme-agent", "2.1")
        .unwrap()
        .with_application(ClientIdentityFragment::new("billing-app", Some("4".to_owned())).unwrap())
        .unwrap()
        .with_contact("ops@example.com")
        .unwrap();
    let resolved = HeaderPipeline::new()
        .resolve(required_layers(HeaderLayer::new(
            HeaderSource::ClientIdentity,
            vec![identity.operation().unwrap()],
        )))
        .unwrap();
    assert_eq!(
        resolved.headers()[header::USER_AGENT],
        "acme-agent/2.1 billing-app/4 (ops@example.com)"
    );
    assert!(ClientIdentity::new("Mozilla", "5.0").is_err());
}

#[derive(Debug)]
struct RecordingPolicy {
    seen: Arc<Mutex<Vec<(String, u32, bool)>>>,
    operation: HeaderOperation,
    delay: Duration,
}

impl DynamicHeaderSource for RecordingPolicy {
    fn resolve(&self, context: DynamicHeaderContext) -> DynamicHeaderFuture {
        let seen = Arc::clone(&self.seen);
        let operation = self.operation.clone();
        let delay = self.delay;
        Box::pin(async move {
            let _ = context.product_id();
            let _ = context.model_id();
            let _ = context.protocol_id();
            let _ = context.local_request_id();
            let _ = context.contains_images();
            let _ = context.reasoning_enabled();
            assert_eq!(context.response_format(), DynamicResponseFormat::Text);
            assert!(context.deadline().is_none());
            assert!(!context.cancellation().is_cancelled());
            seen.lock().unwrap().push((
                context.provider_id().as_str().to_owned(),
                context.attempt_number(),
                context.contains_tools(),
            ));
            tokio::time::sleep(delay).await;
            Ok(vec![operation])
        })
    }
}

#[tokio::test]
async fn dynamic_policy_sees_value_free_facts_and_applies_allowlisted_header() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let name = HeaderName::from_static("x-philo-session");
    let source = Arc::new(RecordingPolicy {
        seen: Arc::clone(&seen),
        operation: HeaderOperation::set(name.clone(), HeaderValue::from_static("session-a")),
        delay: Duration::ZERO,
    });
    let policy = DynamicHeaderPolicy::new(source, vec![name.clone()]).unwrap();
    let runtime = TestProvider::new(ENDPOINT, "header-key")
        .unwrap()
        .with_dynamic_header_policy(policy)
        .build()
        .unwrap();
    let mock = MockTransport::scripted([response()]);
    let client = LlmClient::new(runtime, mock.clone());
    assert!(client.stream(request()).await.is_ok());
    assert_eq!(mock.captured_requests()[0].headers()[name], "session-a");
    assert_eq!(
        &*seen.lock().unwrap(),
        &[("test-only".to_owned(), 1, false)]
    );
}

#[tokio::test]
async fn dynamic_policy_illegal_sensitive_value_fails_before_io() {
    let name = HeaderName::from_static("x-philo-session");
    let source = Arc::new(RecordingPolicy {
        seen: Arc::new(Mutex::new(Vec::new())),
        operation: HeaderOperation::set_sensitive(name.clone(), HeaderValue::from_static("secret")),
        delay: Duration::ZERO,
    });
    let policy = DynamicHeaderPolicy::new(source, vec![name]).unwrap();
    let runtime = TestProvider::new(ENDPOINT, "header-key")
        .unwrap()
        .with_dynamic_header_policy(policy)
        .build()
        .unwrap();
    let mock = MockTransport::default();
    let error = LlmClient::new(runtime, mock.clone())
        .stream(request())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        LlmError::HeaderPolicy(ref error)
            if error.kind() == HeaderPolicyFailure::InvalidOperation
    ));
    assert!(mock.captured_requests().is_empty());
}

#[tokio::test]
async fn dynamic_policy_timeout_fails_before_io() {
    let name = HeaderName::from_static("x-philo-session");
    let source = Arc::new(RecordingPolicy {
        seen: Arc::new(Mutex::new(Vec::new())),
        operation: HeaderOperation::set(name.clone(), HeaderValue::from_static("late")),
        delay: Duration::from_secs(1),
    });
    let policy = DynamicHeaderPolicy::new(source, vec![name])
        .unwrap()
        .with_timeout(Duration::from_millis(5))
        .unwrap();
    let runtime = TestProvider::new(ENDPOINT, "header-key")
        .unwrap()
        .with_dynamic_header_policy(policy)
        .build()
        .unwrap();
    let mock = MockTransport::default();
    let error = LlmClient::new(runtime, mock.clone())
        .stream(request())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        LlmError::HeaderPolicy(ref error) if error.kind() == HeaderPolicyFailure::Timeout
    ));
    assert!(mock.captured_requests().is_empty());
}

#[test]
fn dynamic_allowlist_rejects_security_owner_headers_at_configuration_time() {
    let source = Arc::new(RecordingPolicy {
        seen: Arc::new(Mutex::new(Vec::new())),
        operation: HeaderOperation::remove(header::AUTHORIZATION),
        delay: Duration::ZERO,
    });
    let result = DynamicHeaderPolicy::new(source, vec![header::AUTHORIZATION]);
    assert!(matches!(result, Err(LlmError::HeaderPolicy(_))));
    let error = HeaderPolicyError::new(HeaderPolicyFailure::InvalidOperation);
    assert_eq!(error.kind(), HeaderPolicyFailure::InvalidOperation);
}

#[test]
fn dynamic_policy_configuration_enforces_allowlist_and_budgets() {
    let name = HeaderName::from_static("x-philo-session");
    let source = Arc::new(RecordingPolicy {
        seen: Arc::new(Mutex::new(Vec::new())),
        operation: HeaderOperation::remove(name.clone()),
        delay: Duration::ZERO,
    });
    assert!(DynamicHeaderPolicy::new(source.clone(), Vec::new()).is_err());
    assert!(DynamicHeaderPolicy::new(source.clone(), vec![name.clone(); 33]).is_err());
    let policy = DynamicHeaderPolicy::new(source, vec![name.clone()]).unwrap();
    assert_eq!(policy.allowed_headers(), &[name]);
    assert!(format!("{policy:?}").contains("DynamicHeaderPolicy"));
    assert!(policy.clone().with_timeout(Duration::ZERO).is_err());
    for (operations, bytes) in [(0, 1), (65, 1), (1, 0), (1, 65 * 1024)] {
        assert!(policy.clone().with_budget(operations, bytes).is_err());
    }
    assert!(policy.with_budget(1, 64).is_ok());
}

#[derive(Debug)]
struct OperationsPolicy {
    operations: Vec<HeaderOperation>,
    failure: Option<HeaderPolicyFailure>,
}

impl DynamicHeaderSource for OperationsPolicy {
    fn resolve(&self, _context: DynamicHeaderContext) -> DynamicHeaderFuture {
        let operations = self.operations.clone();
        let failure = self.failure;
        Box::pin(async move {
            if let Some(failure) = failure {
                Err(HeaderPolicyError::new(failure))
            } else {
                Ok(operations)
            }
        })
    }
}

async fn policy_error(
    source: OperationsPolicy,
    allowlist: Vec<HeaderName>,
    budget: (usize, usize),
) -> HeaderPolicyFailure {
    let policy = DynamicHeaderPolicy::new(Arc::new(source), allowlist)
        .unwrap()
        .with_budget(budget.0, budget.1)
        .unwrap();
    let runtime = TestProvider::new(ENDPOINT, "header-key")
        .unwrap()
        .with_dynamic_header_policy(policy)
        .build()
        .unwrap();
    let error = LlmClient::new(runtime, MockTransport::default())
        .stream(request())
        .await
        .unwrap_err();
    match error {
        LlmError::HeaderPolicy(error) => error.kind(),
        unexpected => panic!("unexpected error: {unexpected:?}"),
    }
}

#[tokio::test]
async fn dynamic_policy_enforces_callback_operation_and_byte_results() {
    let allowed = HeaderName::from_static("x-allowed");
    let other = HeaderName::from_static("x-other");
    let failure = policy_error(
        OperationsPolicy {
            operations: Vec::new(),
            failure: Some(HeaderPolicyFailure::InvalidOperation),
        },
        vec![allowed.clone()],
        (1, 64),
    )
    .await;
    assert_eq!(failure, HeaderPolicyFailure::Callback);

    let failure = policy_error(
        OperationsPolicy {
            operations: vec![
                HeaderOperation::remove(allowed.clone()),
                HeaderOperation::remove(allowed.clone()),
            ],
            failure: None,
        },
        vec![allowed.clone()],
        (1, 64),
    )
    .await;
    assert_eq!(failure, HeaderPolicyFailure::BudgetExceeded);

    let failure = policy_error(
        OperationsPolicy {
            operations: vec![HeaderOperation::set(
                other,
                HeaderValue::from_static("value"),
            )],
            failure: None,
        },
        vec![allowed.clone()],
        (1, 64),
    )
    .await;
    assert_eq!(failure, HeaderPolicyFailure::InvalidOperation);

    let failure = policy_error(
        OperationsPolicy {
            operations: vec![HeaderOperation::remove(HeaderName::from_static("x-other"))],
            failure: None,
        },
        vec![allowed.clone()],
        (1, 64),
    )
    .await;
    assert_eq!(failure, HeaderPolicyFailure::InvalidOperation);

    let failure = policy_error(
        OperationsPolicy {
            operations: vec![HeaderOperation::set(
                allowed.clone(),
                HeaderValue::from_static("a-long-value"),
            )],
            failure: None,
        },
        vec![allowed],
        (1, 1),
    )
    .await;
    assert_eq!(failure, HeaderPolicyFailure::BudgetExceeded);
}
