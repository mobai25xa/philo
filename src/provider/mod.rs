//! Provider profiles, endpoint policy, header resolution, authentication, and runtime.

pub mod auth;
pub mod capability;
pub mod endpoint;
pub mod headers;
pub mod profile;
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
#[doc(hidden)]
pub use profile::TestOnlyProfile;
pub use profile::{OfficialOpenAiProfile, ProviderProfile};
pub use runtime::ProviderRuntime;
