use std::sync::Arc;

use crate::domain::{ModelId, PolicySource, ProtocolId, ProviderId, ResourceLimits};
use crate::error::LlmError;
use crate::transport::SseConfig;

use super::super::auth::{AuthProvider, ClientIdentity};
use std::collections::BTreeMap;

use super::super::catalog::{
    CatalogCapabilities, CatalogSource, CatalogSourceId, ModelCatalog, ModelEntry, ModelKey,
    ModelLimits, ProductId, ProviderModelId, SupportStatus, WireModelValue,
};
use super::super::compat::CompatPatch;
use super::super::definition::ResolvedProviderDeployment;

pub(super) fn compatible_deployment(
    auth: Arc<dyn AuthProvider>,
    client_identity: ClientIdentity,
) -> ResolvedProviderDeployment {
    ResolvedProviderDeployment::new(auth, client_identity)
        .with_resource_limits(ResourceLimits::official())
        .with_sse_config(SseConfig::default())
}

pub(super) fn exact_model_catalog(
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
