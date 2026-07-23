//! Explicit secret resolution boundary.
#![allow(clippy::missing_errors_doc)]

use crate::error::{ProviderConfigError, ProviderConfigFailure};
use crate::provider::auth::ApiKey;

use super::schema::SecretReference;

/// Resolves one explicit reference into a redacted API-key type.
pub trait SecretResolver {
    /// Resolves the requested secret without enumerating unrelated secrets.
    fn resolve(&self, reference: &SecretReference) -> Result<ApiKey, ProviderConfigError>;
}

/// Resolver that reads exactly the environment variable named by the reference.
#[derive(Clone, Copy, Debug, Default)]
pub struct EnvironmentSecretResolver;

impl SecretResolver for EnvironmentSecretResolver {
    fn resolve(&self, reference: &SecretReference) -> Result<ApiKey, ProviderConfigError> {
        let value = std::env::var(reference.name()).map_err(|_| {
            ProviderConfigError::new(
                "credential",
                ProviderConfigFailure::SecretUnavailable,
                "referenced environment secret is unavailable",
            )
        })?;
        ApiKey::new(value).map_err(|_| {
            ProviderConfigError::new(
                "credential",
                ProviderConfigFailure::InvalidValue,
                "referenced secret is not a valid API key",
            )
        })
    }
}
