#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;

use crate::domain::{ModelId, ProtocolId, ProviderId, ResourceLimits};
use crate::error::LlmError;
use crate::transport::SseConfig;

use super::super::auth::{ApiKey, BearerCredential, ClientIdentity};
use super::super::capability::{
    ModelCapabilityProfile, ProtocolDialect, ProviderCapabilities, ProviderTransportOptions,
};
use super::super::endpoint::{CredentialAudience, EndpointConfig};
use super::super::profile::{ProviderProfile, ProviderProfileParts};
use super::super::runtime::ProviderRuntime;

/// Stable phase-one official `OpenAI` profile constructor.
#[derive(Clone, Debug)]
pub struct OfficialOpenAiProfile {
    key: ApiKey,
    client_identity: ClientIdentity,
    model_capabilities: BTreeMap<ModelId, ModelCapabilityProfile>,
    resource_limits: ResourceLimits,
    sse: SseConfig,
    max_http_error_body_bytes: usize,
}

impl OfficialOpenAiProfile {
    /// Creates the official profile with the default philo identity.
    pub fn new(key: ApiKey) -> Self {
        Self {
            key,
            client_identity: ClientIdentity::default(),
            model_capabilities: BTreeMap::new(),
            resource_limits: ResourceLimits::official(),
            sse: SseConfig::default(),
            max_http_error_body_bytes: 16 * 1024,
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

    /// Adds or replaces the declaration for one exact model identifier.
    #[must_use]
    pub fn with_model_capabilities(mut self, profile: ModelCapabilityProfile) -> Self {
        self.model_capabilities
            .insert(profile.model().clone(), profile);
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
            protocol_id: ProtocolId::new("openai-chat-completions")?,
            endpoint: EndpointConfig::base_and_path(
                "https://api.openai.com/v1",
                "/chat/completions",
            )?,
            credential: BearerCredential::new(self.key, audience.clone()),
            audience,
            client_identity: self.client_identity,
            provider_headers: Vec::new(),
            model_headers: Vec::new(),
            capabilities: ProviderCapabilities::official_openai(),
            model_capabilities: self.model_capabilities,
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
