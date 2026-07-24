//! P3-008 endpoint, deployment, model mapping, and URL policy contracts.

use std::collections::BTreeMap;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::provider::TestOnlyProfile;
use philo::provider::endpoint::{resolve_official, resolve_official_for, resolve_test_only};
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{
    CapabilityStatus, CatalogCapabilities, CatalogSource, CatalogSourceId, CompatPatch,
    CredentialAudience, DeploymentId, EndpointConfig, EndpointPathVariable, EndpointQuery,
    EndpointQueryAction, EndpointQuerySource, EndpointTemplate, EndpointValues, GenerateRequest,
    LlmClient, Message, ModelBodyWireFormat, ModelCatalog, ModelEntry, ModelId, ModelKey,
    ModelLimits, ModelRef, PolicySource, ProductId, ProtocolId, ProviderId, ProviderModelId,
    QueryMergeRule, ReasoningEffortSupport, RedirectPolicy, SupportStatus, WireModelValue,
};
use proptest::prelude::*;
use url::Url;

const PORT: u16 = 41_994;
const BASE: &str = "http://127.0.0.1:41994/proxy?api-version=old";

fn source() -> CatalogSource {
    CatalogSource::new(
        CatalogSourceId::new("p3-008-fixture").unwrap(),
        "2026-07-24",
        None::<String>,
    )
    .unwrap()
}

fn entry(deployment: &str) -> ModelEntry {
    ModelEntry {
        key: ModelKey {
            provider_id: ProviderId::new("test-only").unwrap(),
            product_id: ProductId::new("chat-completions").unwrap(),
            domain_model_id: ModelId::new("domain-model").unwrap(),
        },
        provider_model_id: ProviderModelId::new("provider/model").unwrap(),
        deployment_id: Some(DeploymentId::new(deployment).unwrap()),
        wire_model_value: WireModelValue::new("wire-model").unwrap(),
        display_name: "Endpoint Mapping Contract".to_owned(),
        protocol_id: ProtocolId::new("openai-chat-completions").unwrap(),
        capabilities: CatalogCapabilities {
            function_tools: CapabilityStatus::Unknown,
            tool_choice_required: CapabilityStatus::Unknown,
            tool_choice_specific: CapabilityStatus::Unknown,
            parallel_tool_calls: CapabilityStatus::Unknown,
            strict_tools: CapabilityStatus::Unknown,
            vision_input: CapabilityStatus::Unknown,
            image_detail_original: CapabilityStatus::Unknown,
            response_format_json_object: CapabilityStatus::Unknown,
            response_format_json_schema: CapabilityStatus::Unknown,
            reasoning_efforts: ReasoningEffortSupport::Unknown,
        },
        limits: ModelLimits::default(),
        default_max_output_tokens: None,
        compat_overrides: CompatPatch::from_source(PolicySource::ModelProfile)
            .with_model_body(ModelBodyWireFormat::Omit),
        pricing: None,
        source: source(),
        support_status: SupportStatus::Experimental,
        provenance: BTreeMap::new(),
    }
}

fn query() -> EndpointQuery {
    EndpointQuery::new()
        .with_api_version("2026-07-01", EndpointQuerySource::DeploymentMapping)
        .unwrap()
        .with_set(
            "feature",
            "stream",
            QueryMergeRule::RejectExisting,
            EndpointQuerySource::ProductProfile,
        )
        .unwrap()
}

fn template_config() -> EndpointConfig {
    EndpointConfig::base_and_template(
        BASE,
        EndpointTemplate::parse(
            "deployments/{deployment}/models/{provider_model}/chat/completions",
        )
        .unwrap(),
        query(),
    )
    .unwrap()
}

fn success() -> MockResponse {
    MockResponse::new(
        StatusCode::OK,
        HeaderMap::from_iter([(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"))]),
        vec![MockBodyItem::chunk(Bytes::from_static(
            b"data: {\"id\":\"mapped\",\"model\":\"wire-model\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ))],
    )
}

proptest! {
    #[test]
    fn base_path_prefix_is_preserved_for_every_join_shape(
        trailing_base in any::<bool>(),
        leading_path in any::<bool>(),
    ) {
        let base = if trailing_base {
            "https://api.openai.com/proxy/v1/"
        } else {
            "https://api.openai.com/proxy/v1"
        };
        let path = if leading_path { "/chat/completions" } else { "chat/completions" };
        let resolved = resolve_official(&EndpointConfig::base_and_path(base, path).unwrap()).unwrap();
        prop_assert_eq!(resolved.url().path(), "/proxy/v1/chat/completions");
    }
}

#[test]
fn deployment_variables_are_segment_encoded_and_cannot_inject_path_or_query() {
    let product = ProductId::new("deployment-chat").unwrap();
    let provider_model = ProviderModelId::new("model/../?admin=true#fragment").unwrap();
    let deployment = DeploymentId::new("tenant/../?x=1#fragment").unwrap();
    let values = EndpointValues::new(&product, &provider_model, Some(&deployment));
    let config = EndpointConfig::base_and_template(
        "https://api.openai.com/proxy",
        EndpointTemplate::parse("deployments/{deployment}/models/{provider_model}").unwrap(),
        EndpointQuery::new(),
    )
    .unwrap();
    let resolved = resolve_official_for(&config, values).unwrap();

    assert_eq!(
        resolved.url().path(),
        "/proxy/deployments/tenant%2F%2E%2E%2F%3Fx%3D1%23fragment/models/model%2F%2E%2E%2F%3Fadmin%3Dtrue%23fragment"
    );
    assert!(resolved.url().query().is_none());
    assert!(resolved.url().fragment().is_none());
    assert_eq!(
        resolved.diagnostics().path_variables(),
        &[
            EndpointPathVariable::Deployment,
            EndpointPathVariable::ProviderModel
        ]
    );
}

#[test]
fn api_version_query_merge_is_deterministic_and_value_free_in_diagnostics() {
    let product = ProductId::new("deployment-chat").unwrap();
    let provider_model = ProviderModelId::new("provider-model").unwrap();
    let deployment = DeploymentId::new("deployment").unwrap();
    let values = EndpointValues::new(&product, &provider_model, Some(&deployment));
    let config = EndpointConfig::base_and_template(
        "https://api.openai.com/prefix?api-version=old",
        EndpointTemplate::parse("deployments/{deployment}").unwrap(),
        query(),
    )
    .unwrap();
    let config_debug = format!("{config:?}");
    assert!(!config_debug.contains("2026-07-01"));
    assert!(!config_debug.contains("stream"));
    assert!(!config_debug.contains("old"));
    let resolved = resolve_official_for(&config, values).unwrap();

    assert_eq!(
        resolved.url().query(),
        Some("api-version=2026-07-01&feature=stream")
    );
    assert_eq!(resolved.diagnostics().query().len(), 2);
    assert_eq!(
        resolved.diagnostics().query()[0].action(),
        EndpointQueryAction::Set
    );
    let debug = format!("{resolved:?}");
    assert!(!debug.contains("2026-07-01"));
    assert!(!debug.contains("stream"));
}

#[tokio::test]
async fn compat_explicitly_controls_model_body_and_catalog_controls_endpoint() {
    let catalog = ModelCatalog::from_entries([entry("tenant/blue")]).unwrap();
    let runtime =
        TestOnlyProfile::localhost(&format!("http://127.0.0.1:{PORT}/bootstrap"), "test-key")
            .unwrap()
            .with_endpoint_config(template_config())
            .with_catalog(catalog)
            .build()
            .unwrap();
    let mock = MockTransport::scripted([MockExchange::response(success())]);
    let request = GenerateRequest::new(
        ModelRef::new("test-only", "domain-model").unwrap(),
        vec![Message::user("hello")],
    );
    LlmClient::new(runtime, mock.clone())
        .complete(request)
        .await
        .unwrap();

    let captured = mock.captured_requests();
    let request = &captured[0];
    assert_eq!(
        request.endpoint().url().path(),
        "/proxy/deployments/tenant%2Fblue/models/provider%2Fmodel/chat/completions"
    );
    assert_eq!(
        request.endpoint().url().query(),
        Some("api-version=2026-07-01&feature=stream")
    );
    let body: serde_json::Value = serde_json::from_slice(request.body()).unwrap();
    assert!(body.get("model").is_none());
}

#[test]
fn userinfo_sensitive_query_private_network_and_idn_fail_closed() {
    for value in [
        "https://user:pass@api.openai.com/v1/chat/completions",
        "https://127.0.0.1/v1/chat/completions",
        "https://10.0.0.1/v1/chat/completions",
        "https://169.254.1.1/v1/chat/completions",
        "https://[::1]/v1/chat/completions",
        "https://xn--bcher-kva.example/v1/chat/completions",
    ] {
        assert!(resolve_official(&EndpointConfig::absolute(value).unwrap()).is_err());
    }
    assert!(
        EndpointQuery::new()
            .with_set(
                "api-key",
                "secret",
                QueryMergeRule::Override,
                EndpointQuerySource::ProductProfile,
            )
            .is_err()
    );
    assert!(
        resolve_test_only(
            &EndpointConfig::absolute("http://169.254.1.1/chat/completions").unwrap()
        )
        .is_err()
    );
}

#[test]
fn credentials_are_revalidated_for_every_redirect_hop() {
    let endpoint = resolve_official(
        &EndpointConfig::absolute("https://api.openai.com/v1/chat/completions").unwrap(),
    )
    .unwrap();
    let same = Url::parse("https://api.openai.com/v1/other").unwrap();
    let resolved = RedirectPolicy::SameOrigin
        .validate_hop(&endpoint, &same, &CredentialAudience::OfficialOpenAi)
        .unwrap();
    assert_eq!(resolved.origin(), endpoint.origin());

    let foreign = Url::parse("https://example.com/v1/other").unwrap();
    assert!(
        RedirectPolicy::SameOrigin
            .validate_hop(&endpoint, &foreign, &CredentialAudience::OfficialOpenAi)
            .is_err()
    );
}

#[test]
fn official_and_test_only_endpoint_contracts_are_unchanged() {
    let official = resolve_official(
        &EndpointConfig::base_and_path("https://api.openai.com/v1/", "/chat/completions").unwrap(),
    )
    .unwrap();
    assert_eq!(
        official.url().as_str(),
        "https://api.openai.com/v1/chat/completions"
    );

    let local = resolve_test_only(
        &EndpointConfig::absolute("http://127.0.0.1:41994/v1/chat/completions").unwrap(),
    )
    .unwrap();
    assert_eq!(local.origin().host(), "127.0.0.1");
}
