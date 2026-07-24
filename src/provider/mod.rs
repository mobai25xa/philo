//! Provider profiles, endpoint policy, header resolution, authentication, and runtime.

pub mod auth;
pub(crate) mod call_policy;
pub mod capability;
pub mod catalog;
pub mod compat;
pub mod config;
pub mod endpoint;
pub mod factory;
pub mod headers;
pub mod profile;
mod profiles;
pub mod registry;
pub mod runtime;

pub use auth::{
    ApiKey, ApiKeyHeaderAuth, AuthContext, AuthProvider, BearerAuth, BearerCredential,
    CredentialFuture, CredentialIdentity, DynamicAuth, DynamicCredential, DynamicCredentialCache,
    DynamicCredentialContext, DynamicCredentialScheme, DynamicCredentialSource, MultiHeaderAuth,
    NoAuth, TenantId,
};
pub use capability::{
    ModelCapabilityProfile, OFFICIAL_OPENAI_CAPABILITY_REVIEW_DATE, ProtocolDialect,
    ProviderCapabilities, ProviderTransportOptions,
};
pub use catalog::{
    CatalogCapabilities, CatalogDefaults, CatalogSource, CatalogSourceId, DeploymentId,
    ModelCatalog, ModelEntry, ModelKey, ModelLimits, ProductId, ProviderModelId, SupportStatus,
    WireModelValue,
};
pub use compat::{
    CompatField, CompatPatch, CompatProfile, FinishReasonCompat, HistoryCompat, InlineErrorCompat,
    MaxOutputTokensWireFormat, RequestCompat, ResponseCompat, ToolArgumentsCompat, UsageCompat,
    resolve_compat, validate_compat,
};
pub use config::{
    ClientIdentityConfig, ConfigSchemaVersion, ConfigSource, ConfigSourceId, ConfigSourceKind,
    ConfigSourceLocation, ConfigValue, CredentialAudienceSpec, EndpointSpec,
    EnvironmentSecretResolver, FieldProvenance, FieldState, ListMerge, MapMerge, NamedConfigValue,
    NamedListMerge, ProviderConfigDocument, ProviderConfigField, ProviderConfigLayer,
    ProviderConfigSnapshot, SecretReference, SecretResolver,
};
pub use endpoint::{CredentialAudience, EndpointConfig, Origin, RedirectPolicy, ResolvedEndpoint};
pub use factory::{OfficialOpenAiFactory, ProviderRuntimeFactory};
pub use headers::{
    ClientIdentity, ClientIdentityFragment, DynamicHeaderContext, DynamicHeaderFuture,
    DynamicHeaderPolicy, DynamicHeaderSource, DynamicResponseFormat, HeaderLayer, HeaderOperation,
    HeaderPipeline, HeaderPolicy, HeaderSource, HeaderTraceEntry, ResolvedHeaders,
    SensitiveHeaderValue, TraceDecision, TraceOperation,
};
pub use profile::ProviderProfile;
pub use profiles::OfficialOpenAiProfile;
#[doc(hidden)]
pub use profiles::TestOnlyProfile;
pub use registry::{ProviderRegistration, ProviderRegistrationMetadata, ProviderRegistry};
pub use runtime::ProviderRuntime;
