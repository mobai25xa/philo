//! Explicit provider selection and definition-to-runtime compilation.
//!
//! There is exactly one way to reach a [`ProviderRuntime`]: a secret-free
//! [`ProviderDefinition`] plus a [`ProviderDeploymentConfig`] that names the
//! credential. The versioned-configuration compiler that used to sit beside it
//! moved to `philo-config` (FR-005); the core no longer knows what a
//! configuration document is.
#![allow(clippy::missing_errors_doc)]

use crate::domain::ProviderId;
use crate::error::LlmError;

use super::runtime::ProviderRuntime;
use super::secret::SecretResolver;
use super::{ProviderDefinition, ProviderDeploymentConfig};

/// Winning source in the frozen provider-selection precedence chain.
///
/// Every source is explicit. The SDK never infers a provider from an endpoint
/// URL: guessing wrong produces a request aimed at the wrong product, and
/// guessing right only saves the caller one declaration.
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
    /// No provider was declared; the caller must supply one before sending.
    Undeclared,
}

/// Typed inputs considered by [`ProviderSelector`] in strict precedence order.
#[derive(Clone, Debug, Default)]
pub struct ProviderSelectionInput {
    request: Option<ProviderId>,
    model: Option<ProviderId>,
    provider: Option<ProviderId>,
    built_in_profile: Option<ProviderId>,
}

impl ProviderSelectionInput {
    /// Creates empty input, which resolves to no declared provider.
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
    ///
    /// Which configuration source supplied the value is the configuration
    /// layer's question, not the core's; `philo-config` carries that
    /// provenance alongside the selection it feeds in here.
    #[must_use]
    pub fn with_provider(mut self, provider: ProviderId) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Sets an explicitly selected built-in profile.
    #[must_use]
    pub fn with_built_in_profile(mut self, provider: ProviderId) -> Self {
        self.built_in_profile = Some(provider);
        self
    }
}

/// The winning provider declaration and the tier it came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSelection {
    provider_id: Option<ProviderId>,
    source: ProviderSelectionSource,
}

impl ProviderSelection {
    /// Returns the declared provider, or `None` when no source declared one.
    #[must_use]
    pub const fn provider_id(&self) -> Option<&ProviderId> {
        self.provider_id.as_ref()
    }

    /// Returns the winning selection source.
    #[must_use]
    pub const fn source(&self) -> ProviderSelectionSource {
        self.source
    }
}

/// Resolves the frozen precedence chain over explicitly declared providers only.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProviderSelector;

impl ProviderSelector {
    /// Returns the highest-precedence declared provider, or `Undeclared`.
    ///
    /// There is no inferred fallback: when no source declares a provider the
    /// result carries no provider at all, and the caller must declare one
    /// before a request can be planned.
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
                    source,
                };
            }
        }

        ProviderSelection {
            provider_id: None,
            source: ProviderSelectionSource::Undeclared,
        }
    }
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
