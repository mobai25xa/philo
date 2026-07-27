//! Thread-safe provider registration and immutable runtime construction.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::domain::{ProtocolId, ProviderId};
use crate::error::{LlmError, ProviderRegistryError, ProviderRegistryFailure};

use super::factory::StaticProviderFactory;
use super::profiles::{OfficialAnthropicProfile, OfficialOpenAiProfile};
use super::runtime::ProviderRuntime;
use super::secret::SecretResolver;
use super::{ProductId, ProviderDefinition, ProviderDeploymentConfig};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RegistrationKey {
    provider_id: ProviderId,
    product_id: Option<ProductId>,
}

/// Value-free metadata describing one registered provider factory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRegistrationMetadata {
    provider_id: ProviderId,
    product_id: Option<ProductId>,
    protocol_id: Option<ProtocolId>,
    version: String,
}

impl ProviderRegistrationMetadata {
    /// Returns the normalized provider identifier.
    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    /// Returns the product identity for a static definition registration.
    pub const fn product_id(&self) -> Option<&ProductId> {
        self.product_id.as_ref()
    }

    /// Returns the protocol identity for a static definition registration.
    pub const fn protocol_id(&self) -> Option<&ProtocolId> {
        self.protocol_id.as_ref()
    }

    /// Returns the registration implementation version.
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// One immutable provider definition registration.
#[derive(Clone)]
pub struct ProviderRegistration {
    metadata: ProviderRegistrationMetadata,
    compiler: Box<StaticProviderFactory>,
}

impl ProviderRegistration {
    /// Creates a static registration directly from a validated definition.
    pub fn from_definition(definition: ProviderDefinition) -> Result<Self, ProviderRegistryError> {
        let provider_id = normalize_provider_id(definition.provider_id().as_str())?;
        let version = validate_version(
            crate::PROVIDER_CONFIG_SCHEMA_VERSION.to_owned(),
            &provider_id,
        )?;
        Ok(Self {
            metadata: ProviderRegistrationMetadata {
                provider_id,
                product_id: Some(definition.product_id().clone()),
                protocol_id: Some(definition.protocol_id().clone()),
                version,
            },
            compiler: Box::new(StaticProviderFactory::new(definition)),
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
    registrations: Arc<RwLock<BTreeMap<RegistrationKey, ProviderRegistration>>>,
}

impl ProviderRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry containing the official `OpenAI` definition.
    pub fn with_official_openai() -> Result<Self, ProviderRegistryError> {
        let registry = Self::new();
        registry.register(ProviderRegistration::from_definition(
            OfficialOpenAiProfile::definition().map_err(official_definition_unavailable)?,
        )?)?;
        Ok(registry)
    }

    /// Creates a registry containing the official Anthropic definition.
    pub fn with_official_anthropic() -> Result<Self, ProviderRegistryError> {
        let registry = Self::new();
        registry.register(ProviderRegistration::from_definition(
            OfficialAnthropicProfile::definition().map_err(official_definition_unavailable)?,
        )?)?;
        Ok(registry)
    }

    /// Creates a registry containing both official definitions.
    pub fn with_official_profiles() -> Result<Self, ProviderRegistryError> {
        let registry = Self::with_official_openai()?;
        registry.register(ProviderRegistration::from_definition(
            OfficialAnthropicProfile::definition().map_err(official_definition_unavailable)?,
        )?)?;
        Ok(registry)
    }

    /// Registers a provider, rejecting a normalized duplicate ID.
    pub fn register(
        &self,
        registration: ProviderRegistration,
    ) -> Result<ProviderRegistrationMetadata, ProviderRegistryError> {
        let metadata = registration.metadata.clone();
        let key = registration_key(&metadata);
        let mut registrations = self.write()?;
        if registrations.contains_key(&key) {
            return Err(ProviderRegistryError::new(
                ProviderRegistryFailure::DuplicateRegistration,
                Some(metadata.provider_id.as_str()),
                "provider ID is already registered; use explicit replace",
            ));
        }
        registrations.insert(key, registration);
        Ok(metadata)
    }

    /// Explicitly replaces an existing registration and returns its old metadata.
    pub fn replace(
        &self,
        registration: ProviderRegistration,
    ) -> Result<ProviderRegistrationMetadata, ProviderRegistryError> {
        let provider_id = registration.metadata.provider_id.clone();
        let key = registration_key(&registration.metadata);
        let mut registrations = self.write()?;
        if !registrations.contains_key(&key) {
            return Err(ProviderRegistryError::new(
                ProviderRegistryFailure::RegistrationNotFound,
                Some(provider_id.as_str()),
                "provider must be registered before it can be replaced",
            ));
        }
        let previous = registrations
            .insert(key, registration)
            .ok_or_else(state_unavailable)?;
        Ok(previous.metadata)
    }

    /// Returns value-free metadata for an exact normalized provider ID.
    pub fn get(
        &self,
        provider_id: &ProviderId,
    ) -> Result<Option<ProviderRegistrationMetadata>, ProviderRegistryError> {
        let registrations = self.read()?;
        if let Some(registration) = registrations.get(&RegistrationKey {
            provider_id: provider_id.clone(),
            product_id: None,
        }) {
            return Ok(Some(registration.metadata.clone()));
        }
        let mut matching = registrations
            .iter()
            .filter(|(key, _)| &key.provider_id == provider_id)
            .map(|(_, registration)| registration.metadata.clone());
        let first = matching.next();
        Ok(if matching.next().is_none() {
            first
        } else {
            None
        })
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

    /// Returns metadata for one exact static provider product.
    pub fn get_product(
        &self,
        provider_id: &ProviderId,
        product_id: &ProductId,
    ) -> Result<Option<ProviderRegistrationMetadata>, ProviderRegistryError> {
        Ok(self
            .read()?
            .get(&RegistrationKey {
                provider_id: provider_id.clone(),
                product_id: Some(product_id.clone()),
            })
            .map(|registration| registration.metadata.clone()))
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
    ///
    /// Mirrors [`Self::get`]: a provider with exactly one registered product is
    /// removable by provider ID alone; a provider with several requires
    /// [`Self::remove_product`].
    pub fn remove(
        &self,
        provider_id: &ProviderId,
    ) -> Result<Option<ProviderRegistrationMetadata>, ProviderRegistryError> {
        let mut registrations = self.write()?;
        let mut matching = registrations
            .keys()
            .filter(|key| &key.provider_id == provider_id)
            .cloned();
        let Some(key) = matching.next() else {
            return Ok(None);
        };
        if matching.next().is_some() {
            return Ok(None);
        }
        Ok(registrations
            .remove(&key)
            .map(|registration| registration.metadata))
    }

    /// Removes one static product registration without affecting built runtimes.
    pub fn remove_product(
        &self,
        provider_id: &ProviderId,
        product_id: &ProductId,
    ) -> Result<Option<ProviderRegistrationMetadata>, ProviderRegistryError> {
        Ok(self
            .write()?
            .remove(&RegistrationKey {
                provider_id: provider_id.clone(),
                product_id: Some(product_id.clone()),
            })
            .map(|registration| registration.metadata))
    }

    /// Builds a runtime from a static definition and deployment configuration.
    pub fn build_deployment(
        &self,
        provider_id: &ProviderId,
        deployment: &ProviderDeploymentConfig,
        resolver: &dyn SecretResolver,
    ) -> Result<ProviderRuntime, LlmError> {
        if deployment.provider_id() != provider_id {
            return Err(ProviderRegistryError::new(
                ProviderRegistryFailure::FactoryProviderMismatch,
                Some(provider_id.as_str()),
                "deployment provider identity does not match registry selection",
            )
            .into());
        }
        let factory = {
            let registrations = self.read()?;
            let mut matching = registrations
                .iter()
                .filter(|(key, _)| &key.provider_id == provider_id && key.product_id.is_some())
                .map(|(_, registration)| registration);
            let registration = matching.next().ok_or_else(|| {
                ProviderRegistryError::new(
                    ProviderRegistryFailure::RegistrationNotFound,
                    Some(provider_id.as_str()),
                    "provider ID is not registered",
                )
            })?;
            if matching.next().is_some() {
                return Err(LlmError::Configuration(
                    "provider has multiple products; select one with build_product_deployment"
                        .to_owned(),
                ));
            }
            registration.compiler.as_ref().clone()
        };
        let definition = factory.definition().clone();
        let runtime = factory.build_deployment(deployment, resolver)?;
        if runtime.provider_id() != definition.provider_id()
            || runtime.product_id() != definition.product_id()
            || runtime.protocol_id() != definition.protocol_id()
        {
            return Err(ProviderRegistryError::new(
                ProviderRegistryFailure::FactoryProviderMismatch,
                Some(provider_id.as_str()),
                "definition compiler returned mismatched runtime identity",
            )
            .into());
        }
        Ok(runtime)
    }

    /// Builds one explicitly selected product from a static definition.
    pub fn build_product_deployment(
        &self,
        provider_id: &ProviderId,
        product_id: &ProductId,
        deployment: &ProviderDeploymentConfig,
        resolver: &dyn SecretResolver,
    ) -> Result<ProviderRuntime, LlmError> {
        if deployment.provider_id() != provider_id {
            return Err(ProviderRegistryError::new(
                ProviderRegistryFailure::FactoryProviderMismatch,
                Some(provider_id.as_str()),
                "deployment provider identity does not match registry selection",
            )
            .into());
        }
        let factory = {
            let registrations = self.read()?;
            let key = RegistrationKey {
                provider_id: provider_id.clone(),
                product_id: Some(product_id.clone()),
            };
            let registration = registrations.get(&key).ok_or_else(|| {
                ProviderRegistryError::new(
                    ProviderRegistryFailure::RegistrationNotFound,
                    Some(provider_id.as_str()),
                    "provider product is not registered",
                )
            })?;
            registration.compiler.as_ref().clone()
        };
        let runtime = factory.build_deployment(deployment, resolver)?;
        if runtime.provider_id() != provider_id || runtime.product_id() != product_id {
            return Err(ProviderRegistryError::new(
                ProviderRegistryFailure::FactoryProviderMismatch,
                Some(provider_id.as_str()),
                "definition compiler returned mismatched runtime identity",
            )
            .into());
        }
        Ok(runtime)
    }

    fn read(
        &self,
    ) -> Result<
        RwLockReadGuard<'_, BTreeMap<RegistrationKey, ProviderRegistration>>,
        ProviderRegistryError,
    > {
        self.registrations.read().map_err(|_| state_unavailable())
    }

    fn write(
        &self,
    ) -> Result<
        RwLockWriteGuard<'_, BTreeMap<RegistrationKey, ProviderRegistration>>,
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

fn registration_key(metadata: &ProviderRegistrationMetadata) -> RegistrationKey {
    RegistrationKey {
        provider_id: metadata.provider_id.clone(),
        product_id: metadata.product_id.clone(),
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

/// The built-in official definitions are assembled from compiled-in constants,
/// so this can only fire if one of those constants stops being valid.
fn official_definition_unavailable(_: LlmError) -> ProviderRegistryError {
    ProviderRegistryError::new(
        ProviderRegistryFailure::InvalidProviderId,
        None,
        "built-in official definition failed to compile",
    )
}

fn state_unavailable() -> ProviderRegistryError {
    ProviderRegistryError::new(
        ProviderRegistryFailure::StateUnavailable,
        None,
        "provider registry synchronization state is unavailable",
    )
}
