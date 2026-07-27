//! Exact model catalog, capabilities, limits, pricing, and provenance.

mod entry;
mod ids;
mod merge;
mod source;
mod validate;

/// Application-facing stable model identifier used by catalog keys.
pub use crate::domain::ModelId as DomainModelId;
pub use entry::{CatalogCapabilities, CatalogDefaults, ModelEntry, ModelKey, ModelLimits};
pub use ids::{CatalogSourceId, DeploymentId, ProductId, ProviderModelId, WireModelValue};
pub use merge::ModelCatalog;
pub use source::CatalogSource;
pub use validate::validate_entry;
