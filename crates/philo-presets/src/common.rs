use std::collections::BTreeMap;

use philo::domain::{
    CapabilityStatus, ModelId, PolicySource, ProtocolId, ProviderId, ResourceLimits,
};
use philo::error::LlmError;
use philo::provider::auth::{AuthProvider, ClientIdentity};
use philo::provider::catalog::{
    CatalogCapabilities, CatalogSource, CatalogSourceId, ModelCatalog, ModelEntry, ModelKey,
    ModelLimits, ProductId, ProviderModelId, WireModelValue,
};
use philo::provider::definition::ProviderDeploymentConfig;
use philo::provider::protocol_contract::CompatProfile;
use std::sync::Arc;

pub(super) fn compatible_deployment(
    provider_id: ProviderId,
    auth: Arc<dyn AuthProvider>,
    client_identity: ClientIdentity,
) -> ProviderDeploymentConfig {
    ProviderDeploymentConfig::with_auth_provider(provider_id, auth)
        .with_client_identity(client_identity)
        .with_resource_limits(ResourceLimits::official())
}

pub(super) fn exact_model_catalog(
    provider_id: ProviderId,
    product_id: ProductId,
    protocol_id: ProtocolId,
    model_id: &ModelId,
    display_name: &str,
    source_id: &str,
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
        pricing: None,
        source,
        support_status: CapabilityStatus::Supported,
        provenance: BTreeMap::new(),
    }])
}

pub(super) fn provider_contract() -> CompatProfile {
    CompatProfile::openai_chat_default()
}

/// Every preset deviation is declared by the provider profile layer.
pub(super) const PRESET_SOURCE: PolicySource = PolicySource::ProviderProfile;
