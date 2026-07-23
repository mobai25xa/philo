//! Versioned provider configuration, deterministic merge, and secret references.

mod merge;
mod schema;
mod secret_ref;
mod source;
mod validate;

pub use merge::{
    ListMerge, MapMerge, NamedConfigValue, NamedListMerge, ProviderConfigField,
    ProviderConfigLayer, ProviderConfigSnapshot,
};
pub use schema::{
    ClientIdentityConfig, ConfigSchemaVersion, ConfigValue, CredentialAudienceSpec, EndpointSpec,
    ProviderConfigDocument, SecretReference,
};
pub use secret_ref::{EnvironmentSecretResolver, SecretResolver};
pub use source::{
    ConfigSource, ConfigSourceId, ConfigSourceKind, ConfigSourceLocation, FieldProvenance,
    FieldState,
};
