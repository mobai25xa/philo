//! Deterministic field, list, and map merge operations.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::{BTreeMap, BTreeSet};

use philo::error::{ProviderConfigError, ProviderConfigFailure};

use philo::provider::secret::SecretReference;

use super::schema::{
    ClientIdentityConfig, ConfigSchemaVersion, ConfigValue, CredentialAudienceSpec, EndpointSpec,
    ProviderConfigDocument,
};
use super::source::{
    ConfigSource, ConfigSourceKind, ConfigSourceLocation, FieldProvenance, FieldState,
};

/// List update semantics for ordered configuration values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ListMerge<T> {
    /// Leave the lower-precedence list unchanged.
    Unset,
    /// Replace the complete list.
    Replace(Vec<T>),
    /// Append values in declared order.
    Append(Vec<T>),
    /// Remove the complete list.
    Remove,
}

impl<T> ListMerge<T> {
    /// Applies this operation to a lower-precedence list.
    pub fn apply(self, mut base: Vec<T>) -> Vec<T> {
        match self {
            Self::Unset => base,
            Self::Replace(values) => values,
            Self::Append(values) => {
                base.extend(values);
                base
            }
            Self::Remove => Vec::new(),
        }
    }
}

/// One value in an ID-merged configuration list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedConfigValue<T> {
    id: String,
    value: T,
}

impl<T> NamedConfigValue<T> {
    /// Creates a value with a validated merge identity.
    pub fn new(id: impl Into<String>, value: T) -> Result<Self, ProviderConfigError> {
        let id = id.into();
        if id.is_empty() || id.trim() != id || id.len() > 128 {
            return Err(ProviderConfigError::new(
                "list.id",
                ProviderConfigFailure::InvalidValue,
                "list merge id must be non-empty and bounded",
            ));
        }
        Ok(Self { id, value })
    }

    /// Returns the merge identity.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the stored value.
    pub fn value(&self) -> &T {
        &self.value
    }

    /// Consumes this entry.
    pub fn into_parts(self) -> (String, T) {
        (self.id, self.value)
    }
}

/// Update semantics for a list whose entries have stable IDs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NamedListMerge<T> {
    /// Leave the lower-precedence list unchanged.
    Unset,
    /// Replace the complete list.
    Replace(Vec<NamedConfigValue<T>>),
    /// Append entries; duplicate IDs are rejected.
    Append(Vec<NamedConfigValue<T>>),
    /// Replace matching IDs and append new IDs while preserving base order.
    MergeById(Vec<NamedConfigValue<T>>),
    /// Remove the complete list.
    Remove,
}

impl<T> NamedListMerge<T> {
    /// Applies this operation with deterministic duplicate handling.
    pub fn apply(
        self,
        base: Vec<NamedConfigValue<T>>,
    ) -> Result<Vec<NamedConfigValue<T>>, ProviderConfigError> {
        match self {
            Self::Unset => Ok(base),
            Self::Remove => Ok(Vec::new()),
            Self::Replace(values) => {
                ensure_unique_ids(&values)?;
                Ok(values)
            }
            Self::Append(values) => {
                ensure_unique_ids(&base)?;
                ensure_unique_ids(&values)?;
                let mut ids = base
                    .iter()
                    .map(|entry| entry.id.clone())
                    .collect::<BTreeSet<_>>();
                if values.iter().any(|entry| !ids.insert(entry.id.clone())) {
                    return Err(merge_conflict("list", "append contains an existing id"));
                }
                let mut merged = base;
                merged.extend(values);
                Ok(merged)
            }
            Self::MergeById(values) => {
                ensure_unique_ids(&base)?;
                ensure_unique_ids(&values)?;
                let mut updates = values
                    .into_iter()
                    .map(|entry| (entry.id.clone(), entry))
                    .collect::<BTreeMap<_, _>>();
                let mut merged = Vec::with_capacity(base.len() + updates.len());
                for entry in base {
                    if let Some(replacement) = updates.remove(&entry.id) {
                        merged.push(replacement);
                    } else {
                        merged.push(entry);
                    }
                }
                merged.extend(updates.into_values());
                Ok(merged)
            }
        }
    }
}

fn ensure_unique_ids<T>(values: &[NamedConfigValue<T>]) -> Result<(), ProviderConfigError> {
    let mut ids = BTreeSet::new();
    if values.iter().any(|entry| !ids.insert(entry.id.as_str())) {
        Err(merge_conflict("list", "list contains a duplicate id"))
    } else {
        Ok(())
    }
}

/// Update semantics for a deterministically ordered configuration map.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MapMerge<V> {
    /// Leave the lower-precedence map unchanged.
    Unset,
    /// Replace the complete map.
    Replace(BTreeMap<String, V>),
    /// Insert or replace individual keys.
    Merge(BTreeMap<String, V>),
    /// Remove named keys.
    RemoveKeys(Vec<String>),
    /// Remove the complete map.
    Remove,
}

impl<V> MapMerge<V> {
    /// Applies this operation using `BTreeMap` ordering.
    pub fn apply(self, mut base: BTreeMap<String, V>) -> BTreeMap<String, V> {
        match self {
            Self::Unset => base,
            Self::Replace(values) => values,
            Self::Merge(values) => {
                base.extend(values);
                base
            }
            Self::RemoveKeys(keys) => {
                for key in keys {
                    base.remove(&key);
                }
                base
            }
            Self::Remove => BTreeMap::new(),
        }
    }
}

/// Fields addressable in a resolved provider configuration snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProviderConfigField {
    /// Provider identifier.
    ProviderId,
    /// Protocol identifier.
    ProtocolId,
    /// Endpoint.
    Endpoint,
    /// Credential destination restriction.
    CredentialAudience,
    /// Secret reference.
    Credential,
    /// Client identity.
    ClientIdentity,
    /// HTTP error body prefix bound.
    MaxHttpErrorBodyBytes,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedField<T> {
    value: Option<T>,
    provenance: Option<FieldProvenance>,
}

impl<T> Default for ResolvedField<T> {
    fn default() -> Self {
        Self {
            value: None,
            provenance: None,
        }
    }
}

impl<T> ResolvedField<T> {
    fn apply(&mut self, setting: ConfigValue<T>, source: &ConfigSource) {
        match setting {
            ConfigValue::Unset => {}
            ConfigValue::Set(value) => {
                self.value = Some(value);
                self.provenance = Some(FieldProvenance::new(source.clone(), FieldState::Set));
            }
            ConfigValue::Remove => {
                self.value = None;
                self.provenance = Some(FieldProvenance::new(source.clone(), FieldState::Removed));
            }
        }
    }
}

/// One versioned provider configuration layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderConfigLayer {
    version: ConfigSchemaVersion,
    source: ConfigSource,
    provider_id: ConfigValue<String>,
    protocol_id: ConfigValue<String>,
    endpoint: ConfigValue<EndpointSpec>,
    credential_audience: ConfigValue<CredentialAudienceSpec>,
    credential: ConfigValue<SecretReference>,
    client_identity: ConfigValue<ClientIdentityConfig>,
    max_http_error_body_bytes: ConfigValue<usize>,
}

impl ProviderConfigLayer {
    /// Creates an empty layer using the current schema.
    pub fn new(source: ConfigSource) -> Self {
        Self {
            version: ConfigSchemaVersion::CURRENT,
            source,
            provider_id: ConfigValue::Unset,
            protocol_id: ConfigValue::Unset,
            endpoint: ConfigValue::Unset,
            credential_audience: ConfigValue::Unset,
            credential: ConfigValue::Unset,
            client_identity: ConfigValue::Unset,
            max_http_error_body_bytes: ConfigValue::Unset,
        }
    }

    /// Converts a deserialized document into a sourced layer.
    pub fn from_document(
        document: ProviderConfigDocument,
        source: ConfigSource,
    ) -> Result<Self, ProviderConfigError> {
        document.schema_version.validate()?;
        Ok(Self {
            version: document.schema_version,
            source,
            provider_id: document.provider_id,
            protocol_id: document.protocol_id,
            endpoint: document.endpoint,
            credential_audience: document.credential_audience,
            credential: document.credential,
            client_identity: document.client_identity,
            max_http_error_body_bytes: document.max_http_error_body_bytes,
        })
    }

    /// Parses a JSON document and attaches the source to any parse or schema error.
    pub fn from_json(input: &str, source: ConfigSource) -> Result<Self, ProviderConfigError> {
        let document = ProviderConfigDocument::from_json(input)
            .map_err(|error| error.with_source(source.id().as_str()))?;
        Self::from_document(document, source)
    }

    /// Sets the schema version for programmatic migration tests or adapters.
    #[must_use]
    pub const fn with_version(mut self, version: ConfigSchemaVersion) -> Self {
        self.version = version;
        self
    }

    /// Sets the provider identifier operation.
    #[must_use]
    pub fn with_provider_id(mut self, value: ConfigValue<String>) -> Self {
        self.provider_id = value;
        self
    }

    /// Sets the protocol identifier operation.
    #[must_use]
    pub fn with_protocol_id(mut self, value: ConfigValue<String>) -> Self {
        self.protocol_id = value;
        self
    }

    /// Sets the endpoint operation.
    #[must_use]
    pub fn with_endpoint(mut self, value: ConfigValue<EndpointSpec>) -> Self {
        self.endpoint = value;
        self
    }

    /// Sets the credential audience operation.
    #[must_use]
    pub fn with_credential_audience(mut self, value: ConfigValue<CredentialAudienceSpec>) -> Self {
        self.credential_audience = value;
        self
    }

    /// Sets the credential reference operation.
    #[must_use]
    pub fn with_credential(mut self, value: ConfigValue<SecretReference>) -> Self {
        self.credential = value;
        self
    }

    /// Sets the client identity operation.
    #[must_use]
    pub fn with_client_identity(mut self, value: ConfigValue<ClientIdentityConfig>) -> Self {
        self.client_identity = value;
        self
    }

    /// Sets the HTTP error body limit operation.
    #[must_use]
    pub const fn with_max_http_error_body_bytes(mut self, value: ConfigValue<usize>) -> Self {
        self.max_http_error_body_bytes = value;
        self
    }

    /// Returns the source identity.
    pub fn source(&self) -> &ConfigSource {
        &self.source
    }

    fn has_non_credential_change(&self) -> bool {
        !self.provider_id.is_unset()
            || !self.protocol_id.is_unset()
            || !self.endpoint.is_unset()
            || !self.credential_audience.is_unset()
            || !self.client_identity.is_unset()
            || !self.max_http_error_body_bytes.is_unset()
    }

    fn has_any_change(&self) -> bool {
        self.has_non_credential_change() || !self.credential.is_unset()
    }

    fn validate_source_permissions(&self) -> Result<(), ProviderConfigError> {
        let forbidden = |field: &'static str| {
            ProviderConfigError::new(
                field,
                ProviderConfigFailure::ForbiddenOverride,
                "configuration source is not allowed to modify this field",
            )
            .with_source(self.source.id().as_str())
        };
        match self.source.kind() {
            ConfigSourceKind::EnvironmentSecretReference => {
                if self.has_non_credential_change() {
                    return Err(forbidden("layer"));
                }
                let (
                    ConfigValue::Set(reference),
                    ConfigSourceLocation::EnvironmentVariable {
                        name: location_name,
                    },
                ) = (&self.credential, self.source.location())
                else {
                    return Err(forbidden("credential"));
                };
                if reference.name() != location_name {
                    return Err(ProviderConfigError::new(
                        "credential",
                        ProviderConfigFailure::MergeConflict,
                        "environment source and secret reference names do not match",
                    )
                    .with_source(self.source.id().as_str()));
                }
            }
            ConfigSourceKind::PerRequestSafeOverride if self.has_any_change() => {
                return Err(forbidden("layer"));
            }
            _ => {}
        }
        Ok(())
    }
}

/// Immutable, provenance-carrying provider configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderConfigSnapshot {
    version: ConfigSchemaVersion,
    provider_id: ResolvedField<String>,
    protocol_id: ResolvedField<String>,
    endpoint: ResolvedField<EndpointSpec>,
    credential_audience: ResolvedField<CredentialAudienceSpec>,
    credential: ResolvedField<SecretReference>,
    client_identity: ResolvedField<ClientIdentityConfig>,
    max_http_error_body_bytes: ResolvedField<usize>,
}

impl ProviderConfigSnapshot {
    /// Creates the built-in official `OpenAI` configuration without a secret value.
    pub fn official_openai() -> Result<Self, ProviderConfigError> {
        let source = ConfigSource::built_in("builtin/official-openai")?;
        let mut snapshot = Self {
            version: ConfigSchemaVersion::CURRENT,
            provider_id: ResolvedField::default(),
            protocol_id: ResolvedField::default(),
            endpoint: ResolvedField::default(),
            credential_audience: ResolvedField::default(),
            credential: ResolvedField::default(),
            client_identity: ResolvedField::default(),
            max_http_error_body_bytes: ResolvedField::default(),
        };
        snapshot
            .provider_id
            .apply(ConfigValue::Set("official-openai".to_owned()), &source);
        snapshot.protocol_id.apply(
            ConfigValue::Set("openai-chat-completions".to_owned()),
            &source,
        );
        snapshot.endpoint.apply(
            ConfigValue::Set(EndpointSpec::base_and_path(
                "https://api.openai.com/v1",
                "/chat/completions",
            )),
            &source,
        );
        snapshot.credential_audience.apply(
            ConfigValue::Set(CredentialAudienceSpec::OfficialOpenAi),
            &source,
        );
        snapshot.client_identity.apply(
            ConfigValue::Set(ClientIdentityConfig::new(
                philo::SDK_NAME,
                philo::SDK_VERSION,
            )),
            &source,
        );
        snapshot
            .max_http_error_body_bytes
            .apply(ConfigValue::Set(16 * 1024), &source);
        Ok(snapshot)
    }

    /// Creates the built-in official Anthropic configuration without a secret value.
    pub fn official_anthropic() -> Result<Self, ProviderConfigError> {
        let source = ConfigSource::built_in("builtin/official-anthropic")?;
        let mut snapshot = Self {
            version: ConfigSchemaVersion::CURRENT,
            provider_id: ResolvedField::default(),
            protocol_id: ResolvedField::default(),
            endpoint: ResolvedField::default(),
            credential_audience: ResolvedField::default(),
            credential: ResolvedField::default(),
            client_identity: ResolvedField::default(),
            max_http_error_body_bytes: ResolvedField::default(),
        };
        snapshot
            .provider_id
            .apply(ConfigValue::Set("official-anthropic".to_owned()), &source);
        snapshot
            .protocol_id
            .apply(ConfigValue::Set("anthropic-messages".to_owned()), &source);
        snapshot.endpoint.apply(
            ConfigValue::Set(EndpointSpec::base_and_path(
                "https://api.anthropic.com/v1",
                "/messages",
            )),
            &source,
        );
        snapshot.credential_audience.apply(
            ConfigValue::Set(CredentialAudienceSpec::OfficialAnthropic),
            &source,
        );
        snapshot.client_identity.apply(
            ConfigValue::Set(ClientIdentityConfig::new(
                philo::SDK_NAME,
                philo::SDK_VERSION,
            )),
            &source,
        );
        snapshot
            .max_http_error_body_bytes
            .apply(ConfigValue::Set(16 * 1024), &source);
        Ok(snapshot)
    }

    /// Merges layers by source precedence and then by source ID.
    pub fn merge_layers<I>(mut self, layers: I) -> Result<Self, ProviderConfigError>
    where
        I: IntoIterator<Item = ProviderConfigLayer>,
    {
        let mut layers = layers.into_iter().collect::<Vec<_>>();
        let mut source_keys = BTreeSet::new();
        for layer in &layers {
            layer.version.validate()?;
            layer.validate_source_permissions()?;
            let source_key = (
                layer.source.kind().precedence(),
                layer.source.id().as_str().to_owned(),
            );
            if !source_keys.insert(source_key) {
                return Err(ProviderConfigError::new(
                    "source.id",
                    ProviderConfigFailure::MergeConflict,
                    "configuration layers contain a duplicate source identity",
                )
                .with_source(layer.source.id().as_str()));
            }
        }
        layers.sort_by(|left, right| {
            left.source
                .kind()
                .precedence()
                .cmp(&right.source.kind().precedence())
                .then_with(|| left.source.id().cmp(right.source.id()))
        });
        for layer in layers {
            self.version.minor = self.version.minor.max(layer.version.minor);
            self.provider_id.apply(layer.provider_id, &layer.source);
            self.protocol_id.apply(layer.protocol_id, &layer.source);
            self.endpoint.apply(layer.endpoint, &layer.source);
            self.credential_audience
                .apply(layer.credential_audience, &layer.source);
            self.credential.apply(layer.credential, &layer.source);
            self.client_identity
                .apply(layer.client_identity, &layer.source);
            self.max_http_error_body_bytes
                .apply(layer.max_http_error_body_bytes, &layer.source);
        }
        super::validate::validate_snapshot(&self)?;
        Ok(self)
    }

    /// Returns the final schema version.
    pub const fn version(&self) -> ConfigSchemaVersion {
        self.version
    }

    /// Returns the final provider identifier.
    pub fn provider_id(&self) -> Option<&str> {
        self.provider_id.value.as_deref()
    }

    /// Returns the final protocol identifier.
    pub fn protocol_id(&self) -> Option<&str> {
        self.protocol_id.value.as_deref()
    }

    /// Returns the final endpoint specification.
    pub fn endpoint(&self) -> Option<&EndpointSpec> {
        self.endpoint.value.as_ref()
    }

    /// Returns the final credential audience.
    pub fn credential_audience(&self) -> Option<CredentialAudienceSpec> {
        self.credential_audience.value
    }

    /// Returns the final secret reference, never the resolved secret.
    pub fn credential_reference(&self) -> Option<&SecretReference> {
        self.credential.value.as_ref()
    }

    /// Returns the final client identity configuration.
    pub fn client_identity(&self) -> Option<&ClientIdentityConfig> {
        self.client_identity.value.as_ref()
    }

    /// Returns the final bounded HTTP error-body prefix size.
    pub fn max_http_error_body_bytes(&self) -> Option<usize> {
        self.max_http_error_body_bytes.value
    }

    /// Returns value-free provenance for one field.
    pub fn provenance(&self, field: ProviderConfigField) -> Option<&FieldProvenance> {
        match field {
            ProviderConfigField::ProviderId => self.provider_id.provenance.as_ref(),
            ProviderConfigField::ProtocolId => self.protocol_id.provenance.as_ref(),
            ProviderConfigField::Endpoint => self.endpoint.provenance.as_ref(),
            ProviderConfigField::CredentialAudience => self.credential_audience.provenance.as_ref(),
            ProviderConfigField::Credential => self.credential.provenance.as_ref(),
            ProviderConfigField::ClientIdentity => self.client_identity.provenance.as_ref(),
            ProviderConfigField::MaxHttpErrorBodyBytes => {
                self.max_http_error_body_bytes.provenance.as_ref()
            }
        }
    }

    pub(crate) fn provider_id_field(&self) -> &ResolvedField<String> {
        &self.provider_id
    }

    pub(crate) fn protocol_id_field(&self) -> &ResolvedField<String> {
        &self.protocol_id
    }

    pub(crate) fn endpoint_field(&self) -> &ResolvedField<EndpointSpec> {
        &self.endpoint
    }

    pub(crate) fn audience_field(&self) -> &ResolvedField<CredentialAudienceSpec> {
        &self.credential_audience
    }

    pub(crate) fn credential_field(&self) -> &ResolvedField<SecretReference> {
        &self.credential
    }

    pub(crate) fn identity_field(&self) -> &ResolvedField<ClientIdentityConfig> {
        &self.client_identity
    }

    pub(crate) fn error_limit_field(&self) -> &ResolvedField<usize> {
        &self.max_http_error_body_bytes
    }
}

impl<T> ResolvedField<T> {
    pub(crate) fn value(&self) -> Option<&T> {
        self.value.as_ref()
    }

    pub(crate) fn source_id(&self) -> Option<&str> {
        self.provenance
            .as_ref()
            .map(|provenance| provenance.source().id().as_str())
    }
}

fn merge_conflict(field: &'static str, message: &'static str) -> ProviderConfigError {
    ProviderConfigError::new(field, ProviderConfigFailure::MergeConflict, message)
}
