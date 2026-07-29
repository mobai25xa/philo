//! Versioned, value-free provider configuration schema.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use serde::{Deserialize, Serialize};

use philo::error::{ProviderConfigError, ProviderConfigFailure};
use philo::provider::secret::SecretReference;

/// The schema version understood by this crate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSchemaVersion {
    /// Major schema version. A mismatch is rejected.
    pub major: u16,
    /// Minor schema version. Newer recognized-field minors remain readable.
    pub minor: u16,
}

impl ConfigSchemaVersion {
    /// The current configuration schema.
    pub const CURRENT: Self = Self { major: 1, minor: 1 };

    /// The immediately previous schema accepted by the migration path.
    pub const PREVIOUS: Self = Self { major: 1, minor: 0 };

    /// Validates the compatibility policy for this schema.
    pub fn validate(self) -> Result<(), ProviderConfigError> {
        if self.major != Self::CURRENT.major {
            return Err(ProviderConfigError::new(
                "schema_version.major",
                ProviderConfigFailure::InvalidVersion,
                "unsupported provider configuration major version",
            ));
        }
        Ok(())
    }
}

impl Default for ConfigSchemaVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

/// Explicit scalar state used during layer merge.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum ConfigValue<T> {
    /// Leave the lower-precedence value unchanged.
    #[default]
    Unset,
    /// Set a value, including an intentionally empty value when the field allows it.
    Set(T),
    /// Remove the lower-precedence value and let validation decide if it was required.
    Remove,
}

impl<T> ConfigValue<T> {
    /// Returns whether this value leaves the lower layer unchanged.
    pub const fn is_unset(&self) -> bool {
        matches!(self, Self::Unset)
    }

    /// Creates a set value.
    pub const fn set(value: T) -> Self {
        Self::Set(value)
    }

    /// Creates an explicit removal.
    pub const fn remove() -> Self {
        Self::Remove
    }
}

/// Endpoint input accepted by the versioned configuration document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EndpointSpec {
    /// Append one path to a validated base URL.
    BaseAndPath {
        /// Base URL, including any intentional path prefix.
        base_url: String,
        /// Endpoint path appended to the base path.
        endpoint_path: String,
    },
    /// Use one absolute endpoint URL.
    Absolute {
        /// Absolute endpoint URL.
        url: String,
    },
}

impl EndpointSpec {
    /// Creates a base/path endpoint specification.
    pub fn base_and_path(base_url: impl Into<String>, endpoint_path: impl Into<String>) -> Self {
        Self::BaseAndPath {
            base_url: base_url.into(),
            endpoint_path: endpoint_path.into(),
        }
    }

    /// Creates an absolute endpoint specification.
    pub fn absolute(url: impl Into<String>) -> Self {
        Self::Absolute { url: url.into() }
    }

    /// Returns the base URL/path components without exposing credentials.
    pub fn as_parts(&self) -> (&str, Option<&str>) {
        match self {
            Self::BaseAndPath {
                base_url,
                endpoint_path,
            } => (base_url, Some(endpoint_path)),
            Self::Absolute { url } => (url, None),
        }
    }
}

/// Serializable form of the controlled SDK client identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientIdentityConfig {
    /// Product segment of the User-Agent.
    pub product: String,
    /// Version segment of the User-Agent.
    pub version: String,
}

impl ClientIdentityConfig {
    /// Creates a client identity configuration.
    pub fn new(product: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            product: product.into(),
            version: version.into(),
        }
    }
}

/// A JSON-deserializable user configuration document.
///
/// Omitted fields become [`ConfigValue::Unset`]. The tagged representation is
/// intentional: `Set("")` and `Remove` remain distinct from omission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfigDocument {
    /// Version of this document.
    pub schema_version: ConfigSchemaVersion,
    /// Provider identifier override.
    #[serde(default)]
    pub provider_id: ConfigValue<String>,
    /// Protocol identifier override.
    #[serde(default)]
    pub protocol_id: ConfigValue<String>,
    /// Endpoint override.
    #[serde(default)]
    pub endpoint: ConfigValue<EndpointSpec>,
    /// Credential audience override.
    #[serde(default)]
    pub credential_audience: ConfigValue<CredentialAudienceSpec>,
    /// Explicit secret reference.
    #[serde(default)]
    pub credential: ConfigValue<SecretReference>,
    /// User-Agent/client identity override.
    #[serde(default)]
    pub client_identity: ConfigValue<ClientIdentityConfig>,
    /// Bounded HTTP error body prefix size.
    #[serde(default)]
    pub max_http_error_body_bytes: ConfigValue<usize>,
}

impl ProviderConfigDocument {
    /// Parses a JSON document, rejects unknown fields/majors, and migrates N-1.
    pub fn from_json(input: &str) -> Result<Self, ProviderConfigError> {
        if input.len() > 64 * 1024 {
            return Err(ProviderConfigError::new(
                "document",
                ProviderConfigFailure::InvalidDocument,
                "provider configuration document exceeds 64 KiB",
            ));
        }
        let document: Self = serde_json::from_str(input).map_err(|_| {
            ProviderConfigError::new(
                "document",
                ProviderConfigFailure::InvalidDocument,
                "provider configuration document is not valid for the known schema",
            )
        })?;
        document.into_current()
    }

    /// Serializes the document using the current writer version.
    pub fn to_current_json(&self) -> Result<String, ProviderConfigError> {
        let mut current = self.clone().into_current()?;
        current.schema_version = ConfigSchemaVersion::CURRENT;
        serde_json::to_string_pretty(&current).map_err(|_| {
            ProviderConfigError::new(
                "document",
                ProviderConfigFailure::InvalidDocument,
                "provider configuration document could not be serialized",
            )
        })
    }

    pub(crate) fn into_current(mut self) -> Result<Self, ProviderConfigError> {
        self.schema_version.validate()?;
        if self.schema_version <= ConfigSchemaVersion::CURRENT {
            self.schema_version = ConfigSchemaVersion::CURRENT;
        }
        Ok(self)
    }
}

/// Credential audience names accepted by the first configuration compiler.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum CredentialAudienceSpec {
    /// Official `OpenAI` API origin.
    OfficialOpenAi,
    /// Official Anthropic API origin.
    OfficialAnthropic,
}
