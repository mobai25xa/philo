#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::{ModelId, ProtocolId, ProviderId, ResourceLimits};
use crate::error::LlmError;
use crate::transport::SseConfig;

use super::super::auth::{ApiKey, AuthProvider, BearerAuth, BearerCredential, ClientIdentity};
use super::super::capability::{
    ModelCapabilityProfile, ProtocolDialect, ProviderCapabilities, ProviderTransportOptions,
};
use super::super::catalog::{ModelCatalog, ProductId};
use super::super::compat::CompatPatch;
use super::super::endpoint::{CredentialAudience, EndpointConfig};
use super::super::headers::DynamicHeaderPolicy;
use super::super::profile::{ProviderProfile, ProviderProfileParts};
use super::super::runtime::ProviderRuntime;

/// Stable phase-one official `OpenAI` profile constructor.
#[derive(Clone, Debug)]
pub struct OfficialOpenAiProfile {
    auth: Arc<dyn AuthProvider>,
    client_identity: ClientIdentity,
    model_capabilities: BTreeMap<ModelId, ModelCapabilityProfile>,
    catalog: ModelCatalog,
    provider_compat: CompatPatch,
    model_compat: BTreeMap<ModelId, CompatPatch>,
    resource_limits: ResourceLimits,
    sse: SseConfig,
    max_http_error_body_bytes: usize,
    dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
}

impl OfficialOpenAiProfile {
    /// Creates the official profile with the default philo identity.
    pub fn new(key: ApiKey) -> Self {
        let credential = BearerCredential::new(key, CredentialAudience::OfficialOpenAi);
        Self {
            auth: Arc::new(BearerAuth::new(credential)),
            client_identity: ClientIdentity::default(),
            model_capabilities: BTreeMap::new(),
            catalog: ModelCatalog::default(),
            provider_compat: CompatPatch::from_source(crate::domain::PolicySource::ProviderProfile),
            model_compat: BTreeMap::new(),
            resource_limits: ResourceLimits::official(),
            sse: SseConfig::default(),
            max_http_error_body_bytes: 16 * 1024,
            dynamic_header_policy: None,
        }
    }

    /// Creates the official profile directly from an API key string.
    pub fn from_api_key(key: impl Into<String>) -> Result<Self, LlmError> {
        Ok(Self::new(ApiKey::new(key)?))
    }

    /// Replaces the truthful client identity.
    #[must_use]
    pub fn with_client_identity(mut self, identity: ClientIdentity) -> Self {
        self.client_identity = identity;
        self
    }

    /// Replaces Bearer authentication with an extensible provider.
    #[must_use]
    pub fn with_auth_provider<A>(mut self, auth: A) -> Self
    where
        A: AuthProvider + 'static,
    {
        self.auth = Arc::new(auth);
        self
    }

    /// Installs a controlled value-free dynamic header policy.
    #[must_use]
    pub fn with_dynamic_header_policy(mut self, policy: DynamicHeaderPolicy) -> Self {
        self.dynamic_header_policy = Some(Arc::new(policy));
        self
    }

    /// Adds or replaces the declaration for one exact model identifier.
    #[must_use]
    pub fn with_model_capabilities(mut self, profile: ModelCapabilityProfile) -> Self {
        self.model_capabilities
            .insert(profile.model().clone(), profile);
        self
    }

    /// Replaces the immutable exact-model catalog.
    #[must_use]
    pub fn with_catalog(mut self, catalog: ModelCatalog) -> Self {
        self.catalog = catalog;
        self
    }

    /// Replaces provider-level typed compatibility overrides.
    #[must_use]
    pub fn with_compat(mut self, compat: CompatPatch) -> Self {
        self.provider_compat = compat;
        self
    }

    /// Adds exact-model typed compatibility overrides.
    #[must_use]
    pub fn with_model_compat(mut self, model: ModelId, compat: CompatPatch) -> Self {
        self.model_compat.insert(model, compat);
        self
    }

    /// Replaces SDK-local request and response safety ceilings.
    #[must_use]
    pub fn with_resource_limits(mut self, limits: ResourceLimits) -> Self {
        self.resource_limits = limits;
        self
    }

    /// Replaces Server-Sent Events framing ceilings.
    #[must_use]
    pub fn with_sse_config(mut self, config: SseConfig) -> Self {
        self.sse = config;
        self
    }

    /// Replaces the bounded HTTP error-body prefix size.
    pub fn with_max_http_error_body_bytes(mut self, limit: usize) -> Result<Self, LlmError> {
        if limit == 0 {
            return Err(LlmError::Configuration(
                "HTTP error body limit must be positive".to_owned(),
            ));
        }
        self.max_http_error_body_bytes = limit;
        Ok(self)
    }

    /// Produces the declarative profile.
    pub fn profile(self) -> Result<ProviderProfile, LlmError> {
        let audience = CredentialAudience::OfficialOpenAi;
        ProviderProfile::from_parts(ProviderProfileParts {
            provider_id: ProviderId::new("official-openai")?,
            product_id: ProductId::new("chat-completions")?,
            protocol_id: ProtocolId::new("openai-chat-completions")?,
            endpoint: EndpointConfig::base_and_path(
                "https://api.openai.com/v1",
                "/chat/completions",
            )?,
            auth: self.auth,
            audience,
            client_identity: self.client_identity,
            provider_headers: Vec::new(),
            model_headers: Vec::new(),
            dynamic_header_policy: self.dynamic_header_policy,
            capabilities: ProviderCapabilities::official_openai(),
            model_capabilities: self.model_capabilities,
            catalog: self.catalog,
            provider_compat: self.provider_compat,
            model_compat: self.model_compat,
            dialect: ProtocolDialect::OpenAiChatCompletions,
            transport: ProviderTransportOptions::secure_defaults(),
            resource_limits: self.resource_limits,
            sse: self.sse,
            max_http_error_body_bytes: self.max_http_error_body_bytes,
            test_only: false,
        })
    }

    /// Builds the immutable runtime.
    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        ProviderRuntime::build(self.profile()?)
    }
}
