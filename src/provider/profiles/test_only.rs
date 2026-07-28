#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::{ProviderId, ResourceLimits};
use crate::error::LlmError;
use crate::transport::SseConfig;

use super::super::auth::{ApiKey, AuthProvider, BearerAuth, BearerCredential, ClientIdentity};
use super::super::capability::{
    ModelCapabilityProfile, ProviderCapabilities, ProviderTransportOptions,
};
use super::super::catalog::{ModelCatalog, ProductId};
use super::super::endpoint::{CredentialAudience, EndpointConfig, resolve_test_only};
use super::super::headers::DynamicHeaderPolicy;
use super::super::profile::{ProviderProfile, ProviderProfileParts};
use super::super::protocol_contract::{CompatProfile, ValidatedProtocolBinding};
use super::super::runtime::ProviderRuntime;
use super::super::{IdempotencyPolicy, RateLimitPolicy};

/// Explicit localhost-only profile for offline tests.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct TestOnlyProfile {
    profile: ProviderProfile,
    protocol_binding_error: bool,
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
                product_id: ProductId::new("chat-completions")?,
                protocol: ValidatedProtocolBinding::openai_chat(),
                endpoint,
                credential_binding: audience.into(),
                auth: Arc::new(BearerAuth::new(credential)),
                client_identity: ClientIdentity::default(),
                provider_headers: Vec::new(),
                model_headers: Vec::new(),
                dynamic_header_policy: None,
                capabilities: ProviderCapabilities::official_openai(),
                model_capabilities: BTreeMap::new(),
                catalog: ModelCatalog::default(),
                model_protocol_contracts: BTreeMap::new(),
                transport: ProviderTransportOptions::secure_defaults(),
                resource_limits: ResourceLimits::official(),
                sse: SseConfig::default(),
                max_http_error_body_bytes: 16 * 1024,
                rate_limit: RateLimitPolicy::standard_only(),
                idempotency: IdempotencyPolicy::standard_header(),
                test_only: true,
            })?,
            protocol_binding_error: false,
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

    /// Switches this localhost runtime to explicit Anthropic Messages protocol dispatch.
    #[doc(hidden)]
    #[must_use]
    pub fn with_anthropic_messages(mut self) -> Self {
        self.profile.product_id = ProductId::new("messages").expect("static product ID is valid");
        self.profile.protocol = ValidatedProtocolBinding::anthropic_messages();
        self.protocol_binding_error = false;
        self.profile.capabilities = ProviderCapabilities::official_anthropic();
        self
    }

    /// Replaces Bearer authentication for an offline test runtime.
    #[must_use]
    pub fn with_auth_provider<A>(mut self, auth: A) -> Self
    where
        A: AuthProvider + 'static,
    {
        self.profile.auth = Arc::new(auth);
        self
    }

    /// Installs a controlled value-free dynamic header policy for offline tests.
    #[must_use]
    pub fn with_dynamic_header_policy(mut self, policy: DynamicHeaderPolicy) -> Self {
        self.profile.dynamic_header_policy = Some(Arc::new(policy));
        self
    }

    /// Replaces the endpoint configuration while retaining the exact loopback audience.
    #[doc(hidden)]
    #[must_use]
    pub fn with_endpoint_config(mut self, endpoint: EndpointConfig) -> Self {
        self.profile.endpoint = endpoint;
        self
    }

    /// Replaces the exact-model catalog for an offline test runtime.
    #[must_use]
    pub fn with_catalog(mut self, catalog: ModelCatalog) -> Self {
        self.profile.catalog = catalog;
        self
    }

    /// Replaces the resolved compatibility contract for an offline test runtime.
    #[must_use]
    pub fn with_compat(mut self, compat: CompatProfile) -> Self {
        let contract = crate::provider::ResolvedProtocolContract::OpenAiChat(
            crate::provider::OpenAiChatContract::from_compat(compat),
        );
        match self.profile.protocol.clone().with_contract(contract) {
            Ok(protocol) => self.profile.protocol = protocol,
            Err(_) => self.protocol_binding_error = true,
        }
        self
    }

    /// Adds an exact-model resolved contract for an offline test runtime.
    #[must_use]
    pub fn with_model_compat(
        mut self,
        model: crate::domain::ModelId,
        compat: CompatProfile,
    ) -> Self {
        self.profile.model_protocol_contracts.insert(
            model,
            crate::provider::ResolvedProtocolContract::OpenAiChat(
                crate::provider::OpenAiChatContract::from_compat(compat),
            ),
        );
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

    /// Replaces typed response rate-limit declarations for offline tests.
    #[must_use]
    pub fn with_rate_limit_policy(mut self, policy: RateLimitPolicy) -> Self {
        self.profile.rate_limit = policy;
        self
    }

    /// Replaces request-idempotency capability for offline tests.
    #[must_use]
    pub fn with_idempotency_policy(mut self, policy: IdempotencyPolicy) -> Self {
        self.profile.idempotency = policy;
        self
    }

    /// Builds the test runtime.
    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        if self.protocol_binding_error {
            return Err(LlmError::Configuration(
                "test-only compatibility contract does not match protocol binding".to_owned(),
            ));
        }
        ProviderRuntime::build(self.profile)
    }
}
