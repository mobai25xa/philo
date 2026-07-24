//! Catalog model mapping compiled for endpoint and request resolution.

use crate::domain::ModelId;
use crate::provider::catalog::{
    DeploymentId, ModelEntry, ProductId, ProviderModelId, WireModelValue,
};

use super::EndpointValues;

/// Immutable model/deployment/wire mapping copied from one exact catalog entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedModelMapping {
    domain_model_id: ModelId,
    product_id: ProductId,
    provider_model_id: ProviderModelId,
    deployment_id: Option<DeploymentId>,
    wire_model_value: WireModelValue,
}

impl ResolvedModelMapping {
    /// Compiles a mapping from a validated exact catalog entry.
    #[must_use]
    pub fn from_entry(entry: &ModelEntry) -> Self {
        Self {
            domain_model_id: entry.key.domain_model_id.clone(),
            product_id: entry.key.product_id.clone(),
            provider_model_id: entry.provider_model_id.clone(),
            deployment_id: entry.deployment_id.clone(),
            wire_model_value: entry.wire_model_value.clone(),
        }
    }

    /// Returns the application-facing model identifier.
    pub fn domain_model_id(&self) -> &ModelId {
        &self.domain_model_id
    }

    /// Returns the provider product identifier.
    pub fn product_id(&self) -> &ProductId {
        &self.product_id
    }

    /// Returns the provider-owned model identifier.
    pub fn provider_model_id(&self) -> &ProviderModelId {
        &self.provider_model_id
    }

    /// Returns the optional deployment identifier.
    pub fn deployment_id(&self) -> Option<&DeploymentId> {
        self.deployment_id.as_ref()
    }

    /// Returns the model value selected for the request wire format.
    pub fn wire_model_value(&self) -> &WireModelValue {
        &self.wire_model_value
    }

    pub(crate) const fn endpoint_values(&self) -> EndpointValues<'_> {
        EndpointValues::new(
            &self.product_id,
            &self.provider_model_id,
            self.deployment_id.as_ref(),
        )
    }
}
