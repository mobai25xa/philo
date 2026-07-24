//! Generic declarative provider profile contract.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::domain::{ModelId, ProtocolId, ProviderId, ResourceLimits};
use crate::error::LlmError;
use crate::transport::SseConfig;

use super::auth::{AuthProvider, ClientIdentity};
use super::capability::{
    ModelCapabilityProfile, ProtocolDialect, ProviderCapabilities, ProviderTransportOptions,
};
use super::catalog::{ModelCatalog, ProductId};
use super::compat::{CompatPatch, OpenRouterRoutingContract};
use super::endpoint::{CredentialAudience, EndpointConfig, resolve_official, resolve_test_only};
use super::headers::{DynamicHeaderPolicy, HeaderOperation};

/// Declarative provider configuration validated into a [`super::runtime::ProviderRuntime`].
#[derive(Clone)]
pub struct ProviderProfile {
    pub(super) provider_id: ProviderId,
    pub(super) product_id: ProductId,
    pub(super) protocol_id: ProtocolId,
    pub(super) endpoint: EndpointConfig,
    pub(super) audience: CredentialAudience,
    pub(super) auth: Arc<dyn AuthProvider>,
    pub(super) client_identity: ClientIdentity,
    pub(super) provider_headers: Vec<HeaderOperation>,
    pub(super) model_headers: Vec<HeaderOperation>,
    pub(super) dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
    pub(super) capabilities: ProviderCapabilities,
    pub(super) model_capabilities: BTreeMap<ModelId, ModelCapabilityProfile>,
    pub(super) catalog: ModelCatalog,
    pub(super) provider_compat: CompatPatch,
    pub(super) model_compat: BTreeMap<ModelId, CompatPatch>,
    pub(super) openrouter_routing: Option<OpenRouterRoutingContract>,
    pub(super) dialect: ProtocolDialect,
    pub(super) transport: ProviderTransportOptions,
    pub(super) resource_limits: ResourceLimits,
    pub(super) sse: SseConfig,
    pub(super) max_http_error_body_bytes: usize,
    pub(super) test_only: bool,
}

/// Typed construction input shared only inside the provider module tree.
pub(super) struct ProviderProfileParts {
    pub(super) provider_id: ProviderId,
    pub(super) product_id: ProductId,
    pub(super) protocol_id: ProtocolId,
    pub(super) endpoint: EndpointConfig,
    pub(super) audience: CredentialAudience,
    pub(super) auth: Arc<dyn AuthProvider>,
    pub(super) client_identity: ClientIdentity,
    pub(super) provider_headers: Vec<HeaderOperation>,
    pub(super) model_headers: Vec<HeaderOperation>,
    pub(super) dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
    pub(super) capabilities: ProviderCapabilities,
    pub(super) model_capabilities: BTreeMap<ModelId, ModelCapabilityProfile>,
    pub(super) catalog: ModelCatalog,
    pub(super) provider_compat: CompatPatch,
    pub(super) model_compat: BTreeMap<ModelId, CompatPatch>,
    pub(super) openrouter_routing: Option<OpenRouterRoutingContract>,
    pub(super) dialect: ProtocolDialect,
    pub(super) transport: ProviderTransportOptions,
    pub(super) resource_limits: ResourceLimits,
    pub(super) sse: SseConfig,
    pub(super) max_http_error_body_bytes: usize,
    pub(super) test_only: bool,
}

impl ProviderProfile {
    pub(super) fn from_parts(parts: ProviderProfileParts) -> Result<Self, LlmError> {
        if parts.max_http_error_body_bytes == 0 {
            return Err(LlmError::Configuration(
                "HTTP error body limit must be positive".to_owned(),
            ));
        }
        parts.capabilities.validate()?;
        for (model, declaration) in &parts.model_capabilities {
            if model != declaration.model() {
                return Err(LlmError::Configuration(
                    "model capability map key does not match its declaration".to_owned(),
                ));
            }
        }
        match parts.dialect {
            ProtocolDialect::OpenAiChatCompletions
                if parts.protocol_id.as_str() == "openai-chat-completions" => {}
            ProtocolDialect::OpenAiChatCompletions => {
                return Err(LlmError::Configuration(
                    "OpenAI Chat dialect requires the openai-chat-completions protocol id"
                        .to_owned(),
                ));
            }
        }
        let endpoint = if parts.test_only {
            resolve_test_only(&parts.endpoint)?
        } else {
            resolve_official(&parts.endpoint)?
        };
        parts.audience.validate(&endpoint)?;
        parts.auth.validate_endpoint(&endpoint)?;

        Ok(Self {
            provider_id: parts.provider_id,
            product_id: parts.product_id,
            protocol_id: parts.protocol_id,
            endpoint: parts.endpoint,
            audience: parts.audience,
            auth: parts.auth,
            client_identity: parts.client_identity,
            provider_headers: parts.provider_headers,
            model_headers: parts.model_headers,
            dynamic_header_policy: parts.dynamic_header_policy,
            capabilities: parts.capabilities,
            model_capabilities: parts.model_capabilities,
            catalog: parts.catalog,
            provider_compat: parts.provider_compat,
            model_compat: parts.model_compat,
            openrouter_routing: parts.openrouter_routing,
            dialect: parts.dialect,
            transport: parts.transport,
            resource_limits: parts.resource_limits,
            sse: parts.sse,
            max_http_error_body_bytes: parts.max_http_error_body_bytes,
            test_only: parts.test_only,
        })
    }

    /// Returns provider identifier.
    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    /// Returns protocol identifier.
    pub fn protocol_id(&self) -> &ProtocolId {
        &self.protocol_id
    }

    /// Returns exact provider product identifier.
    pub fn product_id(&self) -> &ProductId {
        &self.product_id
    }

    /// Returns the immutable model catalog snapshot.
    pub fn catalog(&self) -> &ModelCatalog {
        &self.catalog
    }

    /// Returns declared capabilities.
    pub fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    /// Returns dialect.
    pub fn dialect(&self) -> ProtocolDialect {
        self.dialect
    }

    /// Returns transport options.
    pub fn transport_options(&self) -> ProviderTransportOptions {
        self.transport
    }
}

impl fmt::Debug for ProviderProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderProfile")
            .field("provider_id", &self.provider_id)
            .field("product_id", &self.product_id)
            .field("protocol_id", &self.protocol_id)
            .field("endpoint", &self.endpoint)
            .field("audience", &self.audience)
            .field("auth", &"[REDACTED]")
            .field("client_identity", &self.client_identity)
            .field("capabilities", &self.capabilities)
            .field("dialect", &self.dialect)
            .field("transport", &self.transport)
            .field("test_only", &self.test_only)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ModelId;
    use crate::provider::auth::{ApiKey, BearerAuth, BearerCredential};

    fn official_parts(max_http_error_body_bytes: usize) -> ProviderProfileParts {
        let audience = CredentialAudience::OfficialOpenAi;
        ProviderProfileParts {
            provider_id: ProviderId::new("official-openai").unwrap(),
            product_id: ProductId::new("chat-completions").unwrap(),
            protocol_id: ProtocolId::new("openai-chat-completions").unwrap(),
            endpoint: EndpointConfig::base_and_path(
                "https://api.openai.com/v1",
                "/chat/completions",
            )
            .unwrap(),
            auth: Arc::new(BearerAuth::new(BearerCredential::new(
                ApiKey::new("profile-seam-test-key").unwrap(),
                audience.clone(),
            ))),
            audience,
            client_identity: ClientIdentity::default(),
            provider_headers: Vec::new(),
            model_headers: Vec::new(),
            dynamic_header_policy: None,
            capabilities: ProviderCapabilities::official_openai(),
            model_capabilities: BTreeMap::<ModelId, ModelCapabilityProfile>::new(),
            catalog: ModelCatalog::default(),
            provider_compat: CompatPatch::from_source(crate::domain::PolicySource::ProviderProfile),
            model_compat: BTreeMap::new(),
            openrouter_routing: None,
            dialect: ProtocolDialect::OpenAiChatCompletions,
            transport: ProviderTransportOptions::secure_defaults(),
            resource_limits: ResourceLimits::official(),
            sse: SseConfig::default(),
            max_http_error_body_bytes,
            test_only: false,
        }
    }

    #[test]
    fn internal_construction_seam_rejects_invalid_parts() {
        assert!(ProviderProfile::from_parts(official_parts(0)).is_err());

        let mut mismatched = official_parts(16 * 1024);
        mismatched.protocol_id = ProtocolId::new("wrong-protocol").unwrap();
        assert!(ProviderProfile::from_parts(mismatched).is_err());
    }
}
