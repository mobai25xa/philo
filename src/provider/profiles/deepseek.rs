#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::sync::Arc;

use crate::domain::{ModelId, ProtocolId, ProviderId};
use crate::error::LlmError;

use super::super::auth::{ApiKey, AuthProvider, BearerAuth, BearerCredential, ClientIdentity};
use super::super::capability::ProviderCapabilities;
use super::super::catalog::ProductId;
use super::super::compat::MaxOutputTokensWireFormat;
use super::super::definition::{AuthScheme, ProviderDefinition};
use super::super::endpoint::CredentialAudience;
use super::super::endpoint::EndpointConfig;
use super::super::headers::DynamicHeaderPolicy;
use super::super::profile::ProviderProfile;
use super::super::runtime::ProviderRuntime;
use super::common::{compatible_deployment, exact_model_catalog, provider_patch};

/// Experimental built-in `DeepSeek` OpenAI-format Chat preset.
#[derive(Clone, Debug)]
pub struct DeepSeekProfile {
    auth: Arc<dyn AuthProvider>,
    client_identity: ClientIdentity,
    dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
}

impl DeepSeekProfile {
    /// Creates the preset with a Bearer credential bound to `DeepSeek` only.
    pub fn new(key: ApiKey) -> Self {
        let credential = BearerCredential::new(key, CredentialAudience::DeepSeekApi);
        Self {
            auth: Arc::new(BearerAuth::new(credential)),
            client_identity: ClientIdentity::default(),
            dynamic_header_policy: None,
        }
    }

    /// Creates the preset directly from an API key string.
    pub fn from_api_key(key: impl Into<String>) -> Result<Self, LlmError> {
        Ok(Self::new(ApiKey::new(key)?))
    }

    #[must_use]
    /// Replaces the truthful client identity.
    pub fn with_client_identity(mut self, identity: ClientIdentity) -> Self {
        self.client_identity = identity;
        self
    }

    #[must_use]
    /// Replaces Bearer authentication with an extensible provider.
    pub fn with_auth_provider<A: AuthProvider + 'static>(mut self, auth: A) -> Self {
        self.auth = Arc::new(auth);
        self
    }

    #[must_use]
    /// Installs a controlled value-free dynamic header policy.
    pub fn with_dynamic_header_policy(mut self, policy: DynamicHeaderPolicy) -> Self {
        self.dynamic_header_policy = Some(Arc::new(policy));
        self
    }

    /// Produces the declarative profile.
    pub fn profile(self) -> Result<ProviderProfile, LlmError> {
        let compat = provider_patch().with_max_output_tokens(MaxOutputTokensWireFormat::MaxTokens);
        let provider_id = ProviderId::new("deepseek")?;
        let product_id = ProductId::new("deepseek-chat-openai")?;
        let protocol_id = ProtocolId::new("openai-chat-completions")?;
        let model_id = ModelId::new("deepseek-v4-flash")?;
        let catalog = exact_model_catalog(
            provider_id.clone(),
            product_id.clone(),
            protocol_id,
            &model_id,
            "DeepSeek V4 Flash",
            "p3-001-deepseek-official-docs",
            compat.clone(),
        )?;
        let auth_scheme = AuthScheme::from_auth_provider(self.auth.as_ref())?;
        let definition = ProviderDefinition::openai_chat(provider_id, product_id)
            .with_endpoint(EndpointConfig::base_and_path(
                "https://api.deepseek.com",
                "/chat/completions",
            )?)
            .with_credential_binding(CredentialAudience::DeepSeekApi.into())
            .with_auth_scheme(auth_scheme)
            .with_shared_dynamic_header_policy(self.dynamic_header_policy)
            .with_capabilities(ProviderCapabilities::openai_compatible())
            .with_catalog(catalog)
            .with_provider_compat(compat)
            .build()?;
        definition.compile_resolved(compatible_deployment(self.auth, self.client_identity))
    }

    /// Builds the immutable runtime.
    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        ProviderRuntime::build(self.profile()?)
    }
}
