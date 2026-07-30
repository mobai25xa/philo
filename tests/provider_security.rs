//! Public API security and endpoint decision-table coverage.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use http::{HeaderMap, HeaderName, HeaderValue, header};
use philo::domain::request::CapabilityStatus;
use philo::provider::OfficialOpenAiProfile;
use philo::provider::auth::{
    ApiKey, AuthContext, AuthProvider, BearerAuth, BearerCredential, ClientIdentity,
};
use philo::provider::endpoint::{
    CredentialAudience, EndpointConfig, EndpointNetworkPolicy, RedirectPolicy, resolve_official,
};
use philo::provider::headers::{
    HeaderLayer, HeaderOperation, HeaderPipeline, HeaderSource, SensitiveHeaderValue,
};
use philo::{LlmError, SDK_VERSION};
use philo_presets::{OpenRouterProfile, ZaiCodingProfile, ZaiStandardProfile};
use url::Url;

const CANARY: &str = "philo-canary-secret-8bd758";
const HARDENING_CANARY: &str = "security-hardening-credential-canary";

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
fn production_endpoint_policy_rejects_loopback_http() {
    for endpoint in [
        "http://127.0.0.1:8080/v1/chat/completions",
        "http://localhost:8080/v1/chat/completions",
    ] {
        assert!(resolve_official(&EndpointConfig::absolute(endpoint).unwrap()).is_err());
    }
}

#[test]
fn official_credential_cannot_be_sent_to_foreign_origin() {
    let endpoint =
        resolve_official(&EndpointConfig::absolute("https://example.com/v1/chat").unwrap())
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

#[test]
fn dns_address_policy_rejects_private_link_local_metadata_and_mapped_addresses() {
    let policy = EndpointNetworkPolicy::public_https();
    assert!(
        policy
            .validate_resolved_addresses([IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))])
            .is_ok()
    );
    for address in [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
        IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        "fe80::1".parse().unwrap(),
        "fd00::1".parse().unwrap(),
        "::ffff:127.0.0.1".parse().unwrap(),
    ] {
        assert!(policy.validate_resolved_addresses([address]).is_err());
    }
    assert!(policy.validate_resolved_addresses([]).is_err());
    assert!(
        EndpointNetworkPolicy::test_loopback()
            .validate_resolved_addresses([IpAddr::V4(Ipv4Addr::LOCALHOST)])
            .is_ok()
    );
}

#[test]
fn product_scoped_credentials_cannot_cross_provider_or_zai_product_boundaries() {
    let standard_auth = BearerAuth::new(BearerCredential::new(
        ApiKey::new(HARDENING_CANARY).unwrap(),
        CredentialAudience::ZaiStandard,
    ));
    assert!(
        ZaiCodingProfile::from_api_key("bootstrap")
            .unwrap()
            .with_auth_provider(standard_auth)
            .build()
            .is_err()
    );

    let openrouter = OpenRouterProfile::from_api_key(HARDENING_CANARY)
        .unwrap()
        .build()
        .unwrap();
    let zai = ZaiStandardProfile::from_api_key(HARDENING_CANARY)
        .unwrap()
        .build()
        .unwrap();
    let openrouter_auth = BearerAuth::new(BearerCredential::new(
        ApiKey::new(HARDENING_CANARY).unwrap(),
        CredentialAudience::OpenRouterApi,
    ));
    assert!(
        openrouter_auth
            .resolve_immediate(AuthContext::new(zai.endpoint()))
            .is_err()
    );
    assert!(
        BearerAuth::new(BearerCredential::new(
            ApiKey::new(HARDENING_CANARY).unwrap(),
            CredentialAudience::ZaiStandard,
        ))
        .resolve_immediate(AuthContext::new(openrouter.endpoint()))
        .is_err()
    );
}

#[test]
fn registered_provider_headers_are_owned_only_by_the_profile_layer() {
    let name = HeaderName::from_static("http-referer");
    let pipeline =
        HeaderPipeline::with_registered_headers([http::header::AUTHORIZATION], [name.clone()]);
    let layers = |source| {
        vec![
            HeaderLayer::new(
                HeaderSource::Protocol,
                vec![HeaderOperation::set(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                )],
            ),
            HeaderLayer::new(
                HeaderSource::Auth,
                vec![HeaderOperation::set_sensitive(
                    http::header::AUTHORIZATION,
                    HeaderValue::from_static("Bearer redacted"),
                )],
            ),
            HeaderLayer::new(
                source,
                vec![HeaderOperation::set(
                    name.clone(),
                    HeaderValue::from_static("https://example.invalid"),
                )],
            ),
        ]
    };
    assert!(pipeline.resolve(layers(HeaderSource::Provider)).is_ok());
    for source in [
        HeaderSource::Model,
        HeaderSource::DynamicPolicy,
        HeaderSource::Request,
        HeaderSource::ClientIdentity,
    ] {
        assert!(pipeline.resolve(layers(source)).is_err());
    }
}

#[test]
fn redirects_reject_query_userinfo_fragment_cross_origin_and_scheme_change() {
    let runtime = OpenRouterProfile::from_api_key(HARDENING_CANARY)
        .unwrap()
        .build()
        .unwrap();
    let endpoint = runtime.endpoint();
    for target in [
        "https://openrouter.ai/api/v1/next?token=value",
        "https://user@openrouter.ai/api/v1/next",
        "https://openrouter.ai/api/v1/next#fragment",
        "https://example.com/api/v1/next",
        "http://openrouter.ai/api/v1/next",
    ] {
        assert!(
            RedirectPolicy::SameOrigin
                .validate_hop(
                    endpoint,
                    &Url::parse(target).unwrap(),
                    &CredentialAudience::OpenRouterApi,
                )
                .is_err()
        );
    }
}

#[test]
fn canary_workflow_is_exact_candidate_value_free_and_secret_scoped() {
    let workflow = include_str!("../.github/workflows/canary.yml");
    assert!(!workflow.contains("pull_request_target"));
    assert!(workflow.contains("permissions:\n  contents: read"));
    assert!(workflow.contains("environment: provider-canary"));
    assert!(workflow.contains("ref: ${{ inputs.subject_commit }}"));
    assert!(workflow.contains("test \"$(git rev-parse HEAD)\""));
    assert!(workflow.contains("philo/provider-canary-result"));
    assert!(workflow.contains("content_values_recorded:false"));
    assert!(workflow.contains("stable-blocker"));
    assert!(!workflow.contains("docs/apikey.md"));
    for secret in [
        "secrets.OPENAI_API_KEY",
        "secrets.ANTHROPIC_API_KEY",
        "secrets.OPENROUTER_API_KEY",
        "secrets.DEEPSEEK_API_KEY",
        "secrets.ZAI_API_KEY",
        "secrets.ZAI_CODING_API_KEY",
    ] {
        assert_eq!(workflow.matches(secret).count(), 1);
    }
    assert!(workflow.contains("custom-openrouter-definition"));
    assert!(workflow.contains("custom-zai-anthropic-definition"));
    assert!(workflow.contains("nvidia/nemotron-3-ultra-550b-a55b:free"));
    assert!(workflow.contains("glm-4.7-flash"));
    assert!(workflow.contains("--test custom_provider_online_smoke"));
    assert!(workflow.contains("cargo test --all-features --test anthropic_smoke -- --ignored"));
    assert!(workflow.contains("cargo test --all-features --test openai_smoke -- --nocapture"));
}
