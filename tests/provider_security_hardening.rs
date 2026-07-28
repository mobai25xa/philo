//! P3-013 cross-module negative security contracts.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use http::{HeaderName, HeaderValue};
use philo::provider::auth::{ApiKey, AuthContext, AuthProvider, BearerAuth, BearerCredential};
use philo::provider::endpoint::{CredentialAudience, EndpointNetworkPolicy, RedirectPolicy};
use philo::provider::headers::{HeaderLayer, HeaderOperation, HeaderPipeline, HeaderSource};
use philo_presets::{OpenRouterProfile, ZaiCodingProfile, ZaiStandardProfile};
use url::Url;

const CANARY: &str = "security-hardening-credential-canary";

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
        ApiKey::new(CANARY).unwrap(),
        CredentialAudience::ZaiStandard,
    ));
    assert!(
        ZaiCodingProfile::from_api_key("bootstrap")
            .unwrap()
            .with_auth_provider(standard_auth)
            .build()
            .is_err()
    );

    let openrouter = OpenRouterProfile::from_api_key(CANARY)
        .unwrap()
        .build()
        .unwrap();
    let zai = ZaiStandardProfile::from_api_key(CANARY)
        .unwrap()
        .build()
        .unwrap();
    let openrouter_auth = BearerAuth::new(BearerCredential::new(
        ApiKey::new(CANARY).unwrap(),
        CredentialAudience::OpenRouterApi,
    ));
    assert!(
        openrouter_auth
            .resolve_immediate(AuthContext::new(zai.endpoint()))
            .is_err()
    );
    assert!(
        BearerAuth::new(BearerCredential::new(
            ApiKey::new(CANARY).unwrap(),
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
    let runtime = OpenRouterProfile::from_api_key(CANARY)
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
fn hosted_workflow_exposes_only_the_selected_secret_to_the_test_process() {
    let workflow = include_str!("../.github/workflows/provider-conformance.yml");
    assert!(!workflow.contains("pull_request_target"));
    assert!(workflow.contains("permissions:\n  contents: read"));
    assert!(workflow.contains("if: github.event_name == 'workflow_dispatch'"));
    assert!(workflow.contains("environment: provider-conformance"));
    assert!(workflow.contains("github.event_name == 'workflow_dispatch' && inputs.subject_commit"));
    assert!(!workflow.contains("env:\n      OPENAI_API_KEY"));
    assert!(!workflow.contains("secrets.OPENAI_API_KEY"));
    assert!(!workflow.contains("docs/apikey.md"));
    for secret in [
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
}

#[test]
fn official_anthropic_workflow_is_exact_candidate_and_value_free() {
    let workflow = include_str!("../.github/workflows/anthropic-smoke.yml");
    assert!(!workflow.contains("pull_request_target"));
    assert!(workflow.contains("permissions:\n  contents: read"));
    assert!(workflow.contains("environment: official-anthropic-smoke"));
    assert!(workflow.contains("ref: ${{ inputs.subject_commit }}"));
    assert!(workflow.contains("test \"$(git rev-parse HEAD)\""));
    assert!(workflow.contains("cargo test --all-features --test anthropic_smoke -- --ignored"));
    assert_eq!(workflow.matches("secrets.ANTHROPIC_API_KEY").count(), 1);
    assert!(workflow.contains(
        "Run URL: ${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}"
    ));
    assert!(workflow.contains("Prompt, output, API key, thinking, tool arguments"));
    assert!(!workflow.contains("docs/apikey.md"));
}
