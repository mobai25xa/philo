#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::sync::Arc;

use crate::error::LlmError;

use super::super::auth::{ApiKey, AuthProvider, BearerAuth, BearerCredential, ClientIdentity};
use super::super::compat::MaxOutputTokensWireFormat;
use super::super::endpoint::CredentialAudience;
use super::super::headers::DynamicHeaderPolicy;
use super::super::profile::ProviderProfile;
use super::super::runtime::ProviderRuntime;
use super::common::{CompatibleProfileParts, build_compatible_profile, provider_patch};

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
        build_compatible_profile(CompatibleProfileParts {
            provider: "deepseek",
            product: "deepseek-chat-openai",
            base_url: "https://api.deepseek.com",
            endpoint_path: "/chat/completions",
            audience: CredentialAudience::DeepSeekApi,
            auth: self.auth,
            client_identity: self.client_identity,
            provider_headers: Vec::new(),
            dynamic_header_policy: self.dynamic_header_policy,
            exact_model: "deepseek-v4-flash",
            display_name: "DeepSeek V4 Flash",
            catalog_source: "p3-001-deepseek-official-docs",
            provider_compat: compat,
            openrouter_routing: None,
        })
    }

    /// Builds the immutable runtime.
    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        ProviderRuntime::build(self.profile()?)
    }
}
