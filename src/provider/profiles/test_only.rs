#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;

use crate::domain::{ProtocolId, ProviderId, ResourceLimits};
use crate::error::LlmError;
use crate::transport::SseConfig;

use super::super::auth::{ApiKey, BearerCredential, ClientIdentity};
use super::super::capability::{
    ModelCapabilityProfile, ProtocolDialect, ProviderCapabilities, ProviderTransportOptions,
};
use super::super::endpoint::{CredentialAudience, EndpointConfig, resolve_test_only};
use super::super::profile::{ProviderProfile, ProviderProfileParts};
use super::super::runtime::ProviderRuntime;

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
            profile: ProviderProfile::from_parts(ProviderProfileParts {
                provider_id: ProviderId::new("test-only")?,
                protocol_id: ProtocolId::new("openai-chat-completions")?,
                endpoint,
                audience,
                credential,
                client_identity: ClientIdentity::default(),
                provider_headers: Vec::new(),
                model_headers: Vec::new(),
                capabilities: ProviderCapabilities::official_openai(),
                model_capabilities: BTreeMap::new(),
                dialect: ProtocolDialect::OpenAiChatCompletions,
                transport: ProviderTransportOptions::secure_defaults(),
                resource_limits: ResourceLimits::official(),
                sse: SseConfig::default(),
                max_http_error_body_bytes: 16 * 1024,
                test_only: true,
            })?,
        })
    }

    /// Adds or replaces the declaration for one exact model identifier.
    #[must_use]
    pub fn with_model_capabilities(mut self, profile: ModelCapabilityProfile) -> Self {
        self.profile
            .model_capabilities
            .insert(profile.model().clone(), profile);
        self
    }

    /// Replaces SDK-local request and response safety ceilings for an offline test runtime.
    #[must_use]
    pub fn with_resource_limits(mut self, limits: ResourceLimits) -> Self {
        self.profile.resource_limits = limits;
        self
    }

    /// Replaces SSE framing ceilings for an offline test runtime.
    #[must_use]
    pub fn with_sse_config(mut self, config: SseConfig) -> Self {
        self.profile.sse = config;
        self
    }

    /// Replaces the bounded HTTP error-body prefix size for an offline test runtime.
    pub fn with_max_http_error_body_bytes(mut self, limit: usize) -> Result<Self, LlmError> {
        if limit == 0 {
            return Err(LlmError::Configuration(
                "HTTP error body limit must be positive".to_owned(),
            ));
        }
        self.profile.max_http_error_body_bytes = limit;
        Ok(self)
    }

    /// Builds the test runtime.
    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        ProviderRuntime::build(self.profile)
    }
}
