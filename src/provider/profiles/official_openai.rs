#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::{ModelId, ProviderId, ResourceLimits};
use crate::error::LlmError;
use crate::transport::SseConfig;

use super::super::auth::{ApiKey, AuthProvider, BearerAuth, BearerCredential, ClientIdentity};
use super::super::capability::{ModelCapabilityProfile, ProviderCapabilities};
use super::super::catalog::{ModelCatalog, ProductId};
use super::super::definition::{
    AuthScheme, ProviderDefinition, ProviderDefinitionBuilder, ResolvedProviderDeployment,
};
use super::super::endpoint::{CredentialAudience, EndpointConfig};
use super::super::headers::DynamicHeaderPolicy;
use super::super::profile::ProviderProfile;
use super::super::protocol_contract::CompatProfile;
use super::super::runtime::ProviderRuntime;

/// Stable official `OpenAI` profile constructor.
#[derive(Clone, Debug)]
pub struct OfficialOpenAiProfile {
    auth: Arc<dyn AuthProvider>,
    client_identity: ClientIdentity,
    model_capabilities: BTreeMap<ModelId, ModelCapabilityProfile>,
    catalog: ModelCatalog,
    provider_compat: CompatProfile,
    model_compat: BTreeMap<ModelId, CompatProfile>,
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
            provider_compat: CompatProfile::openai_chat_default(),
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

    /// Replaces the resolved provider-level compatibility contract.
    #[must_use]
    pub fn with_compat(mut self, compat: CompatProfile) -> Self {
        self.provider_compat = compat;
        self
    }

    /// Adds a resolved compatibility contract for one exact model.
    #[must_use]
    pub fn with_model_compat(mut self, model: ModelId, compat: CompatProfile) -> Self {
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

    /// Returns the secret-free official definition.
    ///
    /// This is the single construction input: pair it with a
    /// [`ProviderDeploymentConfig`](crate::provider::ProviderDeploymentConfig)
    /// that names the credential, and compile. The official origin, credential
    /// audience, and protocol contract are fixed here and cannot be widened.
    pub fn definition() -> Result<ProviderDefinition, LlmError> {
        official_openai_builder(AuthScheme::bearer(), None)?
            .with_catalog(ModelCatalog::default())
            .build()
    }

    /// Produces the declarative profile.
    pub fn profile(self) -> Result<ProviderProfile, LlmError> {
        let auth_scheme = AuthScheme::from_auth_provider(self.auth.as_ref())?;
        let mut builder = official_openai_builder(auth_scheme, self.dynamic_header_policy)?
            .with_catalog(self.catalog)
            .with_openai_chat_compat(self.provider_compat);
        for capability in self.model_capabilities.into_values() {
            builder = builder.with_model_capabilities(capability);
        }
        for (model, compat) in self.model_compat {
            builder = builder.with_model_openai_chat_compat(model, compat);
        }
        let deployment = ResolvedProviderDeployment::new(self.auth, self.client_identity)
            .with_resource_limits(self.resource_limits)
            .with_sse_config(self.sse)
            .with_max_http_error_body_bytes(self.max_http_error_body_bytes)?;
        builder.build()?.compile_resolved(deployment)
    }

    /// Builds the immutable runtime.
    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        ProviderRuntime::build(self.profile()?)
    }
}

/// The one place the official `OpenAI` identity, origin, and audience are fixed.
fn official_openai_builder(
    auth_scheme: AuthScheme,
    dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
) -> Result<ProviderDefinitionBuilder, LlmError> {
    Ok(ProviderDefinition::openai_chat(
        ProviderId::new("official-openai")?,
        ProductId::new("chat-completions")?,
    )
    .with_endpoint(EndpointConfig::base_and_path(
        "https://api.openai.com/v1",
        "/chat/completions",
    )?)
    .with_credential_binding(CredentialAudience::OfficialOpenAi.into())
    .with_auth_scheme(auth_scheme)
    .with_shared_dynamic_header_policy(dynamic_header_policy)
    .with_capabilities(ProviderCapabilities::official_openai()))
}
