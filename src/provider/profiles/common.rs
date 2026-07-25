use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::{ModelId, PolicySource, ProtocolId, ProviderId, ResourceLimits};
use crate::error::LlmError;
use crate::transport::SseConfig;

use super::super::auth::{AuthProvider, ClientIdentity};
use super::super::capability::{
    ModelCapabilityProfile, ProtocolDialect, ProviderCapabilities, ProviderTransportOptions,
};
use super::super::catalog::{
    CatalogCapabilities, CatalogSource, CatalogSourceId, ModelCatalog, ModelEntry, ModelKey,
    ModelLimits, ProductId, ProviderModelId, SupportStatus, WireModelValue,
};
use super::super::compat::{CompatPatch, OpenRouterRoutingContract};
use super::super::endpoint::{CredentialAudience, EndpointConfig};
use super::super::headers::{DynamicHeaderPolicy, HeaderOperation};
use super::super::profile::{ProviderProfile, ProviderProfileParts};
use super::super::{IdempotencyPolicy, RateLimitPolicy};

pub(super) struct CompatibleProfileParts {
    pub provider: &'static str,
    pub product: &'static str,
    pub base_url: &'static str,
    pub endpoint_path: &'static str,
    pub audience: CredentialAudience,
    pub auth: Arc<dyn AuthProvider>,
    pub client_identity: ClientIdentity,
    pub provider_headers: Vec<HeaderOperation>,
    pub dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
    pub exact_model: &'static str,
    pub display_name: &'static str,
    pub catalog_source: &'static str,
    pub provider_compat: CompatPatch,
    pub openrouter_routing: Option<OpenRouterRoutingContract>,
}

pub(super) fn build_compatible_profile(
    parts: CompatibleProfileParts,
) -> Result<ProviderProfile, LlmError> {
    let provider_id = ProviderId::new(parts.provider)?;
    let product_id = ProductId::new(parts.product)?;
    let protocol_id = ProtocolId::new("openai-chat-completions")?;
    let model_id = ModelId::new(parts.exact_model)?;
    let catalog = exact_model_catalog(
        provider_id.clone(),
        product_id.clone(),
        protocol_id.clone(),
        &model_id,
        parts.display_name,
        parts.catalog_source,
        parts.provider_compat.clone(),
    )?;
    ProviderProfile::from_parts(ProviderProfileParts {
        provider_id,
        product_id,
        protocol_id,
        endpoint: EndpointConfig::base_and_path(parts.base_url, parts.endpoint_path)?,
        audience: parts.audience,
        auth: parts.auth,
        client_identity: parts.client_identity,
        provider_headers: parts.provider_headers,
        model_headers: Vec::new(),
        dynamic_header_policy: parts.dynamic_header_policy,
        capabilities: ProviderCapabilities::openai_compatible(),
        model_capabilities: BTreeMap::<ModelId, ModelCapabilityProfile>::new(),
        catalog,
        provider_compat: parts.provider_compat,
        model_compat: BTreeMap::new(),
        openrouter_routing: parts.openrouter_routing,
        dialect: ProtocolDialect::OpenAiChatCompletions,
        transport: ProviderTransportOptions::secure_defaults(),
        resource_limits: ResourceLimits::official(),
        sse: SseConfig::default(),
        max_http_error_body_bytes: 16 * 1024,
        rate_limit: RateLimitPolicy::standard_only(),
        idempotency: IdempotencyPolicy::unknown(),
        test_only: false,
    })
}

fn exact_model_catalog(
    provider_id: ProviderId,
    product_id: ProductId,
    protocol_id: ProtocolId,
    model_id: &ModelId,
    display_name: &str,
    source_id: &str,
    compat_overrides: CompatPatch,
) -> Result<ModelCatalog, LlmError> {
    let source = CatalogSource::new(
        CatalogSourceId::new(source_id)?,
        "2026-07-23",
        Some("2026-10-23"),
    )?;
    ModelCatalog::from_entries([ModelEntry {
        key: ModelKey {
            provider_id,
            product_id,
            domain_model_id: model_id.clone(),
        },
        provider_model_id: ProviderModelId::new(model_id.as_str())?,
        deployment_id: None,
        wire_model_value: WireModelValue::new(model_id.as_str())?,
        display_name: display_name.to_owned(),
        protocol_id,
        capabilities: CatalogCapabilities::default(),
        limits: ModelLimits::default(),
        default_max_output_tokens: None,
        compat_overrides,
        pricing: None,
        source,
        support_status: SupportStatus::Experimental,
        provenance: BTreeMap::new(),
    }])
}

pub(super) fn provider_patch() -> CompatPatch {
    CompatPatch::from_source(PolicySource::ProviderProfile)
}
