//! Authentication provider and credential lifecycle contracts.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use philo::error::{CredentialError, CredentialFailure};
use philo::provider::auth::{
    ApiKey, ApiKeyHeaderAuth, AuthContext, AuthProvider, AuthSchemeKind, BearerAuth,
    BearerCredential, CredentialFuture, CredentialIdentity, CredentialSourceKind, DynamicAuth,
    DynamicCredential, DynamicCredentialCache, DynamicCredentialContext, DynamicCredentialSource,
    MultiHeaderAuth, NoAuth, TenantId,
};
use philo::provider::headers::{HeaderLayer, HeaderOperation, HeaderPipeline, HeaderSource};
use philo::provider::profiles::OfficialOpenAiProfile;
use philo::{GenerateRequest, LlmClient, LlmError, Message, ModelRef, RequestControl};
use support::mock_transport::{MockExchange, MockResponse, MockTransport};
use support::provider::TestProvider;
use tokio::time::Instant;

const ENDPOINT: &str = "https://test.invalid/v1/chat/completions";
const SECRET: &str = "provider-auth-dynamic-secret-canary";

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

fn test_audience() -> philo::provider::endpoint::CredentialBinding {
    let runtime = TestProvider::new(ENDPOINT, "bootstrap")
        .unwrap()
        .build()
        .unwrap();
    philo::provider::endpoint::CredentialBinding::exact_https_origin(runtime.endpoint()).unwrap()
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

#[tokio::test]
async fn static_auth_providers_enforce_complete_shapes_and_bindings() {
    let runtime = OfficialOpenAiProfile::from_api_key("bootstrap")
        .unwrap()
        .build()
        .unwrap();
    let endpoint = runtime.endpoint();
    let context = AuthContext::new(endpoint);
    let binding = philo::provider::endpoint::CredentialAudience::OfficialOpenAi;
    let api_name = HeaderName::from_static("x-api-key");
    let api = ApiKeyHeaderAuth::new(
        api_name.clone(),
        ApiKey::new("api-secret").unwrap(),
        binding.clone(),
    )
    .unwrap();

    assert_eq!(api.resolve(context).await.unwrap().len(), 1);
    assert_eq!(api.protected_headers(), vec![api_name.clone()]);
    assert!(api.credential_binding().is_some());
    assert!(api.validate_endpoint(endpoint).is_ok());
    assert_eq!(api.scheme_kind(), AuthSchemeKind::ApiKeyHeader);
    assert_eq!(api.credential_source_kind(), CredentialSourceKind::Static);
    assert!(!format!("{api:?}").contains("api-secret"));
    assert!(api.validate_final(&HeaderMap::new()).is_err());
    let mut complete = HeaderMap::new();
    complete.insert(api_name.clone(), HeaderValue::from_static("present"));
    assert!(api.validate_final(&complete).is_ok());

    assert!(
        ApiKeyHeaderAuth::new(
            header::HOST,
            ApiKey::new("secret").unwrap(),
            binding.clone(),
        )
        .is_err()
    );
    let long_name = HeaderName::from_bytes(&[b'x'; 129]).unwrap();
    assert!(
        ApiKeyHeaderAuth::new(long_name, ApiKey::new("secret").unwrap(), binding.clone()).is_err()
    );

    assert!(MultiHeaderAuth::new(Vec::new(), binding.clone()).is_err());
    let too_many = (0..9)
        .map(|index| {
            (
                HeaderName::from_bytes(format!("x-auth-{index}").as_bytes()).unwrap(),
                ApiKey::new(format!("key-{index}")).unwrap(),
            )
        })
        .collect();
    assert!(MultiHeaderAuth::new(too_many, binding.clone()).is_err());
    let duplicate = vec![
        (api_name.clone(), ApiKey::new("one").unwrap()),
        (api_name.clone(), ApiKey::new("two").unwrap()),
    ];
    assert!(MultiHeaderAuth::new(duplicate, binding.clone()).is_err());

    let signature_name = HeaderName::from_static("x-api-signature");
    let multi = MultiHeaderAuth::new(
        vec![
            (api_name.clone(), ApiKey::new("one").unwrap()),
            (signature_name.clone(), ApiKey::new("two").unwrap()),
        ],
        binding,
    )
    .unwrap();
    assert_eq!(multi.resolve(context).await.unwrap().len(), 2);
    assert_eq!(multi.protected_headers().len(), 2);
    assert!(multi.credential_binding().is_some());
    assert!(multi.validate_endpoint(endpoint).is_ok());
    assert_eq!(multi.scheme_kind(), AuthSchemeKind::MultiHeader);
    assert_eq!(multi.credential_source_kind(), CredentialSourceKind::Static);
    assert!(!format!("{multi:?}").contains("one"));
    assert!(multi.validate_final(&complete).is_err());
    complete.insert(signature_name, HeaderValue::from_static("present"));
    assert!(multi.validate_final(&complete).is_ok());

    let no_auth = NoAuth;
    assert!(no_auth.resolve(context).await.unwrap().is_empty());
    assert!(no_auth.protected_headers().is_empty());
    assert!(no_auth.credential_binding().is_none());
    assert!(no_auth.validate_endpoint(endpoint).is_ok());
    assert_eq!(no_auth.scheme_kind(), AuthSchemeKind::None);
    assert_eq!(no_auth.credential_source_kind(), CredentialSourceKind::None);
    assert!(no_auth.validate_final(&HeaderMap::new()).is_ok());
    let mut forbidden = HeaderMap::new();
    forbidden.insert(
        header::PROXY_AUTHORIZATION,
        HeaderValue::from_static("forbidden"),
    );
    assert!(no_auth.validate_final(&forbidden).is_err());
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

#[test]
fn dynamic_identifiers_and_credentials_reject_invalid_boundaries() {
    for invalid in ["", " leading", "trailing "] {
        assert!(TenantId::new(invalid).is_err());
        assert!(CredentialIdentity::new(invalid).is_err());
    }
    assert!(TenantId::new("x".repeat(257)).is_err());
    assert!(CredentialIdentity::new("x".repeat(257)).is_err());

    let tenant = TenantId::new("tenant").unwrap();
    assert_eq!(format!("{tenant:?}"), "[REDACTED]");
    assert_eq!(format!("{tenant}"), "[REDACTED]");
    assert!(DynamicCredential::bearer(ApiKey::new("expired").unwrap(), Instant::now()).is_err());
    assert!(
        DynamicCredential::api_key_header(
            header::CONTENT_TYPE,
            ApiKey::new("secret").unwrap(),
            Instant::now() + Duration::from_secs(1),
        )
        .is_err()
    );
    let credential = DynamicCredential::api_key_header(
        HeaderName::from_static("x-api-key"),
        ApiKey::new("secret").unwrap(),
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
    assert!(matches!(
        credential.scheme(),
        philo::provider::auth::DynamicCredentialScheme::ApiKeyHeader(name)
            if name == "x-api-key"
    ));
    assert!(credential.expires_at() > Instant::now());
    assert!(!format!("{credential:?}").contains("secret"));
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
            let _ = context.tenant_id();
            assert_eq!(context.provider_id().as_str(), "test-only");
            assert_eq!(context.product_id().as_str(), "chat-completions");
            let _ = context.binding();
            let _ = context.audience();
            let _ = context.credential_identity();
            assert!(context.deadline().is_none());
            assert!(!context.cancellation().is_cancelled());
            assert!(!format!("{context:?}").contains(SECRET));
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
    let runtime = TestProvider::new(ENDPOINT, "unused")
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

#[derive(Debug)]
struct ApiHeaderSource {
    name: HeaderName,
}

impl DynamicCredentialSource for ApiHeaderSource {
    fn acquire(&self, _context: DynamicCredentialContext) -> CredentialFuture {
        let name = self.name.clone();
        Box::pin(async move {
            Ok(DynamicCredential::api_key_header(
                name,
                ApiKey::new("dynamic-api-key").unwrap(),
                Instant::now() + Duration::from_mins(1),
            )
            .unwrap())
        })
    }
}

#[tokio::test]
async fn dynamic_auth_declares_metadata_and_enforces_scheme_allowlist() {
    let name = HeaderName::from_static("x-dynamic-key");
    let source = Arc::new(ApiHeaderSource { name: name.clone() });
    let make_auth = || {
        DynamicAuth::new(
            source.clone(),
            test_audience(),
            TenantId::new("tenant-api-header").unwrap(),
            CredentialIdentity::new("workload-api-header").unwrap(),
        )
    };
    assert!(make_auth().with_callback_timeout(Duration::ZERO).is_err());
    assert!(
        make_auth()
            .allow_api_key_header(header::CONTENT_TYPE)
            .is_err()
    );

    let auth = make_auth()
        .with_callback_timeout(Duration::from_secs(1))
        .unwrap()
        .with_refresh_window(Duration::from_secs(2))
        .with_still_valid_fallback(false)
        .allow_api_key_header(name.clone())
        .unwrap()
        .allow_api_key_header(name.clone())
        .unwrap();
    assert_eq!(auth.protected_headers().len(), 2);
    assert!(auth.credential_binding().is_some());
    assert_eq!(auth.scheme_kind(), AuthSchemeKind::Dynamic);
    assert_eq!(auth.credential_source_kind(), CredentialSourceKind::Dynamic);
    assert!(!format!("{auth:?}").contains("workload-api-header"));
    assert!(auth.validate_final(&HeaderMap::new()).is_err());
    let mut headers = HeaderMap::new();
    headers.insert(name.clone(), HeaderValue::from_static("present"));
    assert!(auth.validate_final(&headers).is_ok());
    headers.insert(header::AUTHORIZATION, HeaderValue::from_static("present"));
    assert!(auth.validate_final(&headers).is_err());

    let bare_runtime = TestProvider::new(ENDPOINT, "unused")
        .unwrap()
        .build()
        .unwrap();
    assert!(
        auth.resolve(AuthContext::new(bare_runtime.endpoint()))
            .await
            .is_err()
    );

    let runtime = TestProvider::new(ENDPOINT, "unused")
        .unwrap()
        .with_auth_provider(auth)
        .build()
        .unwrap();
    let mock = MockTransport::scripted([response()]);
    let client = LlmClient::new(runtime, mock.clone());
    assert!(client.stream(request()).await.is_ok());
    assert_eq!(
        mock.captured_requests()[0].headers()[name],
        "dynamic-api-key"
    );

    let rejected_runtime = TestProvider::new(ENDPOINT, "unused")
        .unwrap()
        .with_auth_provider(make_auth())
        .build()
        .unwrap();
    let rejected_transport = MockTransport::default();
    let error = LlmClient::new(rejected_runtime, rejected_transport.clone())
        .stream(request())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        LlmError::Credential(ref error) if error.kind() == CredentialFailure::Invalid
    ));
    assert!(rejected_transport.captured_requests().is_empty());
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
    let runtime_a = TestProvider::new(ENDPOINT, "unused")
        .unwrap()
        .with_auth_provider(make_auth(
            "tenant-a",
            "workload",
            SECRET_A,
            Arc::clone(&calls_a),
        ))
        .build()
        .unwrap();
    let runtime_b = TestProvider::new(ENDPOINT, "unused")
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
    let runtime = TestProvider::new(ENDPOINT, "unused")
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
