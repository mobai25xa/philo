//! P3-004 exact model catalog, provenance, and planner contracts.

use std::collections::BTreeMap;

use philo::domain::ids::ProtocolId;
use philo::domain::request::{CapabilityStatus, ReasoningEffortSupport};
use philo::provider::TestOnlyProfile;
use philo::provider::catalog::{
    CatalogCapabilities, CatalogSource, CatalogSourceId, ModelCatalog, ModelEntry, ModelKey,
    ModelLimits, ProductId, ProviderModelId, WireModelValue,
};
use philo::{ModelId, ProviderId};

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
        adaptive_thinking: CapabilityStatus::Unknown,
        adaptive_thinking_effort: CapabilityStatus::Unknown,
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
        pricing: None,
        source: source(source_id, None),
        support_status: CapabilityStatus::Supported,
        provenance: BTreeMap::new(),
    }
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
    base_entry.support_status = CapabilityStatus::Supported;
    let base = ModelCatalog::from_entries([base_entry]).unwrap();
    let mut overlay_entry = entry(
        "overlay",
        CapabilityStatus::Unknown,
        ModelLimits {
            max_tools: Some(2),
            ..ModelLimits::default()
        },
    );
    overlay_entry.support_status = CapabilityStatus::Unknown;
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
    assert_eq!(resolved.support_status, CapabilityStatus::Supported);
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
fn capability_decision_and_stale_evidence_are_independent() {
    let entry = ModelEntry {
        source: source("stale", Some("2026-07-24")),
        support_status: CapabilityStatus::Supported,
        ..entry(
            "original",
            CapabilityStatus::Unknown,
            ModelLimits::default(),
        )
    };
    assert!(entry.source.is_stale_on("2026-07-25").unwrap());
    assert_eq!(entry.support_status, CapabilityStatus::Supported);
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
fn model_provider_deployment_and_wire_ids_remain_distinct_types() {
    let entry = entry("ids", CapabilityStatus::Unknown, ModelLimits::default());
    assert_eq!(entry.key.domain_model_id.as_str(), "domain-model");
    assert_eq!(entry.provider_model_id.as_str(), "provider-model");
    assert_eq!(entry.wire_model_value.as_str(), "wire-model");
}
