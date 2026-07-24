//! Provider configuration snapshot to immutable runtime compilation.
#![allow(clippy::missing_errors_doc)]

use crate::error::LlmError;

use super::config::{ProviderConfigSnapshot, SecretResolver};
use super::runtime::ProviderRuntime;

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
