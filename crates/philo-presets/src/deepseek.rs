#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::sync::Arc;

use super::common::{PRESET_SOURCE, compatible_deployment, exact_model_catalog, provider_contract};
use philo::domain::{ModelId, ProtocolId, ProviderId};
use philo::error::LlmError;
use philo::provider::EnvironmentSecretResolver;
use philo::provider::auth::{ApiKey, AuthProvider, BearerAuth, BearerCredential, ClientIdentity};
use philo::provider::capability::ProviderCapabilities;
use philo::provider::catalog::ProductId;
use philo::provider::definition::{AuthScheme, ProviderDefinition};
use philo::provider::endpoint::{CredentialAudience, EndpointConfig};
use philo::provider::headers::DynamicHeaderPolicy;
use philo::provider::profile::ProviderProfile;
use philo::provider::protocol_contract::MaxOutputTokensWireFormat;
use philo::provider::runtime::ProviderRuntime;

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
        let compat = provider_contract()
            .with_max_output_tokens(MaxOutputTokensWireFormat::MaxTokens, PRESET_SOURCE);
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
            "deepseek-official-docs-reviewed-2026-07-23",
        )?;
        let auth_scheme = AuthScheme::from_auth_provider(self.auth.as_ref())?;
        let mut builder = ProviderDefinition::openai_chat(provider_id.clone(), product_id)
            .with_endpoint(EndpointConfig::base_and_path(
                "https://api.deepseek.com",
                "/chat/completions",
            )?)
            .with_credential_binding(CredentialAudience::DeepSeekApi.into())
            .with_auth_scheme(auth_scheme)
            .with_capabilities(ProviderCapabilities::conservative_chat_completions())
            .with_catalog(catalog)
            .with_openai_chat_compat(compat);
        if let Some(policy) = self.dynamic_header_policy {
            builder = builder.with_dynamic_header_policy(Arc::unwrap_or_clone(policy));
        }
        let definition = builder.build()?;
        let deployment = compatible_deployment(provider_id, self.auth, self.client_identity);
        definition.compile(&deployment, &EnvironmentSecretResolver)
    }

    /// Builds the immutable runtime.
    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        ProviderRuntime::build(self.profile()?)
    }
}
