//! Endpoint configuration, typed templates, model mapping, and destination policy.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

mod audience;
mod config;
mod mapping;
mod origin;
mod policy;
mod template;

pub use audience::CredentialAudience;
pub use config::{EndpointConfig, resolve_official, resolve_official_for, resolve_test_only};
pub use mapping::ResolvedModelMapping;
pub use origin::{EndpointResolutionDiagnostics, Origin, ResolvedEndpoint};
pub use policy::{EndpointNetworkPolicy, RedirectPolicy};
pub use template::{
    EndpointPathVariable, EndpointQuery, EndpointQueryAction, EndpointQueryDiagnostic,
    EndpointQuerySource, EndpointTemplate, EndpointValues, QueryMergeRule,
};

pub(crate) use config::{EndpointMode, resolve_test_only_for};
