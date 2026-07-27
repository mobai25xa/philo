//! Explicit secret-reference and secret-resolution boundary.
//!
//! Loading, merging, and validating configuration is not the core's business —
//! that lives in `philo-config` (FR-005). What *is* the core's business is
//! knowing that a credential is a **reference** rather than a plaintext value,
//! because only then can it guarantee the secret never reaches a log, a `Debug`
//! rendering, or a diagnostic. That guarantee is why these three items stayed
//! behind when the rest of `provider/config` moved out.
#![allow(clippy::missing_errors_doc)]

use serde::{Deserialize, Serialize};

use crate::error::{ProviderConfigError, ProviderConfigFailure};
use crate::provider::auth::ApiKey;

/// A named environment-secret reference. The secret value is never stored here.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum SecretReference {
    /// Read exactly one named environment variable when explicitly resolved.
    EnvironmentVariable(String),
}

impl SecretReference {
    /// Creates and validates an environment variable reference.
    pub fn environment_variable(name: impl Into<String>) -> Result<Self, ProviderConfigError> {
        let name = name.into();
        if name.is_empty()
            || name.len() > 128
            || !name.bytes().enumerate().all(|(index, byte)| {
                (index == 0 && (byte.is_ascii_alphabetic() || byte == b'_'))
                    || (index > 0 && (byte.is_ascii_alphanumeric() || byte == b'_'))
            })
        {
            return Err(ProviderConfigError::new(
                "credential.name",
                ProviderConfigFailure::InvalidValue,
                "environment secret reference must be a valid variable name",
            ));
        }
        Ok(Self::EnvironmentVariable(name))
    }

    /// Returns the referenced environment variable name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::EnvironmentVariable(name) => name,
        }
    }

    /// Re-checks a reference that arrived by deserialization rather than
    /// through [`Self::environment_variable`].
    pub fn validate(&self) -> Result<(), ProviderConfigError> {
        Self::environment_variable(self.name()).map(|_| ())
    }
}

impl std::fmt::Debug for SecretReference {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretReference")
            .field("kind", &"environment_variable")
            .field("name", &self.name())
            .finish()
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_carries_only_a_name_and_resolution_stays_explicit() {
        // The reference names the variable; reading it is a separate, explicit
        // act. There is nowhere in this type for a secret value to live, which
        // is the property the core depends on.
        let reference = SecretReference::environment_variable("PHILO_ABSENT_SECRET").unwrap();
        assert_eq!(reference.name(), "PHILO_ABSENT_SECRET");
        assert!(format!("{reference:?}").contains("PHILO_ABSENT_SECRET"));

        let error = EnvironmentSecretResolver.resolve(&reference).unwrap_err();
        assert_eq!(error.reason(), ProviderConfigFailure::SecretUnavailable);
    }

    #[test]
    fn invalid_variable_names_fail_closed() {
        for name in ["", "1leading-digit", "has space", "has-dash"] {
            assert!(
                SecretReference::environment_variable(name).is_err(),
                "{name}"
            );
        }
        assert!(SecretReference::environment_variable("_OK_1").is_ok());
    }
}
