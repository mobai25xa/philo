//! Provider profiles, endpoint policy, header resolution, authentication, and runtime.

pub mod auth;
pub(crate) mod call_policy;
pub mod capability;
pub mod catalog;
pub mod compat;
pub mod config;
mod definition;
pub mod detection;
pub mod diagnostics;
pub mod endpoint;
pub mod factory;
pub mod headers;
mod idempotency;
pub mod profile;
mod profiles;
mod protocol_contract;
mod rate_limit;
pub mod registry;
pub mod runtime;

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
    SupportStatus, WireModelValue,
};
pub use compat::{
    AnthropicUsageCompat, CompatField, CompatPatch, CompatProfile, ConstraintStrength,
    DataRetention, FallbackDimension, FinishReasonCompat, HistoryCompat, InlineErrorCompat,
    MaxOutputTokensWireFormat, ModelBodyWireFormat, OpenRouterRoutingContract,
    OpenRouterRoutingPatch, ProviderRequestOptions, RequestCompat, ResolvedProviderRouting,
    ResponseCompat, RoutingFallback, RoutingField, RoutingRegion, RoutingSort, ToolArgumentsCompat,
    UpstreamId, UsageCompat, resolve_compat, validate_compat,
};
pub use config::{
    ClientIdentityConfig, ConfigSchemaVersion, ConfigSource, ConfigSourceId, ConfigSourceKind,
    ConfigSourceLocation, ConfigValue, CredentialAudienceSpec, EndpointSpec,
    EnvironmentSecretResolver, FieldProvenance, FieldState, ListMerge, MapMerge, NamedConfigValue,
    NamedListMerge, ProviderConfigDocument, ProviderConfigField, ProviderConfigLayer,
    ProviderConfigSnapshot, SecretReference, SecretResolver,
};
pub use definition::{
    AuthScheme, ProviderDefinition, ProviderDefinitionBuilder, ProviderDeploymentConfig,
};
pub use detection::{
    DetectionConfidence, DetectionExplanation, DetectionSuggestion, DetectionUnknownReason,
    EndpointDetection, EndpointDetectionPolicy, EndpointDetector, NormalizedEndpointFacts,
};
pub use diagnostics::{
    AuthDiagnostics, CompatDiagnostic, EffectiveSupportStatus, EndpointDiagnostics,
    EvidenceVerification, HeaderDiagnostic, ProviderDiagnostics, SupportDiagnostics,
};
pub use endpoint::{
    CredentialAudience, CredentialBinding, EndpointConfig, EndpointNetworkPolicy,
    EndpointPathVariable, EndpointQuery, EndpointQueryAction, EndpointQueryDiagnostic,
    EndpointQuerySource, EndpointResolutionDiagnostics, EndpointTemplate, EndpointValues, Origin,
    QueryMergeRule, RedirectPolicy, ResolvedEndpoint, ResolvedModelMapping,
};
pub use factory::{
    OfficialAnthropicFactory, OfficialOpenAiFactory, ProviderRuntimeFactory, ProviderSelection,
    ProviderSelectionInput, ProviderSelectionSource, ProviderSelector, StaticProviderFactory,
};
pub use headers::{
    ClientIdentity, ClientIdentityFragment, DynamicHeaderContext, DynamicHeaderFuture,
    DynamicHeaderPolicy, DynamicHeaderSource, DynamicResponseFormat, HeaderLayer, HeaderOperation,
    HeaderPipeline, HeaderPolicy, HeaderSource, HeaderTraceEntry, ResolvedHeaders,
    SensitiveHeaderValue, TraceDecision, TraceOperation,
};
pub(crate) use idempotency::ResolvedIdempotency;
pub use idempotency::{
    IdempotencyCapability, IdempotencyKey, IdempotencyKeySource, IdempotencyPolicy,
};
pub use profile::ProviderProfile;
#[doc(hidden)]
pub use profiles::TestOnlyProfile;
pub use profiles::{
    DeepSeekProfile, OFFICIAL_ANTHROPIC_API_VERSION, OfficialAnthropicProfile,
    OfficialOpenAiProfile, OpenRouterAttribution, OpenRouterProfile, ZaiCodingProfile,
    ZaiStandardProfile,
};
pub(crate) use protocol_contract::{
    AnthropicMessagesContract, OpenAiChatContract, ResolvedProtocolContract,
};
pub(crate) use rate_limit::observe_rate_limit;
pub use rate_limit::{
    RateLimitHeaderKind, RateLimitHeaderSpec, RateLimitObservation, RateLimitPolicy,
    RateLimitQuota, RateLimitReset, RateLimitSourceKind, RateLimitUnit, RateLimitValue,
};
pub use registry::{ProviderRegistration, ProviderRegistrationMetadata, ProviderRegistry};
pub use runtime::ProviderRuntime;
