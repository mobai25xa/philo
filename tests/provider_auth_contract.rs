//! P3-006 authentication provider and credential lifecycle contracts.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use philo::error::{CredentialError, CredentialFailure};
use philo::provider::TestOnlyProfile;
use philo::provider::auth::{
    ApiKey, ApiKeyHeaderAuth, AuthContext, AuthProvider, BearerAuth, BearerCredential,
    CredentialFuture, CredentialIdentity, DynamicAuth, DynamicCredential, DynamicCredentialCache,
    DynamicCredentialContext, DynamicCredentialSource, MultiHeaderAuth, NoAuth, TenantId,
};
use philo::provider::headers::{HeaderLayer, HeaderOperation, HeaderPipeline, HeaderSource};
use philo::provider::profiles::OfficialOpenAiProfile;
use philo::transport::mock::{MockExchange, MockResponse, MockTransport};
use philo::{GenerateRequest, LlmClient, LlmError, Message, ModelRef, RequestControl};
use tokio::time::Instant;

const ENDPOINT: &str = "http://127.0.0.1:42006/v1/chat/completions";
const SECRET: &str = "p3-006-dynamic-secret-canary";

fn request() -> GenerateRequest {
    GenerateRequest::new(
        ModelRef::new("test-only", "gpt-test").unwrap(),
        vec![Message::user("hello")],
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

fn test_audience() -> philo::provider::endpoint::CredentialAudience {
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "bootstrap")
        .unwrap()
        .build()
        .unwrap();
    philo::provider::endpoint::CredentialAudience::TestOnlyExactOrigin(
        runtime.endpoint().origin().clone(),
    )
}

#[test]
fn bearer_api_key_multi_header_and_no_auth_have_explicit_shapes() {
    let runtime = OfficialOpenAiProfile::from_api_key("bootstrap")
        .unwrap()
        .build()
        .unwrap();
    let audience = philo::provider::endpoint::CredentialAudience::OfficialOpenAi;
    let context = AuthContext::new(runtime.endpoint());

    let bearer = BearerAuth::new(BearerCredential::new(
        ApiKey::new("bearer-secret").unwrap(),
        audience.clone(),
    ));
    assert_eq!(bearer.resolve_immediate(context).unwrap().len(), 1);

    let api_key = ApiKeyHeaderAuth::new(
        HeaderName::from_static("x-api-key"),
        ApiKey::new("api-secret").unwrap(),
        audience.clone(),
    )
    .unwrap();
    let operations = api_key.resolve_immediate(context).unwrap();
    let resolved = HeaderPipeline::with_auth_headers(api_key.protected_headers())
        .resolve(vec![
            HeaderLayer::new(
                HeaderSource::Protocol,
                vec![HeaderOperation::set(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                )],
            ),
            HeaderLayer::new(HeaderSource::Auth, operations),
        ])
        .unwrap();
    assert_eq!(resolved.headers()["x-api-key"], "api-secret");

    let multi = MultiHeaderAuth::new(
        vec![
            (
                HeaderName::from_static("x-api-key"),
                ApiKey::new("one").unwrap(),
            ),
            (
                HeaderName::from_static("x-api-signature"),
                ApiKey::new("two").unwrap(),
            ),
        ],
        audience,
    )
    .unwrap();
    assert_eq!(multi.resolve_immediate(context).unwrap().len(), 2);
    assert!(NoAuth.resolve_immediate(context).unwrap().is_empty());
}

#[test]
fn auth_header_names_are_registered_and_ordinary_layers_cannot_write_them() {
    let name = HeaderName::from_static("x-api-key");
    let pipeline = HeaderPipeline::with_auth_headers([name.clone()]);
    let result = pipeline.resolve(vec![
        HeaderLayer::new(
            HeaderSource::Protocol,
            vec![HeaderOperation::set(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
        ),
        HeaderLayer::new(
            HeaderSource::Request,
            vec![HeaderOperation::set(
                name,
                HeaderValue::from_static("attacker"),
            )],
        ),
    ]);
    assert!(matches!(result, Err(LlmError::Validation(_))));
    assert!(
        ApiKeyHeaderAuth::new(
            header::CONTENT_TYPE,
            ApiKey::new("secret").unwrap(),
            philo::provider::endpoint::CredentialAudience::OfficialOpenAi,
        )
        .is_err()
    );
}

#[derive(Debug)]
struct CountingSource {
    calls: Arc<AtomicUsize>,
    delay: Duration,
}

impl DynamicCredentialSource for CountingSource {
    fn acquire(&self, context: DynamicCredentialContext) -> CredentialFuture {
        let calls = Arc::clone(&self.calls);
        let delay = self.delay;
        Box::pin(async move {
            assert_eq!(context.provider_id().as_str(), "test-only");
            assert_eq!(context.product_id().as_str(), "chat-completions");
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            DynamicCredential::bearer(
                ApiKey::new(SECRET).unwrap(),
                Instant::now() + Duration::from_mins(1),
            )
        })
    }
}

#[tokio::test]
async fn same_dynamic_cache_key_refreshes_once_under_concurrency() {
    let calls = Arc::new(AtomicUsize::new(0));
    let source = Arc::new(CountingSource {
        calls: Arc::clone(&calls),
        delay: Duration::from_millis(20),
    });
    let auth = DynamicAuth::new(
        source,
        test_audience(),
        TenantId::new("tenant-a").unwrap(),
        CredentialIdentity::new("workload-a").unwrap(),
    );
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "unused")
        .unwrap()
        .with_auth_provider(auth)
        .build()
        .unwrap();
    let mock = MockTransport::scripted([response(), response()]);
    let client = LlmClient::new(runtime, mock.clone());
    let (first, second) = tokio::join!(client.stream(request()), client.stream(request()));
    assert!(first.is_ok());
    assert!(second.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let captured = mock.captured_requests();
    assert_eq!(captured.len(), 2);
    assert!(
        captured.iter().all(|request| {
            request.headers()[header::AUTHORIZATION] == format!("Bearer {SECRET}")
        })
    );
    assert!(!format!("{client:?}").contains(SECRET));
}

#[derive(Debug)]
struct PartitionSource {
    calls: Arc<AtomicUsize>,
    secret: &'static str,
}

impl DynamicCredentialSource for PartitionSource {
    fn acquire(&self, _context: DynamicCredentialContext) -> CredentialFuture {
        let calls = Arc::clone(&self.calls);
        let secret = self.secret;
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            DynamicCredential::bearer(
                ApiKey::new(secret).unwrap(),
                Instant::now() + Duration::from_mins(1),
            )
        })
    }
}

#[tokio::test]
async fn shared_cache_isolates_tenant_and_credential_identity() {
    const SECRET_A: &str = "tenant-a-credential-canary";
    const SECRET_B: &str = "tenant-b-credential-canary";
    let cache = DynamicCredentialCache::new();
    let calls_a = Arc::new(AtomicUsize::new(0));
    let calls_b = Arc::new(AtomicUsize::new(0));
    let make_auth = |tenant: &str, identity: &str, secret, calls: Arc<AtomicUsize>| {
        DynamicAuth::new(
            Arc::new(PartitionSource { calls, secret }),
            test_audience(),
            TenantId::new(tenant).unwrap(),
            CredentialIdentity::new(identity).unwrap(),
        )
        .with_cache(cache.clone())
    };
    let runtime_a = TestOnlyProfile::localhost(ENDPOINT, "unused")
        .unwrap()
        .with_auth_provider(make_auth(
            "tenant-a",
            "workload",
            SECRET_A,
            Arc::clone(&calls_a),
        ))
        .build()
        .unwrap();
    let runtime_b = TestOnlyProfile::localhost(ENDPOINT, "unused")
        .unwrap()
        .with_auth_provider(make_auth(
            "tenant-b",
            "workload",
            SECRET_B,
            Arc::clone(&calls_b),
        ))
        .build()
        .unwrap();
    let transport_a = MockTransport::scripted([response()]);
    let transport_b = MockTransport::scripted([response()]);
    let client_a = LlmClient::new(runtime_a, transport_a.clone());
    let client_b = LlmClient::new(runtime_b, transport_b.clone());
    let (result_a, result_b) = tokio::join!(client_a.stream(request()), client_b.stream(request()));
    assert!(result_a.is_ok());
    assert!(result_b.is_ok());
    assert_eq!(calls_a.load(Ordering::SeqCst), 1);
    assert_eq!(calls_b.load(Ordering::SeqCst), 1);
    assert_eq!(
        transport_a.captured_requests()[0].headers()[header::AUTHORIZATION],
        format!("Bearer {SECRET_A}")
    );
    assert_eq!(
        transport_b.captured_requests()[0].headers()[header::AUTHORIZATION],
        format!("Bearer {SECRET_B}")
    );
}

#[tokio::test]
async fn dynamic_timeout_and_cancellation_fail_before_transport() {
    let calls = Arc::new(AtomicUsize::new(0));
    let source = Arc::new(CountingSource {
        calls: Arc::clone(&calls),
        delay: Duration::from_secs(1),
    });
    let auth = DynamicAuth::new(
        source,
        test_audience(),
        TenantId::new("tenant-timeout").unwrap(),
        CredentialIdentity::new("workload-timeout").unwrap(),
    )
    .with_callback_timeout(Duration::from_millis(5))
    .unwrap();
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "unused")
        .unwrap()
        .with_auth_provider(auth)
        .build()
        .unwrap();
    let mock = MockTransport::default();
    let client = LlmClient::new(runtime, mock.clone());
    let error = client.stream(request()).await.unwrap_err();
    assert!(matches!(
        error,
        LlmError::Credential(ref error) if error.kind() == CredentialFailure::Timeout
    ));
    assert!(mock.captured_requests().is_empty());

    let control = RequestControl::new();
    control.cancel();
    let error = client
        .stream_with_control(request(), control)
        .await
        .unwrap_err();
    assert!(matches!(error, LlmError::Cancelled));
    assert!(mock.captured_requests().is_empty());
}

#[test]
fn credential_errors_and_debug_output_are_value_free() {
    let error = CredentialError::new(CredentialFailure::Unavailable);
    assert!(!format!("{error:?}").contains(SECRET));
    let identity = CredentialIdentity::new(SECRET).unwrap();
    assert_eq!(format!("{identity:?}"), "[REDACTED]");
    assert_eq!(format!("{identity}"), "[REDACTED]");
}
