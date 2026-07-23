//! Provider profiles, endpoint policy, header resolution, authentication, and runtime.

pub mod auth;
pub(crate) mod call_policy;
pub mod capability;
pub mod endpoint;
pub mod headers;
pub mod profile;
mod profiles;
pub mod runtime;

pub use auth::{ApiKey, AuthContext, AuthProvider, BearerAuth, BearerCredential, ClientIdentity};
pub use capability::{
    ModelCapabilityProfile, OFFICIAL_OPENAI_CAPABILITY_REVIEW_DATE, ProtocolDialect,
    ProviderCapabilities, ProviderTransportOptions,
};
pub use endpoint::{CredentialAudience, EndpointConfig, Origin, RedirectPolicy, ResolvedEndpoint};
pub use headers::{
    HeaderLayer, HeaderOperation, HeaderPipeline, HeaderPolicy, HeaderSource, HeaderTraceEntry,
    ResolvedHeaders, SensitiveHeaderValue, TraceDecision, TraceOperation,
};
pub use profile::ProviderProfile;
pub use profiles::OfficialOpenAiProfile;
#[doc(hidden)]
pub use profiles::TestOnlyProfile;
pub use runtime::ProviderRuntime;
