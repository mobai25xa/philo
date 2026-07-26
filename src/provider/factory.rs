//! Provider configuration snapshot to immutable runtime compilation.
#![allow(clippy::missing_errors_doc)]

use crate::domain::ProviderId;
use crate::error::LlmError;

use super::catalog::ProductId;
use super::config::{ConfigSourceId, FieldProvenance, ProviderConfigSnapshot, SecretResolver};
use super::detection::{
    DetectionExplanation, EndpointDetection, EndpointDetectionPolicy, EndpointDetector,
    NormalizedEndpointFacts,
};
use super::runtime::ProviderRuntime;
use super::{ProviderDefinition, ProviderDeploymentConfig};

/// Winning source in the frozen provider-selection precedence chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderSelectionSource {
    /// Explicit request selection.
    RequestExplicit,
    /// Provider carried by the exact model reference.
    ModelExplicit,
    /// Explicit provider configuration.
    ProviderExplicit,
    /// Explicit built-in profile selection.
    BuiltInProfile,
    /// Low-priority reviewed endpoint detection.
    EndpointDetection,
    /// No provider was selected; the caller remains at protocol default/unknown.
    ProtocolDefault,
}

/// Typed inputs considered by [`ProviderSelector`] in strict precedence order.
#[derive(Clone, Debug, Default)]
pub struct ProviderSelectionInput {
    request: Option<ProviderId>,
    model: Option<ProviderId>,
    provider: Option<ProviderId>,
    provider_source: Option<ConfigSourceId>,
    built_in_profile: Option<ProviderId>,
    endpoint: Option<NormalizedEndpointFacts>,
    detection_policy: EndpointDetectionPolicy,
}

impl ProviderSelectionInput {
    /// Creates empty input that resolves to protocol default/unknown.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets request-explicit provider selection.
    #[must_use]
    pub fn with_request_provider(mut self, provider: ProviderId) -> Self {
        self.request = Some(provider);
        self
    }

    /// Sets model-explicit provider selection.
    #[must_use]
    pub fn with_model_provider(mut self, provider: ProviderId) -> Self {
        self.model = Some(provider);
        self
    }

    /// Sets provider-explicit configuration selection.
    #[must_use]
    pub fn with_provider(mut self, provider: ProviderId) -> Self {
        self.provider = Some(provider);
        self.provider_source = None;
        self
    }

    /// Sets provider-explicit selection and preserves its value-free config source.
    #[must_use]
    pub fn with_provider_from_config(
        mut self,
        provider: ProviderId,
        provenance: &FieldProvenance,
    ) -> Self {
        self.provider = Some(provider);
        self.provider_source = Some(provenance.source().id().clone());
        self
    }

    /// Sets an explicitly selected built-in profile.
    #[must_use]
    pub fn with_built_in_profile(mut self, provider: ProviderId) -> Self {
        self.built_in_profile = Some(provider);
        self
    }

    /// Sets sanitized endpoint facts used only as the final provider fallback.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: NormalizedEndpointFacts) -> Self {
        self.endpoint = Some(endpoint);
        self
    }

    /// Enables or disables endpoint detection explicitly.
    #[must_use]
    pub const fn with_detection_policy(mut self, policy: EndpointDetectionPolicy) -> Self {
        self.detection_policy = policy;
        self
    }
}

/// Provider selection plus value-free provenance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSelection {
    provider_id: Option<ProviderId>,
    product_id: Option<ProductId>,
    source: ProviderSelectionSource,
    detection: Option<DetectionExplanation>,
    config_source: Option<ConfigSourceId>,
}

impl ProviderSelection {
    /// Returns the selected provider, or `None` for protocol default/unknown.
    #[must_use]
    pub const fn provider_id(&self) -> Option<&ProviderId> {
        self.provider_id.as_ref()
    }

    /// Returns the detected product when endpoint detection won.
    #[must_use]
    pub const fn product_id(&self) -> Option<&ProductId> {
        self.product_id.as_ref()
    }

    /// Returns the winning selection source.
    #[must_use]
    pub const fn source(&self) -> ProviderSelectionSource {
        self.source
    }

    /// Returns value-free detection explanation when detection was evaluated.
    #[must_use]
    pub const fn detection(&self) -> Option<&DetectionExplanation> {
        self.detection.as_ref()
    }

    /// Returns provider-field config provenance when that source won.
    #[must_use]
    pub const fn config_source(&self) -> Option<&ConfigSourceId> {
        self.config_source.as_ref()
    }
}

/// Sole authority that may adopt an endpoint detection suggestion.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProviderSelector;

impl ProviderSelector {
    /// Applies explicit precedence and, only if still unresolved, endpoint detection.
    #[must_use]
    pub fn select(input: &ProviderSelectionInput) -> ProviderSelection {
        for (provider, source) in [
            (&input.request, ProviderSelectionSource::RequestExplicit),
            (&input.model, ProviderSelectionSource::ModelExplicit),
            (&input.provider, ProviderSelectionSource::ProviderExplicit),
            (
                &input.built_in_profile,
                ProviderSelectionSource::BuiltInProfile,
            ),
        ] {
            if let Some(provider) = provider {
                return ProviderSelection {
                    provider_id: Some(provider.clone()),
                    product_id: None,
                    source,
                    detection: None,
                    config_source: (source == ProviderSelectionSource::ProviderExplicit)
                        .then(|| input.provider_source.clone())
                        .flatten(),
                };
            }
        }

        match EndpointDetector::detect_with_policy(input.detection_policy, input.endpoint.as_ref())
        {
            EndpointDetection::Suggested(suggestion) => ProviderSelection {
                provider_id: Some(suggestion.provider_id().clone()),
                product_id: Some(suggestion.product_id().clone()),
                source: ProviderSelectionSource::EndpointDetection,
                detection: Some(suggestion.explanation().clone()),
                config_source: None,
            },
            EndpointDetection::Unknown(explanation) => ProviderSelection {
                provider_id: None,
                product_id: None,
                source: ProviderSelectionSource::ProtocolDefault,
                detection: Some(explanation),
                config_source: None,
            },
        }
    }
}

/// Compiles one validated provider configuration into an immutable runtime.
///
/// Implementations must not retain the resolver or mutable configuration state.
/// Registry calls this method after releasing its synchronization lock.
pub trait ProviderRuntimeFactory: Send + Sync {
    /// Builds a runtime from a complete configuration snapshot.
    fn build(
        &self,
        config: &ProviderConfigSnapshot,
        resolver: &dyn SecretResolver,
    ) -> Result<ProviderRuntime, LlmError>;
}

/// Generic compiler for one immutable static provider definition.
#[derive(Clone, Debug)]
pub struct StaticProviderFactory {
    definition: ProviderDefinition,
}

impl StaticProviderFactory {
    /// Creates a static factory without requiring a custom factory trait implementation.
    pub const fn new(definition: ProviderDefinition) -> Self {
        Self { definition }
    }

    /// Returns the immutable registered definition.
    pub const fn definition(&self) -> &ProviderDefinition {
        &self.definition
    }

    /// Resolves deployment credentials and freezes a runtime.
    pub fn build_deployment(
        &self,
        deployment: &ProviderDeploymentConfig,
        resolver: &dyn SecretResolver,
    ) -> Result<ProviderRuntime, LlmError> {
        ProviderRuntime::build(self.definition.compile(deployment, resolver)?)
    }
}

/// Built-in factory for the official `OpenAI` Chat Completions profile.
#[derive(Clone, Copy, Debug, Default)]
pub struct OfficialOpenAiFactory;

impl ProviderRuntimeFactory for OfficialOpenAiFactory {
    fn build(
        &self,
        config: &ProviderConfigSnapshot,
        resolver: &dyn SecretResolver,
    ) -> Result<ProviderRuntime, LlmError> {
        config.build_official_openai_runtime(resolver)
    }
}

/// Built-in factory for the official Anthropic Messages profile.
#[derive(Clone, Copy, Debug, Default)]
pub struct OfficialAnthropicFactory;

impl ProviderRuntimeFactory for OfficialAnthropicFactory {
    fn build(
        &self,
        config: &ProviderConfigSnapshot,
        resolver: &dyn SecretResolver,
    ) -> Result<ProviderRuntime, LlmError> {
        config.build_official_anthropic_runtime(resolver)
    }
}
