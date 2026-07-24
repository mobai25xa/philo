//! P3-004 exact model catalog, provenance, and planner contracts.

use std::collections::BTreeMap;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use philo::provider::TestOnlyProfile;
use philo::transport::mock::{MockBodyItem, MockExchange, MockResponse, MockTransport};
use philo::{
    CapabilityStatus, CatalogCapabilities, CatalogSource, CatalogSourceId, CompatField,
    CompatPatch, GenerateRequest, GenerationOptions, LlmClient, MaxOutputTokensWireFormat, Message,
    ModelCatalog, ModelEntry, ModelId, ModelKey, ModelLimits, ModelRef, PolicySource, ProductId,
    ProtocolId, ProviderId, ProviderModelId, ReasoningEffortSupport, SupportStatus,
    ToolArgumentsCompat, WireModelValue, resolve_compat,
};

const ENDPOINT: &str = "http://127.0.0.1:41993/v1/chat/completions";

fn source(id: &str, expires: Option<&str>) -> CatalogSource {
    CatalogSource::new(
        CatalogSourceId::new(id).unwrap(),
        "2026-07-23",
        expires.map(str::to_owned),
    )
    .unwrap()
}

fn capabilities(function_tools: CapabilityStatus) -> CatalogCapabilities {
    CatalogCapabilities {
        function_tools,
        tool_choice_required: CapabilityStatus::Unknown,
        tool_choice_specific: CapabilityStatus::Unknown,
        parallel_tool_calls: CapabilityStatus::Unknown,
        strict_tools: CapabilityStatus::Unknown,
        vision_input: CapabilityStatus::Unknown,
        image_detail_original: CapabilityStatus::Unknown,
        response_format_json_object: CapabilityStatus::Unknown,
        response_format_json_schema: CapabilityStatus::Unknown,
        reasoning_efforts: ReasoningEffortSupport::Unknown,
    }
}

fn entry(source_id: &str, function_tools: CapabilityStatus, limits: ModelLimits) -> ModelEntry {
    ModelEntry {
        key: ModelKey {
            provider_id: ProviderId::new("test-only").unwrap(),
            product_id: ProductId::new("chat-completions").unwrap(),
            domain_model_id: ModelId::new("domain-model").unwrap(),
        },
        provider_model_id: ProviderModelId::new("provider-model").unwrap(),
        deployment_id: None,
        wire_model_value: WireModelValue::new("wire-model").unwrap(),
        display_name: "Catalog Contract Model".to_owned(),
        protocol_id: ProtocolId::new("openai-chat-completions").unwrap(),
        capabilities: capabilities(function_tools),
        limits,
        default_max_output_tokens: None,
        compat_overrides: CompatPatch::from_source(PolicySource::ModelProfile),
        pricing: None,
        source: source(source_id, None),
        support_status: SupportStatus::Experimental,
        provenance: BTreeMap::new(),
    }
}

fn success() -> MockResponse {
    MockResponse::new(
        StatusCode::OK,
        HeaderMap::from_iter([(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"))]),
        vec![MockBodyItem::chunk(Bytes::from_static(
            b"data: {\"id\":\"catalog\",\"model\":\"wire-model\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ))],
    )
}

#[test]
fn catalog_merge_is_deterministic_fieldwise_and_preserves_unknown() {
    let mut base_entry = entry(
        "base",
        CapabilityStatus::Supported,
        ModelLimits {
            max_messages: Some(10),
            max_tools: Some(4),
            ..ModelLimits::default()
        },
    );
    base_entry.support_status = SupportStatus::Supported;
    let base = ModelCatalog::from_entries([base_entry]).unwrap();
    let mut overlay_entry = entry(
        "overlay",
        CapabilityStatus::Unknown,
        ModelLimits {
            max_tools: Some(2),
            ..ModelLimits::default()
        },
    );
    overlay_entry.support_status = SupportStatus::Unknown;
    let overlay = ModelCatalog::from_entries([overlay_entry]).unwrap();
    let merged = ModelCatalog::merge(&[&base, &overlay]).unwrap();
    let resolved = merged.entries().next().unwrap();
    assert_eq!(
        resolved.capabilities.function_tools,
        CapabilityStatus::Supported
    );
    assert_eq!(resolved.limits.max_messages, Some(10));
    assert_eq!(resolved.limits.max_tools, Some(2));
    assert_eq!(resolved.pricing, None);
    assert_eq!(resolved.support_status, SupportStatus::Supported);
    assert_eq!(resolved.source.id().as_str(), "overlay");
    assert_eq!(
        resolved
            .field_source("limits.max_messages")
            .unwrap()
            .id()
            .as_str(),
        "base"
    );
    assert_eq!(
        resolved
            .field_source("limits.max_tools")
            .unwrap()
            .id()
            .as_str(),
        "overlay"
    );
    assert_eq!(
        resolved
            .field_source("capabilities.function_tools")
            .unwrap()
            .id()
            .as_str(),
        "base"
    );
}

#[test]
fn stale_evidence_is_visible_without_rewriting_support_status() {
    let entry = ModelEntry {
        source: source("stale", Some("2026-07-24")),
        support_status: SupportStatus::Experimental,
        ..entry(
            "original",
            CapabilityStatus::Unknown,
            ModelLimits::default(),
        )
    };
    assert!(entry.source.is_stale_on("2026-07-25").unwrap());
    assert_eq!(entry.support_status, SupportStatus::Experimental);
}

#[test]
fn duplicate_and_cross_provider_catalog_entries_fail_closed() {
    let first = entry("one", CapabilityStatus::Unknown, ModelLimits::default());
    assert!(ModelCatalog::from_entries([first.clone(), first]).is_err());

    let original = entry(
        "original",
        CapabilityStatus::Unknown,
        ModelLimits::default(),
    );
    let mut catalog = ModelCatalog::from_entries([original.clone()]).unwrap();
    let mut replacement = original;
    replacement.source = source("replacement", None);
    assert!(catalog.insert(replacement).is_err());
    assert_eq!(
        catalog.entries().next().unwrap().source.id().as_str(),
        "original"
    );

    let foreign = ModelEntry {
        key: ModelKey {
            provider_id: ProviderId::new("foreign").unwrap(),
            product_id: ProductId::new("chat-completions").unwrap(),
            domain_model_id: ModelId::new("domain-model").unwrap(),
        },
        ..entry("foreign", CapabilityStatus::Unknown, ModelLimits::default())
    };
    let catalog = ModelCatalog::from_entries([foreign]).unwrap();
    assert!(
        TestOnlyProfile::localhost(ENDPOINT, "test-key")
            .unwrap()
            .with_catalog(catalog)
            .build()
            .is_err()
    );
}

#[test]
fn catalog_merge_preserves_each_compat_leaf_source() {
    let mut base_entry = entry("base", CapabilityStatus::Unknown, ModelLimits::default());
    base_entry.compat_overrides = CompatPatch::from_source(PolicySource::ProviderProfile)
        .with_max_output_tokens(MaxOutputTokensWireFormat::MaxTokens);
    let mut overlay_entry = entry("overlay", CapabilityStatus::Unknown, ModelLimits::default());
    overlay_entry.compat_overrides = CompatPatch::from_source(PolicySource::ModelProfile)
        .with_tool_arguments(ToolArgumentsCompat::StringOrObject);
    let base = ModelCatalog::from_entries([base_entry]).unwrap();
    let overlay = ModelCatalog::from_entries([overlay_entry]).unwrap();
    let merged = ModelCatalog::merge(&[&base, &overlay]).unwrap();
    let profile = resolve_compat(&[merged.entries().next().unwrap().compat_overrides.clone()]);

    assert_eq!(
        profile.source(CompatField::RequestMaxOutputTokens),
        PolicySource::ProviderProfile
    );
    assert_eq!(
        profile.source(CompatField::ResponseToolArguments),
        PolicySource::ModelProfile
    );
}

#[tokio::test]
async fn planner_enforces_exact_model_output_limit_before_transport() {
    let catalog = ModelCatalog::from_entries([entry(
        "limit",
        CapabilityStatus::Unknown,
        ModelLimits {
            max_output_tokens: Some(8),
            ..ModelLimits::default()
        },
    )])
    .unwrap();
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "test-key")
        .unwrap()
        .with_catalog(catalog)
        .build()
        .unwrap();
    let mock = MockTransport::default();
    let request = GenerateRequest::new(
        ModelRef::new("test-only", "domain-model").unwrap(),
        vec![Message::user("hello")],
    )
    .with_options(GenerationOptions::new().with_max_output_tokens(9));
    let error = LlmClient::new(runtime, mock.clone())
        .complete(request)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("max_output_tokens"));
    assert!(mock.captured_requests().is_empty());
}

#[tokio::test]
async fn catalog_compiles_wire_model_and_safe_defaults_into_the_call_plan() {
    let mut exact = entry(
        "wire",
        CapabilityStatus::Unknown,
        ModelLimits {
            max_output_tokens: Some(12),
            ..ModelLimits::default()
        },
    );
    exact.default_max_output_tokens = Some(7);
    let runtime = TestOnlyProfile::localhost(ENDPOINT, "test-key")
        .unwrap()
        .with_catalog(ModelCatalog::from_entries([exact]).unwrap())
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
    let body: serde_json::Value =
        serde_json::from_slice(mock.captured_requests()[0].body()).unwrap();
    assert_eq!(body["model"], "wire-model");
    assert_eq!(body["max_completion_tokens"], 7);
}

#[test]
fn model_provider_deployment_and_wire_ids_remain_distinct_types() {
    let entry = entry("ids", CapabilityStatus::Unknown, ModelLimits::default());
    assert_eq!(entry.key.domain_model_id.as_str(), "domain-model");
    assert_eq!(entry.provider_model_id.as_str(), "provider-model");
    assert_eq!(entry.wire_model_value.as_str(), "wire-model");
}
