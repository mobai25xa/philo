//! P3-007 header ownership, identity, and dynamic policy contracts.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use philo::provider::TestOnlyProfile;
use philo::transport::mock::{MockExchange, MockResponse, MockTransport};
use philo::{
    ClientIdentity, ClientIdentityFragment, DynamicHeaderContext, DynamicHeaderFuture,
    DynamicHeaderPolicy, DynamicHeaderSource, GenerateRequest, HeaderLayer, HeaderOperation,
    HeaderPipeline, HeaderPolicyError, HeaderPolicyFailure, HeaderSource, LlmClient, LlmError,
    Message, ModelRef,
};

const ENDPOINT: &str = "http://127.0.0.1:42007/v1/chat/completions";

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
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "header-key")
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
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "header-key")
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
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "header-key")
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
