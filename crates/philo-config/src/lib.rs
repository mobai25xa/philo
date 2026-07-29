//! Versioned, layered provider configuration for the philo SDK core.
//!
//! # Why this is not in the core
//!
//! Configuration loading, layered merge, source provenance, and a versioned
//! document schema decide *how convenient* a deployment is to configure. When
//! they are wrong the result is "it will not start", not "it sent an illegal
//! request", "it returned the wrong answer", or "it billed inaccurately". By the
//! FR-000 criterion that puts them outside the core, and the caller ecosystem
//! already has figment, config-rs, and plain serde for the same job.
//!
//! What stayed in the core is the part that *is* a safety mechanism:
//! [`SecretReference`](philo::provider::secret::SecretReference) and
//! [`SecretResolver`](philo::provider::secret::SecretResolver), so the core can
//! guarantee a credential is a reference rather than a value it might log.
//!
//! # The one shape this crate produces
//!
//! ```text
//! multi-source config (file / env / code)
//!     -> philo-config merge + validate
//!     -> (ProviderDefinition, ProviderDeploymentConfig)
//!     -> philo core compiles a ProviderRuntime
//! ```
//!
//! The core has exactly one construction path and does not know this crate
//! exists.
//!
//! # Stability
//!
//! This companion crate is **Experimental** during the 0.x series. Its schema,
//! merge, provenance, and migration rules follow `COMPATIBILITY.md`, but they
//! are not part of the core crate's 1.0 Stable surface.

mod merge;
mod schema;
mod source;
mod validate;

pub use merge::{ProviderConfigField, ProviderConfigLayer, ProviderConfigSnapshot};
pub use schema::{
    ClientIdentityConfig, ConfigSchemaVersion, ConfigValue, CredentialAudienceSpec, EndpointSpec,
    ProviderConfigDocument,
};
pub use source::{
    ConfigSource, ConfigSourceId, ConfigSourceKind, ConfigSourceLocation, FieldProvenance,
    FieldState,
};
