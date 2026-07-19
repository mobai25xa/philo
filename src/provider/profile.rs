//! Declarative official and test-only provider profiles.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;

use crate::domain::{ProtocolId, ProviderId};
use crate::error::LlmError;

use super::auth::{ApiKey, BearerCredential, ClientIdentity};
use super::capability::{ProtocolDialect, ProviderCapabilities, ProviderTransportOptions};
use super::endpoint::{CredentialAudience, EndpointConfig, resolve_test_only};
use super::headers::HeaderOperation;
use super::runtime::ProviderRuntime;

/// Declarative provider configuration validated into a [`ProviderRuntime`].
#[derive(Clone)]
pub struct ProviderProfile {
    pub(super) provider_id: ProviderId,
    pub(super) protocol_id: ProtocolId,
    pub(super) endpoint: EndpointConfig,
    pub(super) audience: CredentialAudience,
    pub(super) credential: BearerCredential,
    pub(super) client_identity: ClientIdentity,
    pub(super) provider_headers: Vec<HeaderOperation>,
    pub(super) model_headers: Vec<HeaderOperation>,
    pub(super) capabilities: ProviderCapabilities,
    pub(super) dialect: ProtocolDialect,
    pub(super) transport: ProviderTransportOptions,
    pub(super) test_only: bool,
}

impl ProviderProfile {
    /// Returns provider identifier.
    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    /// Returns protocol identifier.
    pub fn protocol_id(&self) -> &ProtocolId {
        &self.protocol_id
    }

    /// Returns declared capabilities.
    pub fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
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
            .field("protocol_id", &self.protocol_id)
            .field("endpoint", &self.endpoint)
            .field("audience", &self.audience)
            .field("credential", &"[REDACTED]")
            .field("client_identity", &self.client_identity)
            .field("capabilities", &self.capabilities)
            .field("dialect", &self.dialect)
            .field("transport", &self.transport)
            .field("test_only", &self.test_only)
            .finish_non_exhaustive()
    }
}

/// Stable phase-one official `OpenAI` profile constructor.
#[derive(Clone, Debug)]
pub struct OfficialOpenAiProfile {
    key: ApiKey,
    client_identity: ClientIdentity,
}

impl OfficialOpenAiProfile {
    /// Creates the official profile with the default philo identity.
    pub fn new(key: ApiKey) -> Self {
        Self {
            key,
            client_identity: ClientIdentity::default(),
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

    /// Produces the declarative profile.
    pub fn profile(self) -> Result<ProviderProfile, LlmError> {
        let provider_id = ProviderId::new("official-openai")?;
        let protocol_id = ProtocolId::new("openai-chat-completions")?;
        let endpoint =
            EndpointConfig::base_and_path("https://api.openai.com/v1", "/chat/completions")?;
        let audience = CredentialAudience::OfficialOpenAi;
        let credential = BearerCredential::new(self.key, audience.clone());
        Ok(ProviderProfile {
            provider_id,
            protocol_id,
            endpoint,
            audience,
            credential,
            client_identity: self.client_identity,
            provider_headers: Vec::new(),
            model_headers: Vec::new(),
            capabilities: ProviderCapabilities::official_openai(),
            dialect: ProtocolDialect::OpenAiChatCompletions,
            transport: ProviderTransportOptions::secure_defaults(),
            test_only: false,
        })
    }

    /// Builds the immutable runtime.
    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        ProviderRuntime::build(self.profile()?)
    }
}

/// Explicit localhost-only profile for offline tests.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct TestOnlyProfile {
    profile: ProviderProfile,
}

impl TestOnlyProfile {
    /// Creates a test-only profile restricted to the exact resolved loopback origin.
    pub fn localhost(endpoint: &str, key: impl Into<String>) -> Result<Self, LlmError> {
        let endpoint = EndpointConfig::absolute(endpoint)?;
        let resolved = resolve_test_only(&endpoint)?;
        let audience = CredentialAudience::TestOnlyExactOrigin(resolved.origin().clone());
        let credential = BearerCredential::new(ApiKey::new(key)?, audience.clone());
        Ok(Self {
            profile: ProviderProfile {
                provider_id: ProviderId::new("test-only")?,
                protocol_id: ProtocolId::new("openai-chat-completions")?,
                endpoint,
                audience,
                credential,
                client_identity: ClientIdentity::default(),
                provider_headers: Vec::new(),
                model_headers: Vec::new(),
                capabilities: ProviderCapabilities::official_openai(),
                dialect: ProtocolDialect::OpenAiChatCompletions,
                transport: ProviderTransportOptions::secure_defaults(),
                test_only: true,
            },
        })
    }

    /// Builds the test runtime.
    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        ProviderRuntime::build(self.profile)
    }
}
