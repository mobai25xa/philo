#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::sync::Arc;

use http::{HeaderName, HeaderValue};

use crate::domain::{
    CapabilityStatus, ModelId, PolicySource, ProtocolId, ProviderId, ReasoningEffortSupport,
    ResourceLimits,
};
use crate::error::LlmError;
use crate::transport::SseConfig;

use super::super::auth::{ApiKey, ApiKeyHeaderAuth, AuthProvider, ClientIdentity};
use super::super::capability::{
    ModelCapabilityProfile, ProtocolDialect, ProviderCapabilities, ProviderTransportOptions,
};
use super::super::catalog::{
    CatalogCapabilities, CatalogSource, CatalogSourceId, ModelCatalog, ModelEntry, ModelKey,
    ModelLimits, ProductId, ProviderModelId, SupportStatus, WireModelValue,
};
use super::super::compat::CompatPatch;
use super::super::endpoint::{CredentialAudience, EndpointConfig};
use super::super::headers::{DynamicHeaderPolicy, HeaderOperation};
use super::super::profile::{ProviderProfile, ProviderProfileParts};
use super::super::runtime::ProviderRuntime;
use super::super::{
    IdempotencyPolicy, RateLimitHeaderKind, RateLimitHeaderSpec, RateLimitPolicy, RateLimitUnit,
};

/// Fixed API version sent by the official Anthropic Messages profile.
pub const OFFICIAL_ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Official Anthropic Messages profile with a fixed origin, API version, and protocol.
#[derive(Clone, Debug)]
pub struct OfficialAnthropicProfile {
    auth: Arc<dyn AuthProvider>,
    client_identity: ClientIdentity,
    model_capabilities: BTreeMap<ModelId, ModelCapabilityProfile>,
    catalog: ModelCatalog,
    resource_limits: ResourceLimits,
    sse: SseConfig,
    max_http_error_body_bytes: usize,
    dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
}

impl OfficialAnthropicProfile {
    /// Creates the official profile with a protected `x-api-key` credential.
    pub fn new(key: ApiKey) -> Result<Self, LlmError> {
        let audience = CredentialAudience::OfficialAnthropic;
        let auth = ApiKeyHeaderAuth::new(HeaderName::from_static("x-api-key"), key, audience)?;
        Ok(Self {
            auth: Arc::new(auth),
            client_identity: ClientIdentity::default(),
            model_capabilities: BTreeMap::new(),
            catalog: official_catalog()?,
            resource_limits: ResourceLimits::builder()
                .with_max_request_body_bytes(32 * 1024 * 1024)
                .build()?,
            sse: SseConfig::default(),
            max_http_error_body_bytes: 16 * 1024,
            dynamic_header_policy: None,
        })
    }

    /// Creates the official profile directly from an API key string.
    pub fn from_api_key(key: impl Into<String>) -> Result<Self, LlmError> {
        Self::new(ApiKey::new(key)?)
    }

    /// Replaces the truthful client identity.
    #[must_use]
    pub fn with_client_identity(mut self, identity: ClientIdentity) -> Self {
        self.client_identity = identity;
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
        let version = HeaderName::from_static("anthropic-version");
        let beta = HeaderName::from_static("anthropic-beta");
        let rate_limit = RateLimitPolicy::standard_only()
            .with_header(RateLimitHeaderSpec::new(
                HeaderName::from_static("anthropic-ratelimit-requests-remaining"),
                RateLimitHeaderKind::RemainingRequests,
            ))?
            .with_header(RateLimitHeaderSpec::new(
                HeaderName::from_static("anthropic-ratelimit-tokens-remaining"),
                RateLimitHeaderKind::RemainingUnits(RateLimitUnit::Tokens),
            ))?;
        let mut resource_limits = self.resource_limits;
        resource_limits.max_request_body_bytes =
            resource_limits.max_request_body_bytes.min(32 * 1024 * 1024);
        ProviderProfile::from_parts(ProviderProfileParts {
            provider_id: ProviderId::new("official-anthropic")?,
            product_id: ProductId::new("messages")?,
            protocol_id: ProtocolId::new("anthropic-messages")?,
            endpoint: EndpointConfig::base_and_path("https://api.anthropic.com/v1", "/messages")?,
            audience: CredentialAudience::OfficialAnthropic,
            auth: self.auth,
            client_identity: self.client_identity,
            provider_headers: vec![
                HeaderOperation::set(
                    version,
                    HeaderValue::from_static(OFFICIAL_ANTHROPIC_API_VERSION),
                ),
                HeaderOperation::remove(beta),
            ],
            model_headers: Vec::new(),
            dynamic_header_policy: self.dynamic_header_policy,
            capabilities: ProviderCapabilities::official_anthropic(),
            model_capabilities: self.model_capabilities,
            catalog: self.catalog,
            provider_compat: CompatPatch::from_source(PolicySource::ProviderProfile),
            model_compat: BTreeMap::new(),
            openrouter_routing: None,
            dialect: ProtocolDialect::AnthropicMessages,
            transport: ProviderTransportOptions::secure_defaults(),
            resource_limits,
            sse: self.sse,
            max_http_error_body_bytes: self.max_http_error_body_bytes,
            rate_limit,
            idempotency: IdempotencyPolicy::unknown(),
            test_only: false,
        })
    }

    /// Builds the immutable runtime.
    pub fn build(self) -> Result<ProviderRuntime, LlmError> {
        ProviderRuntime::build(self.profile()?)
    }
}

fn official_catalog() -> Result<ModelCatalog, LlmError> {
    let provider_id = ProviderId::new("official-anthropic")?;
    let product_id = ProductId::new("messages")?;
    let protocol_id = ProtocolId::new("anthropic-messages")?;
    let source = CatalogSource::new(
        CatalogSourceId::new("anthropic-models-ledger")?,
        "2026-07-25",
        Some("2026-08-25"),
    )?;
    let capabilities = CatalogCapabilities {
        function_tools: CapabilityStatus::Supported,
        tool_choice_required: CapabilityStatus::Supported,
        tool_choice_specific: CapabilityStatus::Supported,
        parallel_tool_calls: CapabilityStatus::Supported,
        strict_tools: CapabilityStatus::Supported,
        vision_input: CapabilityStatus::Supported,
        image_detail_original: CapabilityStatus::Unsupported,
        response_format_json_object: CapabilityStatus::Unsupported,
        response_format_json_schema: CapabilityStatus::Supported,
        reasoning_efforts: ReasoningEffortSupport::Unsupported,
        adaptive_thinking: CapabilityStatus::Supported,
        adaptive_thinking_effort: CapabilityStatus::Supported,
    };
    ModelCatalog::from_entries(
        ["claude-sonnet-5", "claude-opus-5"]
            .into_iter()
            .map(|model| {
                Ok(ModelEntry {
                    key: ModelKey {
                        provider_id: provider_id.clone(),
                        product_id: product_id.clone(),
                        domain_model_id: ModelId::new(model)?,
                    },
                    provider_model_id: ProviderModelId::new(model)?,
                    deployment_id: None,
                    wire_model_value: WireModelValue::new(model)?,
                    display_name: model.to_owned(),
                    protocol_id: protocol_id.clone(),
                    capabilities: capabilities.clone(),
                    limits: ModelLimits::default(),
                    default_max_output_tokens: Some(4096),
                    compat_overrides: CompatPatch::from_source(PolicySource::ModelProfile),
                    pricing: None,
                    source: source.clone(),
                    support_status: SupportStatus::Experimental,
                    provenance: BTreeMap::new(),
                })
            })
            .collect::<Result<Vec<_>, LlmError>>()?,
    )
}

#[cfg(test)]
mod tests {
    use http::{HeaderMap, HeaderValue};

    use super::*;

    #[test]
    fn official_runtime_freezes_protocol_endpoint_and_catalog() {
        let runtime = OfficialAnthropicProfile::from_api_key("anthropic-profile-test-key")
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(runtime.provider_id().as_str(), "official-anthropic");
        assert_eq!(runtime.product_id().as_str(), "messages");
        assert_eq!(runtime.protocol_id().as_str(), "anthropic-messages");
        assert_eq!(runtime.dialect(), ProtocolDialect::AnthropicMessages);
        assert_eq!(
            runtime.endpoint().url().as_str(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(runtime.catalog().entries().count(), 2);
    }

    #[test]
    fn official_headers_protect_auth_version_and_beta_owners() {
        let runtime = OfficialAnthropicProfile::from_api_key("anthropic-profile-test-key")
            .unwrap()
            .build()
            .unwrap();
        let resolved = runtime
            .resolve_headers(Vec::new(), &HeaderMap::new())
            .unwrap();
        assert!(resolved.headers().contains_key("x-api-key"));
        assert_eq!(
            resolved.headers().get("anthropic-version"),
            Some(&HeaderValue::from_static(OFFICIAL_ANTHROPIC_API_VERSION))
        );
        assert!(!resolved.headers().contains_key("anthropic-beta"));

        for name in ["x-api-key", "anthropic-version", "anthropic-beta"] {
            let mut request = HeaderMap::new();
            request.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_static("forbidden"),
            );
            assert!(runtime.resolve_headers(Vec::new(), &request).is_err());
        }
    }
}
