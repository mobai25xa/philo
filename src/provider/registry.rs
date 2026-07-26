//! Thread-safe provider registration and immutable runtime construction.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::domain::ProviderId;
use crate::error::{
    LlmError, ProviderConfigError, ProviderConfigFailure, ProviderRegistryError,
    ProviderRegistryFailure,
};

use super::config::{ProviderConfigField, ProviderConfigSnapshot, SecretResolver};
use super::factory::{OfficialAnthropicFactory, OfficialOpenAiFactory, ProviderRuntimeFactory};
use super::runtime::ProviderRuntime;

/// Value-free metadata describing one registered provider factory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRegistrationMetadata {
    provider_id: ProviderId,
    version: String,
}

impl ProviderRegistrationMetadata {
    /// Returns the normalized provider identifier.
    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    /// Returns the registration implementation version.
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// One immutable provider factory registration.
#[derive(Clone)]
pub struct ProviderRegistration {
    metadata: ProviderRegistrationMetadata,
    factory: Arc<dyn ProviderRuntimeFactory>,
}

impl ProviderRegistration {
    /// Creates a registration and normalizes its provider ID to lowercase ASCII.
    pub fn new<F>(
        provider_id: impl Into<String>,
        version: impl Into<String>,
        factory: F,
    ) -> Result<Self, ProviderRegistryError>
    where
        F: ProviderRuntimeFactory + 'static,
    {
        Self::from_shared(provider_id, version, Arc::new(factory))
    }

    /// Creates a registration from an already shared factory.
    pub fn from_shared(
        provider_id: impl Into<String>,
        version: impl Into<String>,
        factory: Arc<dyn ProviderRuntimeFactory>,
    ) -> Result<Self, ProviderRegistryError> {
        let provider_id = provider_id.into();
        let provider_id = normalize_provider_id(&provider_id)?;
        let version = validate_version(version.into(), &provider_id)?;
        Ok(Self {
            metadata: ProviderRegistrationMetadata {
                provider_id,
                version,
            },
            factory,
        })
    }

    /// Returns value-free registration metadata.
    pub fn metadata(&self) -> &ProviderRegistrationMetadata {
        &self.metadata
    }
}

impl fmt::Debug for ProviderRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderRegistration")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

/// Startup-time registry for provider runtime factories.
///
/// Clones share the same registration map. Built runtimes never retain or read
/// this map and therefore remain unchanged after replacement or removal.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    registrations: Arc<RwLock<BTreeMap<ProviderId, ProviderRegistration>>>,
}

impl ProviderRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry containing the official `OpenAI` built-in factory.
    pub fn with_official_openai() -> Result<Self, ProviderRegistryError> {
        let registry = Self::new();
        registry.register(ProviderRegistration::new(
            "official-openai",
            crate::PROVIDER_CONFIG_SCHEMA_VERSION,
            OfficialOpenAiFactory,
        )?)?;
        Ok(registry)
    }

    /// Creates a registry containing the official Anthropic built-in factory.
    pub fn with_official_anthropic() -> Result<Self, ProviderRegistryError> {
        let registry = Self::new();
        registry.register(ProviderRegistration::new(
            "official-anthropic",
            crate::PROVIDER_CONFIG_SCHEMA_VERSION,
            OfficialAnthropicFactory,
        )?)?;
        Ok(registry)
    }

    /// Creates a registry containing all official built-in factories.
    pub fn with_official_profiles() -> Result<Self, ProviderRegistryError> {
        let registry = Self::with_official_openai()?;
        registry.register(ProviderRegistration::new(
            "official-anthropic",
            crate::PROVIDER_CONFIG_SCHEMA_VERSION,
            OfficialAnthropicFactory,
        )?)?;
        Ok(registry)
    }

    /// Registers a provider, rejecting a normalized duplicate ID.
    pub fn register(
        &self,
        registration: ProviderRegistration,
    ) -> Result<ProviderRegistrationMetadata, ProviderRegistryError> {
        let metadata = registration.metadata.clone();
        let mut registrations = self.write()?;
        if registrations.contains_key(&metadata.provider_id) {
            return Err(ProviderRegistryError::new(
                ProviderRegistryFailure::DuplicateRegistration,
                Some(metadata.provider_id.as_str()),
                "provider ID is already registered; use explicit replace",
            ));
        }
        registrations.insert(metadata.provider_id.clone(), registration);
        Ok(metadata)
    }

    /// Explicitly replaces an existing registration and returns its old metadata.
    pub fn replace(
        &self,
        registration: ProviderRegistration,
    ) -> Result<ProviderRegistrationMetadata, ProviderRegistryError> {
        let provider_id = registration.metadata.provider_id.clone();
        let mut registrations = self.write()?;
        if !registrations.contains_key(&provider_id) {
            return Err(ProviderRegistryError::new(
                ProviderRegistryFailure::RegistrationNotFound,
                Some(provider_id.as_str()),
                "provider must be registered before it can be replaced",
            ));
        }
        let previous = registrations
            .insert(provider_id, registration)
            .ok_or_else(state_unavailable)?;
        Ok(previous.metadata)
    }

    /// Returns value-free metadata for an exact normalized provider ID.
    pub fn get(
        &self,
        provider_id: &ProviderId,
    ) -> Result<Option<ProviderRegistrationMetadata>, ProviderRegistryError> {
        Ok(self
            .read()?
            .get(provider_id)
            .map(|registration| registration.metadata.clone()))
    }

    /// Normalizes a textual provider ID and returns its registration metadata.
    pub fn get_by_name(
        &self,
        provider_id: impl Into<String>,
    ) -> Result<Option<ProviderRegistrationMetadata>, ProviderRegistryError> {
        let provider_id = provider_id.into();
        let provider_id = normalize_provider_id(&provider_id)?;
        self.get(&provider_id)
    }

    /// Lists registrations in deterministic normalized provider-ID order.
    pub fn list(&self) -> Result<Vec<ProviderRegistrationMetadata>, ProviderRegistryError> {
        Ok(self
            .read()?
            .values()
            .map(|registration| registration.metadata.clone())
            .collect())
    }

    /// Removes one registration without affecting runtimes already built from it.
    pub fn remove(
        &self,
        provider_id: &ProviderId,
    ) -> Result<Option<ProviderRegistrationMetadata>, ProviderRegistryError> {
        Ok(self
            .write()?
            .remove(provider_id)
            .map(|registration| registration.metadata))
    }

    /// Builds a runtime using a registration snapshot captured before factory execution.
    pub fn build(
        &self,
        provider_id: &ProviderId,
        config: &ProviderConfigSnapshot,
        resolver: &dyn SecretResolver,
    ) -> Result<ProviderRuntime, LlmError> {
        let registration = {
            let registrations = self.read()?;
            registrations.get(provider_id).cloned().ok_or_else(|| {
                ProviderRegistryError::new(
                    ProviderRegistryFailure::RegistrationNotFound,
                    Some(provider_id.as_str()),
                    "provider ID is not registered",
                )
            })?
        };

        ensure_config_provider(config, provider_id)?;
        let runtime = registration.factory.build(config, resolver)?;
        if runtime.provider_id() != provider_id {
            return Err(ProviderRegistryError::new(
                ProviderRegistryFailure::FactoryProviderMismatch,
                Some(provider_id.as_str()),
                "factory returned a runtime for a different provider ID",
            )
            .into());
        }
        Ok(runtime)
    }

    fn read(
        &self,
    ) -> Result<
        RwLockReadGuard<'_, BTreeMap<ProviderId, ProviderRegistration>>,
        ProviderRegistryError,
    > {
        self.registrations.read().map_err(|_| state_unavailable())
    }

    fn write(
        &self,
    ) -> Result<
        RwLockWriteGuard<'_, BTreeMap<ProviderId, ProviderRegistration>>,
        ProviderRegistryError,
    > {
        self.registrations.write().map_err(|_| state_unavailable())
    }
}

impl fmt::Debug for ProviderRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.list() {
            Ok(registrations) => formatter
                .debug_struct("ProviderRegistry")
                .field("registrations", &registrations)
                .finish(),
            Err(_) => formatter
                .debug_struct("ProviderRegistry")
                .field("state", &"unavailable")
                .finish(),
        }
    }
}

fn normalize_provider_id(value: &str) -> Result<ProviderId, ProviderRegistryError> {
    let normalized = value.trim().to_ascii_lowercase();
    let valid = !normalized.is_empty()
        && normalized.len() <= 128
        && normalized.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._/-".contains(&byte)
        })
        && normalized
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && normalized
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric);
    if !valid {
        return Err(ProviderRegistryError::new(
            ProviderRegistryFailure::InvalidProviderId,
            None,
            "provider ID must be bounded lowercase ASCII with controlled separators",
        ));
    }
    ProviderId::new(normalized).map_err(|_| {
        ProviderRegistryError::new(
            ProviderRegistryFailure::InvalidProviderId,
            None,
            "provider ID failed domain validation",
        )
    })
}

fn validate_version(
    version: String,
    provider_id: &ProviderId,
) -> Result<String, ProviderRegistryError> {
    let valid = !version.is_empty()
        && version.len() <= 64
        && version.trim() == version
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'));
    if valid {
        Ok(version)
    } else {
        Err(ProviderRegistryError::new(
            ProviderRegistryFailure::InvalidVersion,
            Some(provider_id.as_str()),
            "registration version must be non-empty bounded ASCII",
        ))
    }
}

fn ensure_config_provider(
    config: &ProviderConfigSnapshot,
    provider_id: &ProviderId,
) -> Result<(), ProviderConfigError> {
    if config.provider_id() == Some(provider_id.as_str()) {
        return Ok(());
    }
    let error = ProviderConfigError::new(
        "provider_id",
        ProviderConfigFailure::InvalidValue,
        "configuration provider does not match the selected registration",
    );
    Err(match config.provenance(ProviderConfigField::ProviderId) {
        Some(provenance) => error.with_source(provenance.source().id().as_str()),
        None => error,
    })
}

fn state_unavailable() -> ProviderRegistryError {
    ProviderRegistryError::new(
        ProviderRegistryFailure::StateUnavailable,
        None,
        "provider registry synchronization state is unavailable",
    )
}
