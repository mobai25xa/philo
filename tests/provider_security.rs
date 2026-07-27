//! Public API security and decision-table coverage for P1-007 through P1-010.

use std::sync::Arc;

use http::{HeaderMap, HeaderName, HeaderValue, header};
use philo::domain::request::CapabilityStatus;
use philo::provider::auth::{ApiKey, AuthContext, BearerAuth, BearerCredential, ClientIdentity};
use philo::provider::endpoint::{
    CredentialAudience, EndpointConfig, RedirectPolicy, resolve_official, resolve_test_only,
};
use philo::provider::headers::{
    HeaderLayer, HeaderOperation, HeaderPipeline, HeaderSource, SensitiveHeaderValue,
};
use philo::provider::{OfficialOpenAiProfile, TestOnlyProfile};
use philo::{LlmError, SDK_VERSION};
use url::Url;

const CANARY: &str = "philo-canary-secret-8bd758";

#[test]
fn official_profile_builds_golden_endpoint_and_capabilities() {
    let runtime = OfficialOpenAiProfile::from_api_key(CANARY)
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(runtime.provider_id().as_str(), "official-openai");
    assert_eq!(runtime.protocol_id().as_str(), "openai-chat-completions");
    assert_eq!(
        runtime.endpoint().url().as_str(),
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(runtime.method(), http::Method::POST);
    assert_eq!(
        runtime.capabilities().developer_role,
        CapabilityStatus::Supported
    );
    assert_eq!(
        runtime.capabilities().streaming_usage,
        CapabilityStatus::Supported
    );
    assert_eq!(
        runtime.transport_options().redirect_policy(),
        RedirectPolicy::Disabled
    );
    assert!(!format!("{runtime:?}").contains(CANARY));
}

#[test]
fn endpoint_resolution_preserves_base_path_and_rejects_unsafe_parts() {
    let endpoint = resolve_official(
        &EndpointConfig::base_and_path("https://api.openai.com/v1/", "/chat/completions").unwrap(),
    )
    .unwrap();
    assert_eq!(endpoint.url().path(), "/v1/chat/completions");
    assert!(!endpoint.url().path().contains("//"));

    for value in [
        "https://user:pass@api.openai.com/v1/chat/completions",
        "https://api.openai.com/v1/chat/completions?key=value",
        "https://api.openai.com/v1/chat/completions#fragment",
        "ftp://api.openai.com/v1/chat/completions",
    ] {
        assert!(resolve_official(&EndpointConfig::absolute(value).unwrap()).is_err());
    }
    assert!(
        EndpointConfig::base_and_path("https://api.openai.com/v1", "/chat?bad")
            .and_then(|config| resolve_official(&config))
            .is_err()
    );
}

#[test]
fn test_profile_is_explicit_and_loopback_only() {
    assert!(
        TestOnlyProfile::localhost("http://127.0.0.1:8080/v1/chat/completions", CANARY).is_ok()
    );
    assert!(
        TestOnlyProfile::localhost("http://localhost:8080/v1/chat/completions", CANARY).is_ok()
    );
    assert!(TestOnlyProfile::localhost("https://example.com/v1/chat/completions", CANARY).is_err());
}

#[test]
fn official_credential_cannot_be_sent_to_test_origin() {
    let endpoint = resolve_test_only(
        &EndpointConfig::absolute("http://127.0.0.1:8080/v1/chat/completions").unwrap(),
    )
    .unwrap();
    let credential = BearerCredential::new(
        ApiKey::new(CANARY).unwrap(),
        CredentialAudience::OfficialOpenAi,
    );
    let auth = BearerAuth::new(credential);
    assert!(auth.operation(AuthContext::new(&endpoint)).is_err());
}

#[test]
fn redirect_policy_rejects_disabled_and_cross_origin_redirects() {
    let endpoint = resolve_official(
        &EndpointConfig::absolute("https://api.openai.com/v1/chat/completions").unwrap(),
    )
    .unwrap();
    let same_origin = Url::parse("https://api.openai.com/v1/other").unwrap();
    let cross_origin = Url::parse("https://example.com/v1/other").unwrap();
    let different_port = Url::parse("https://api.openai.com:444/v1/other").unwrap();
    assert!(
        RedirectPolicy::Disabled
            .validate(&endpoint, &same_origin)
            .is_err()
    );
    assert!(
        RedirectPolicy::SameOrigin
            .validate(&endpoint, &same_origin)
            .is_ok()
    );
    assert!(
        RedirectPolicy::SameOrigin
            .validate(&endpoint, &cross_origin)
            .is_err()
    );
    assert!(
        RedirectPolicy::SameOrigin
            .validate(&endpoint, &different_port)
            .is_err()
    );
}

fn required_layers(extra: Vec<HeaderLayer>) -> Vec<HeaderLayer> {
    let mut layers = vec![
        HeaderLayer::new(
            HeaderSource::Protocol,
            vec![HeaderOperation::set(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
        ),
        HeaderLayer::new(
            HeaderSource::Auth,
            vec![HeaderOperation::set_sensitive(
                header::AUTHORIZATION,
                HeaderValue::from_static("Bearer canary"),
            )],
        ),
    ];
    layers.extend(extra);
    layers
}

#[test]
fn header_pipeline_applies_priority_remove_and_value_free_trace() {
    let name = HeaderName::from_static("x-philo-test");
    let resolved = HeaderPipeline::new()
        .resolve(required_layers(vec![
            HeaderLayer::new(
                HeaderSource::Provider,
                vec![HeaderOperation::set(
                    name.clone(),
                    HeaderValue::from_static("provider"),
                )],
            ),
            HeaderLayer::new(
                HeaderSource::Model,
                vec![HeaderOperation::remove(name.clone())],
            ),
            HeaderLayer::new(
                HeaderSource::Request,
                vec![HeaderOperation::set(
                    name.clone(),
                    HeaderValue::from_static("request"),
                )],
            ),
        ]))
        .unwrap();
    assert_eq!(resolved.headers().get(&name).unwrap(), "request");
    assert_eq!(resolved.final_source(&name), Some(HeaderSource::Request));
    let debug = format!("{resolved:?}");
    assert!(!debug.contains("Bearer canary"));
    assert!(!debug.contains("provider"));
    assert!(!debug.contains("request"));
}

#[test]
fn same_layer_set_remove_semantics_follow_operation_order() {
    let name = HeaderName::from_static("x-philo-order");
    let removed = HeaderPipeline::new()
        .resolve(required_layers(vec![HeaderLayer::new(
            HeaderSource::Provider,
            vec![
                HeaderOperation::set(name.clone(), HeaderValue::from_static("first")),
                HeaderOperation::remove(name.clone()),
            ],
        )]))
        .unwrap();
    assert!(!removed.headers().contains_key(&name));

    let restored = HeaderPipeline::new()
        .resolve(required_layers(vec![HeaderLayer::new(
            HeaderSource::Provider,
            vec![
                HeaderOperation::remove(name.clone()),
                HeaderOperation::set(name.clone(), HeaderValue::from_static("second")),
            ],
        )]))
        .unwrap();
    assert_eq!(restored.headers().get(&name).unwrap(), "second");
    assert_eq!(restored.final_source(&name), Some(HeaderSource::Provider));
}

#[test]
fn protected_headers_are_case_insensitive_and_auth_only() {
    let upper_authorization = HeaderName::from_bytes(b"AUTHORIZATION").unwrap();
    let result = HeaderPipeline::new().resolve(required_layers(vec![HeaderLayer::new(
        HeaderSource::Request,
        vec![HeaderOperation::set(
            upper_authorization,
            HeaderValue::from_static("Bearer attacker"),
        )],
    )]));
    assert!(matches!(result, Err(LlmError::Validation(_))));

    let remove_content_type =
        HeaderPipeline::new().resolve(required_layers(vec![HeaderLayer::new(
            HeaderSource::Request,
            vec![HeaderOperation::remove(header::CONTENT_TYPE)],
        )]));
    assert!(matches!(remove_content_type, Err(LlmError::Validation(_))));
    let remove_authorization =
        HeaderPipeline::new().resolve(required_layers(vec![HeaderLayer::new(
            HeaderSource::Request,
            vec![HeaderOperation::remove(header::AUTHORIZATION)],
        )]));
    assert!(matches!(remove_authorization, Err(LlmError::Validation(_))));
    assert!(SensitiveHeaderValue::from_bytes(b"ok\r\nbad", false).is_err());
}

#[test]
fn bearer_and_client_identity_headers_are_correct_but_redacted() {
    let profile = OfficialOpenAiProfile::from_api_key(CANARY).unwrap();
    assert!(!format!("{profile:?}").contains(CANARY));
    let runtime = profile.build().unwrap();
    let resolved = runtime
        .resolve_headers(Vec::new(), &HeaderMap::new())
        .unwrap();
    assert_eq!(
        resolved
            .headers()
            .get(header::AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap(),
        format!("Bearer {CANARY}")
    );
    assert_eq!(
        resolved
            .headers()
            .get(header::USER_AGENT)
            .unwrap()
            .to_str()
            .unwrap(),
        format!("philo/{SDK_VERSION}")
    );
    assert_eq!(
        resolved.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    assert_eq!(
        resolved.headers().get(header::ACCEPT).unwrap(),
        "text/event-stream"
    );
    assert!(!format!("{resolved:?}").contains(CANARY));
    assert_eq!(
        resolved.final_source(&header::AUTHORIZATION),
        Some(HeaderSource::Auth)
    );
}

#[test]
fn client_identity_is_controlled_and_cannot_impersonate_openai() {
    assert!(ClientIdentity::new("my-app", "1.2.3").is_ok());
    assert!(ClientIdentity::new("openai-rust", "1.0").is_err());
    assert!(ClientIdentity::new("bad product", "1.0").is_err());
}

#[tokio::test]
async fn runtime_is_shareable_and_request_headers_do_not_cross_talk() {
    let runtime = Arc::new(
        OfficialOpenAiProfile::from_api_key(CANARY)
            .unwrap()
            .build()
            .unwrap(),
    );
    let mut tasks = Vec::new();
    for index in 0..8 {
        let runtime = Arc::clone(&runtime);
        tasks.push(tokio::spawn(async move {
            let expected = format!("request-{index}");
            let mut request = HeaderMap::new();
            request.insert(
                HeaderName::from_static("x-philo-request"),
                HeaderValue::from_str(&expected).unwrap(),
            );
            let resolved = runtime.resolve_headers(Vec::new(), &request).unwrap();
            assert_eq!(
                resolved
                    .headers()
                    .get("x-philo-request")
                    .unwrap()
                    .to_str()
                    .unwrap(),
                expected
            );
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
}
