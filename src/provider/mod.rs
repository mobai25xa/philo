//! Provider profiles, endpoint policy, header resolution, authentication, and runtime.

pub mod auth;
pub mod capability;
pub mod catalog;

pub mod definition;
pub mod endpoint;
pub mod factory;
pub mod headers;
pub mod idempotency;
pub mod profile;
pub mod profiles;
pub mod protocol_contract;
pub mod rate_limit;
pub mod registry;
pub mod runtime;
pub mod secret;

pub use auth::{
    ApiKey, ApiKeyHeaderAuth, AuthContext, AuthProvider, AuthSchemeKind, BearerAuth,
    BearerCredential, CredentialFuture, CredentialIdentity, CredentialSourceKind, DynamicAuth,
    DynamicCredential, DynamicCredentialCache, DynamicCredentialContext, DynamicCredentialScheme,
    DynamicCredentialSource, MultiHeaderAuth, NoAuth, TenantId,
};
pub use capability::{
    ModelCapabilityProfile, OFFICIAL_ANTHROPIC_CAPABILITY_REVIEW_DATE,
    OFFICIAL_OPENAI_CAPABILITY_REVIEW_DATE, ProtocolDialect, ProviderCapabilities,
    ProviderTransportOptions,
};
pub use catalog::{
    CatalogCapabilities, CatalogDefaults, CatalogSource, CatalogSourceId, DeploymentId,
    DomainModelId, ModelCatalog, ModelEntry, ModelKey, ModelLimits, ProductId, ProviderModelId,
    WireModelValue,
};
pub use definition::{
    AuthScheme, ProviderDefinition, ProviderDefinitionBuilder, ProviderDeploymentConfig,
};
pub use endpoint::{
    CredentialAudience, CredentialBinding, EndpointConfig, EndpointNetworkPolicy,
    EndpointPathVariable, EndpointQuery, EndpointQueryAction, EndpointQueryDiagnostic,
    EndpointQuerySource, EndpointResolutionDiagnostics, EndpointTemplate, EndpointValues, Origin,
    QueryMergeRule, RedirectPolicy, ResolvedEndpoint, ResolvedModelMapping,
};
pub use factory::{
    ProviderSelection, ProviderSelectionInput, ProviderSelectionSource, ProviderSelector,
    StaticProviderFactory,
};
pub use headers::{
    ClientIdentity, ClientIdentityFragment, DynamicHeaderContext, DynamicHeaderFuture,
    DynamicHeaderPolicy, DynamicHeaderSource, DynamicResponseFormat, HeaderLayer, HeaderOperation,
    HeaderPipeline, HeaderPolicy, HeaderSource, ResolvedHeaders, SensitiveHeaderValue,
};
pub(crate) use idempotency::ResolvedIdempotency;
pub use idempotency::{
    IdempotencyCapability, IdempotencyKey, IdempotencyKeySource, IdempotencyPolicy,
};
pub use profile::ProviderProfile;
#[doc(hidden)]
#[cfg(feature = "test-util")]
pub use profiles::TestOnlyProfile;
pub use profiles::{
    OFFICIAL_ANTHROPIC_API_VERSION, OfficialAnthropicProfile, OfficialOpenAiProfile,
};
pub(crate) use protocol_contract::{
    AnthropicMessagesContract, OpenAiChatContract, ResolvedProtocolContract,
};
pub use protocol_contract::{
    AnthropicUsageCompat, CompatField, CompatProfile, FinishReasonCompat, HistoryCompat,
    InlineErrorCompat, MaxOutputTokensWireFormat, ModelBodyWireFormat, RequestCompat,
    ResponseCompat, ToolArgumentsCompat, UsageCompat,
};
pub(crate) use rate_limit::observe_rate_limit;
pub use rate_limit::{
    RateLimitHeaderKind, RateLimitHeaderSpec, RateLimitObservation, RateLimitPolicy,
    RateLimitQuota, RateLimitReset, RateLimitSourceKind, RateLimitUnit, RateLimitValue,
};
pub use registry::{ProviderRegistration, ProviderRegistrationMetadata, ProviderRegistry};
pub use runtime::ProviderRuntime;
pub use secret::{EnvironmentSecretResolver, SecretReference, SecretResolver};
