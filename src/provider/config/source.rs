//! Configuration source identity and value provenance.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{ProviderConfigError, ProviderConfigFailure};

/// The source class used to order configuration layers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ConfigSourceKind {
    /// Defaults owned by the protocol implementation.
    ProtocolDefault,
    /// Defaults owned by a built-in provider profile.
    BuiltInProfile,
    /// A user-authored configuration document.
    UserConfig,
    /// An explicit reference to one environment secret.
    EnvironmentSecretReference,
    /// An application-provided programmatic override.
    ProgrammaticOverride,
    /// A request-scoped safe override. Provider configuration rejects this class.
    PerRequestSafeOverride,
}

impl ConfigSourceKind {
    /// Returns the stable merge precedence.
    pub const fn precedence(self) -> u8 {
        match self {
            Self::ProtocolDefault => 0,
            Self::BuiltInProfile => 1,
            Self::UserConfig => 2,
            Self::EnvironmentSecretReference => 3,
            Self::ProgrammaticOverride => 4,
            Self::PerRequestSafeOverride => 5,
        }
    }
}

/// A validated, non-secret source identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConfigSourceId(String);

impl ConfigSourceId {
    /// Creates a source identifier without trimming or normalizing it.
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderConfigError> {
        let value = value.into();
        if value.is_empty() || value.trim() != value || value.len() > 128 {
            return Err(ProviderConfigError::new(
                "source.id",
                ProviderConfigFailure::InvalidValue,
                "source id must be non-empty, bounded, and have no boundary whitespace",
            ));
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(&byte))
        {
            return Err(ProviderConfigError::new(
                "source.id",
                ProviderConfigFailure::InvalidValue,
                "source id contains an unsupported character",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the source identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ConfigSourceId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ConfigSourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A safe location label for configuration provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigSourceLocation {
    /// A built-in source compiled into the SDK.
    BuiltIn,
    /// A user configuration document identified by a safe display label.
    File {
        /// Safe display label, not raw file contents.
        label: String,
    },
    /// One explicitly named environment variable.
    EnvironmentVariable {
        /// Explicit environment variable name.
        name: String,
    },
    /// A caller-owned programmatic component label.
    Programmatic {
        /// Safe application component label.
        label: String,
    },
    /// A request-scoped source marker.
    PerRequest,
}

impl ConfigSourceLocation {
    fn validate(&self) -> Result<(), ProviderConfigError> {
        let value = match self {
            Self::BuiltIn | Self::PerRequest => return Ok(()),
            Self::File { label } | Self::Programmatic { label } => label,
            Self::EnvironmentVariable { name } => name,
        };
        if value.is_empty() || value.trim() != value || value.len() > 256 {
            return Err(ProviderConfigError::new(
                "source.location",
                ProviderConfigFailure::InvalidValue,
                "source location must be non-empty and bounded",
            ));
        }
        Ok(())
    }
}

/// A source identity carried by every configuration layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigSource {
    id: ConfigSourceId,
    kind: ConfigSourceKind,
    location: ConfigSourceLocation,
}

impl ConfigSource {
    /// Creates and validates a source identity.
    pub fn new(
        id: impl Into<String>,
        kind: ConfigSourceKind,
        location: ConfigSourceLocation,
    ) -> Result<Self, ProviderConfigError> {
        let id = ConfigSourceId::new(id)?;
        location.validate()?;
        Ok(Self { id, kind, location })
    }

    /// Creates a built-in profile source.
    pub fn built_in(id: impl Into<String>) -> Result<Self, ProviderConfigError> {
        Self::new(
            id,
            ConfigSourceKind::BuiltInProfile,
            ConfigSourceLocation::BuiltIn,
        )
    }

    /// Creates a user configuration source.
    pub fn user_config(
        id: impl Into<String>,
        label: impl Into<String>,
    ) -> Result<Self, ProviderConfigError> {
        Self::new(
            id,
            ConfigSourceKind::UserConfig,
            ConfigSourceLocation::File {
                label: label.into(),
            },
        )
    }

    /// Creates an explicit environment-secret source.
    pub fn environment_secret(
        id: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Self, ProviderConfigError> {
        Self::new(
            id,
            ConfigSourceKind::EnvironmentSecretReference,
            ConfigSourceLocation::EnvironmentVariable { name: name.into() },
        )
    }

    /// Creates a programmatic override source.
    pub fn programmatic(id: impl Into<String>) -> Result<Self, ProviderConfigError> {
        let id = id.into();
        Self::new(
            id.clone(),
            ConfigSourceKind::ProgrammaticOverride,
            ConfigSourceLocation::Programmatic { label: id },
        )
    }

    /// Creates a request-safe source marker.
    pub fn per_request(id: impl Into<String>) -> Result<Self, ProviderConfigError> {
        Self::new(
            id,
            ConfigSourceKind::PerRequestSafeOverride,
            ConfigSourceLocation::PerRequest,
        )
    }

    /// Returns the source identifier.
    pub fn id(&self) -> &ConfigSourceId {
        &self.id
    }

    /// Returns the source class.
    pub const fn kind(&self) -> ConfigSourceKind {
        self.kind
    }

    /// Returns the safe source location.
    pub fn location(&self) -> &ConfigSourceLocation {
        &self.location
    }
}

/// Whether a layer set or removed a field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldState {
    /// The layer supplied a value.
    Set,
    /// The layer explicitly removed a value.
    Removed,
}

/// Non-sensitive provenance for one resolved field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldProvenance {
    source: ConfigSource,
    state: FieldState,
}

impl FieldProvenance {
    pub(crate) fn new(source: ConfigSource, state: FieldState) -> Self {
        Self { source, state }
    }

    /// Returns the source identity.
    pub fn source(&self) -> &ConfigSource {
        &self.source
    }

    /// Returns whether the field is set or explicitly removed.
    pub const fn state(&self) -> FieldState {
        self.state
    }
}
