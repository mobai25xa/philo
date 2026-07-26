//! Validated provider definitions and deployment-owned runtime inputs.
#![allow(dead_code, clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::Arc;

use http::{HeaderName, HeaderValue, header};

use crate::domain::{ModelId, PolicySource, ProtocolId, ProviderId, ResourceLimits};
use crate::error::{LlmError, ValidationError, ValidationReason};
use crate::transport::SseConfig;

use super::auth::{
    ApiKeyHeaderAuth, AuthProvider, AuthSchemeKind, BearerAuth, BearerCredential, ClientIdentity,
    NoAuth,
};
use super::capability::{
    ModelCapabilityProfile, ProtocolDialect, ProviderCapabilities, ProviderTransportOptions,
};
use super::catalog::{ModelCatalog, ProductId};
use super::compat::{AnthropicUsageCompat, CompatPatch, OpenRouterRoutingContract};
use super::config::{SecretReference, SecretResolver};
use super::endpoint::{
    CredentialBinding, EndpointConfig, ResolvedModelMapping, resolve_official, resolve_official_for,
};
use super::headers::{
    DynamicHeaderPolicy, HeaderLayer, HeaderOperation, HeaderPipeline, HeaderSource,
};
use super::profile::{ProviderProfile, ProviderProfileParts};
use super::{IdempotencyPolicy, RateLimitPolicy, ResolvedProtocolContract};

/// Authentication shape fixed by a provider definition without containing a secret.
/// Validated, secret-free authentication shape for a provider definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthScheme(AuthSchemeInner);

#[derive(Clone, Debug, Eq, PartialEq)]
enum AuthSchemeInner {
    Bearer,
    ApiKeyHeader(HeaderName),
    Custom {
        kind: AuthSchemeKind,
        protected_headers: Vec<HeaderName>,
    },
    None,
}

impl AuthScheme {
    /// Declares Bearer authentication.
    pub const fn bearer() -> Self {
        Self(AuthSchemeInner::Bearer)
    }

    /// Declares one validated API-key header.
    pub fn api_key_header(name: HeaderName) -> Result<Self, LlmError> {
        validate_auth_header_name(&name)?;
        Ok(Self(AuthSchemeInner::ApiKeyHeader(name)))
    }

    /// Declares explicit unauthenticated operation.
    pub const fn none() -> Self {
        Self(AuthSchemeInner::None)
    }

    /// Captures the value-free authentication shape of an extensible provider.
    pub fn from_auth_provider(auth: &dyn AuthProvider) -> Result<Self, LlmError> {
        match auth.scheme_kind() {
            AuthSchemeKind::Bearer => Ok(Self(AuthSchemeInner::Bearer)),
            AuthSchemeKind::ApiKeyHeader => {
                let headers = auth.protected_headers();
                if headers.len() != 1 {
                    return Err(configuration(
                        "API-key authentication must protect exactly one header",
                    ));
                }
                Self::api_key_header(headers[0].clone())
            }
            AuthSchemeKind::None => Ok(Self(AuthSchemeInner::None)),
            kind => {
                let protected_headers = auth.protected_headers();
                if protected_headers.is_empty() {
                    return Err(configuration(
                        "custom authentication must declare protected headers",
                    ));
                }
                for name in &protected_headers {
                    validate_auth_header_name(name)?;
                }
                if header_set(protected_headers.clone()).len() != protected_headers.len() {
                    return Err(configuration(
                        "custom authentication protected headers must be unique",
                    ));
                }
                Ok(Self(AuthSchemeInner::Custom {
                    kind,
                    protected_headers,
                }))
            }
        }
    }

    fn kind(&self) -> AuthSchemeKind {
        match &self.0 {
            AuthSchemeInner::Bearer => AuthSchemeKind::Bearer,
            AuthSchemeInner::ApiKeyHeader(_) => AuthSchemeKind::ApiKeyHeader,
            AuthSchemeInner::Custom { kind, .. } => *kind,
            AuthSchemeInner::None => AuthSchemeKind::None,
        }
    }

    fn protected_headers(&self) -> Vec<HeaderName> {
        match &self.0 {
            AuthSchemeInner::Bearer => vec![header::AUTHORIZATION],
            AuthSchemeInner::ApiKeyHeader(name) => vec![name.clone()],
            AuthSchemeInner::Custom {
                protected_headers, ..
            } => protected_headers.clone(),
            AuthSchemeInner::None => Vec::new(),
        }
    }
}

/// Trusted, secret-free declaration of one provider product and protocol.
#[derive(Clone)]
pub struct ProviderDefinition {
    provider_id: ProviderId,
    product_id: ProductId,
    protocol_id: ProtocolId,
    dialect: ProtocolDialect,
    protocol_contract: ResolvedProtocolContract,
    endpoint: EndpointConfig,
    credential_binding: CredentialBinding,
    auth_scheme: AuthScheme,
    provider_headers: Vec<HeaderOperation>,
    model_headers: Vec<HeaderOperation>,
    dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
    capabilities: ProviderCapabilities,
    model_capabilities: BTreeMap<ModelId, ModelCapabilityProfile>,
    catalog: ModelCatalog,
    provider_compat: CompatPatch,
    model_compat: BTreeMap<ModelId, CompatPatch>,
    openrouter_routing: Option<OpenRouterRoutingContract>,
    transport: ProviderTransportOptions,
    rate_limit: RateLimitPolicy,
    idempotency: IdempotencyPolicy,
}

impl ProviderDefinition {
    /// Starts a definition for the `OpenAI` Chat Completions protocol.
    ///
    /// # Panics
    ///
    /// This function only constructs a compile-time protocol identifier; the
    /// invariant would panic only if that static identifier became invalid.
    pub fn openai_chat(
        provider_id: ProviderId,
        product_id: ProductId,
    ) -> ProviderDefinitionBuilder {
        ProviderDefinitionBuilder::new(
            provider_id,
            product_id,
            ProtocolId::new("openai-chat-completions").expect("static protocol ID is valid"),
            ProtocolDialect::OpenAiChatCompletions,
            ResolvedProtocolContract::strict_openai_chat(),
        )
    }

    /// Starts a definition for the Anthropic Messages protocol.
    ///
    /// # Panics
    ///
    /// This function only constructs a compile-time protocol identifier; the
    /// invariant would panic only if that static identifier became invalid.
    pub fn anthropic_messages(
        provider_id: ProviderId,
        product_id: ProductId,
    ) -> ProviderDefinitionBuilder {
        ProviderDefinitionBuilder::new(
            provider_id,
            product_id,
            ProtocolId::new("anthropic-messages").expect("static protocol ID is valid"),
            ProtocolDialect::AnthropicMessages,
            ResolvedProtocolContract::strict_anthropic_messages(),
        )
    }

    /// Returns the fixed provider identity.
    pub const fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    /// Returns the fixed product identity.
    pub const fn product_id(&self) -> &ProductId {
        &self.product_id
    }

    /// Returns the explicitly selected protocol identity.
    pub const fn protocol_id(&self) -> &ProtocolId {
        &self.protocol_id
    }

    /// Returns the endpoint-bound credential restriction.
    pub const fn credential_binding(&self) -> &CredentialBinding {
        &self.credential_binding
    }

    pub(crate) fn compile_resolved(
        &self,
        deployment: ResolvedProviderDeployment,
    ) -> Result<ProviderProfile, LlmError> {
        validate_deployment_limits(&deployment)?;
        if deployment.auth.scheme_kind() != self.auth_scheme.kind() {
            return Err(configuration(
                "deployment authentication scheme does not match provider definition",
            ));
        }
        let expected_headers = self.auth_scheme.protected_headers();
        if header_set(deployment.auth.protected_headers()) != header_set(expected_headers) {
            return Err(configuration(
                "deployment authentication headers do not match provider definition",
            ));
        }
        let Some(auth_binding) = deployment.auth.credential_binding() else {
            if !matches!(self.auth_scheme.0, AuthSchemeInner::None) {
                return Err(configuration(
                    "credential-bearing deployment must expose its destination binding",
                ));
            }
            return self.compile_profile(deployment);
        };
        if auth_binding != &self.credential_binding {
            return Err(configuration(
                "deployment credential binding does not match provider definition",
            ));
        }
        self.compile_profile(deployment)
    }

    /// Resolves one deployment credential and compiles an immutable profile.
    pub fn compile(
        &self,
        deployment: &ProviderDeploymentConfig,
        resolver: &dyn SecretResolver,
    ) -> Result<ProviderProfile, LlmError> {
        if deployment.provider_id != self.provider_id {
            return Err(configuration(
                "deployment provider identity does not match provider definition",
            ));
        }
        let auth = match &deployment.credential {
            DeploymentCredential::Reference(reference) => {
                let key = resolver.resolve(reference)?;
                match &self.auth_scheme.0 {
                    AuthSchemeInner::Bearer => Arc::new(BearerAuth::new(BearerCredential::new(
                        key,
                        self.credential_binding.clone(),
                    ))) as Arc<dyn AuthProvider>,
                    AuthSchemeInner::ApiKeyHeader(name) => Arc::new(ApiKeyHeaderAuth::new(
                        name.clone(),
                        key,
                        self.credential_binding.clone(),
                    )?)
                        as Arc<dyn AuthProvider>,
                    AuthSchemeInner::Custom { .. } => {
                        return Err(configuration(
                            "custom authentication requires a deployment auth provider",
                        ));
                    }
                    AuthSchemeInner::None => {
                        return Err(configuration(
                            "credential reference cannot be used with unauthenticated definition",
                        ));
                    }
                }
            }
            DeploymentCredential::AuthProvider(auth) => Arc::clone(auth),
            DeploymentCredential::None => {
                if !matches!(self.auth_scheme.0, AuthSchemeInner::None) {
                    return Err(configuration(
                        "credential-bearing definition requires deployment authentication",
                    ));
                }
                Arc::new(NoAuth) as Arc<dyn AuthProvider>
            }
        };
        self.compile_resolved(
            ResolvedProviderDeployment::new(auth, deployment.client_identity.clone())
                .with_resource_limits(deployment.resource_limits)
                .with_sse_config(deployment.sse)
                .with_max_http_error_body_bytes(deployment.max_http_error_body_bytes)?,
        )
    }

    fn compile_profile(
        &self,
        deployment: ResolvedProviderDeployment,
    ) -> Result<ProviderProfile, LlmError> {
        ProviderProfile::from_parts(ProviderProfileParts {
            provider_id: self.provider_id.clone(),
            product_id: self.product_id.clone(),
            protocol_id: self.protocol_id.clone(),
            endpoint: self.endpoint.clone(),
            credential_binding: self.credential_binding.clone(),
            auth: deployment.auth,
            client_identity: deployment.client_identity,
            provider_headers: self.provider_headers.clone(),
            model_headers: self.model_headers.clone(),
            dynamic_header_policy: self.dynamic_header_policy.clone(),
            capabilities: self.capabilities.clone(),
            model_capabilities: self.model_capabilities.clone(),
            catalog: self.catalog.clone(),
            provider_compat: self.provider_compat.clone(),
            model_compat: self.model_compat.clone(),
            openrouter_routing: self.openrouter_routing.clone(),
            dialect: self.dialect,
            protocol_contract: self.protocol_contract.clone(),
            transport: self.transport,
            resource_limits: deployment.resource_limits,
            sse: deployment.sse,
            max_http_error_body_bytes: deployment.max_http_error_body_bytes,
            rate_limit: self.rate_limit.clone(),
            idempotency: self.idempotency.clone(),
            test_only: false,
        })
    }
}

impl fmt::Debug for ProviderDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let provider_headers = self
            .provider_headers
            .iter()
            .map(HeaderOperation::name)
            .collect::<Vec<_>>();
        let model_headers = self
            .model_headers
            .iter()
            .map(HeaderOperation::name)
            .collect::<Vec<_>>();
        formatter
            .debug_struct("ProviderDefinition")
            .field("provider_id", &self.provider_id)
            .field("product_id", &self.product_id)
            .field("protocol_id", &self.protocol_id)
            .field("protocol_contract", &self.protocol_contract)
            .field("endpoint", &self.endpoint)
            .field("credential_binding", &self.credential_binding)
            .field("auth_scheme", &self.auth_scheme)
            .field("provider_header_names", &provider_headers)
            .field("model_header_names", &model_headers)
            .field("capabilities", &self.capabilities)
            .field("catalog_entries", &self.catalog.entries().count())
            .finish_non_exhaustive()
    }
}

enum DeploymentCredential {
    Reference(SecretReference),
    AuthProvider(Arc<dyn AuthProvider>),
    None,
}

/// Deployment-owned credential reference and bounded runtime resources.
pub struct ProviderDeploymentConfig {
    provider_id: ProviderId,
    credential: DeploymentCredential,
    client_identity: ClientIdentity,
    resource_limits: ResourceLimits,
    sse: SseConfig,
    max_http_error_body_bytes: usize,
}

impl ProviderDeploymentConfig {
    /// Creates a deployment that resolves exactly one secret during compilation.
    pub fn new(provider_id: ProviderId, credential: SecretReference) -> Self {
        Self {
            provider_id,
            credential: DeploymentCredential::Reference(credential),
            client_identity: ClientIdentity::default(),
            resource_limits: ResourceLimits::default(),
            sse: SseConfig::default(),
            max_http_error_body_bytes: 16 * 1024,
        }
    }

    /// Creates a deployment backed by an extensible authentication provider.
    pub fn with_auth_provider(provider_id: ProviderId, auth: Arc<dyn AuthProvider>) -> Self {
        Self {
            provider_id,
            credential: DeploymentCredential::AuthProvider(auth),
            client_identity: ClientIdentity::default(),
            resource_limits: ResourceLimits::default(),
            sse: SseConfig::default(),
            max_http_error_body_bytes: 16 * 1024,
        }
    }

    /// Creates an explicitly unauthenticated deployment.
    pub fn without_auth(provider_id: ProviderId) -> Self {
        Self {
            provider_id,
            credential: DeploymentCredential::None,
            client_identity: ClientIdentity::default(),
            resource_limits: ResourceLimits::default(),
            sse: SseConfig::default(),
            max_http_error_body_bytes: 16 * 1024,
        }
    }

    /// Replaces the truthful client identity.
    #[must_use]
    pub fn with_client_identity(mut self, identity: ClientIdentity) -> Self {
        self.client_identity = identity;
        self
    }

    /// Replaces SDK-local request and response safety ceilings.
    #[must_use]
    pub fn with_resource_limits(mut self, limits: ResourceLimits) -> Self {
        self.resource_limits = limits;
        self
    }

    /// Replaces Server-Sent Events framing ceilings.
    #[must_use]
    pub fn with_sse_config(mut self, sse: SseConfig) -> Self {
        self.sse = sse;
        self
    }

    /// Replaces the bounded HTTP error-body prefix size.
    pub fn with_max_http_error_body_bytes(mut self, limit: usize) -> Result<Self, LlmError> {
        if limit == 0 {
            return Err(configuration("HTTP error body limit must be positive"));
        }
        self.max_http_error_body_bytes = limit;
        Ok(self)
    }

    /// Returns the provider provenance fixed by this deployment.
    pub const fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }
}

impl fmt::Debug for ProviderDeploymentConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let credential_kind = match self.credential {
            DeploymentCredential::Reference(_) => "secret-reference",
            DeploymentCredential::AuthProvider(_) => "auth-provider",
            DeploymentCredential::None => "none",
        };
        formatter
            .debug_struct("ProviderDeploymentConfig")
            .field("provider_id", &self.provider_id)
            .field("credential", &credential_kind)
            .field("client_identity", &self.client_identity)
            .field("resource_limits", &self.resource_limits)
            .field("sse", &self.sse)
            .field("max_http_error_body_bytes", &self.max_http_error_body_bytes)
            .finish()
    }
}

/// Deployment-owned credential and bounded runtime resources.
pub(crate) struct ResolvedProviderDeployment {
    auth: Arc<dyn AuthProvider>,
    client_identity: ClientIdentity,
    resource_limits: ResourceLimits,
    sse: SseConfig,
    max_http_error_body_bytes: usize,
}

impl fmt::Debug for ResolvedProviderDeployment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedProviderDeployment")
            .field("auth", &"[REDACTED]")
            .field("client_identity", &self.client_identity)
            .field("resource_limits", &self.resource_limits)
            .field("sse", &self.sse)
            .field("max_http_error_body_bytes", &self.max_http_error_body_bytes)
            .finish()
    }
}

impl ResolvedProviderDeployment {
    pub(crate) fn new(auth: Arc<dyn AuthProvider>, client_identity: ClientIdentity) -> Self {
        Self {
            auth,
            client_identity,
            resource_limits: ResourceLimits::default(),
            sse: SseConfig::default(),
            max_http_error_body_bytes: 16 * 1024,
        }
    }

    pub(crate) fn with_resource_limits(mut self, limits: ResourceLimits) -> Self {
        self.resource_limits = limits;
        self
    }

    pub(crate) fn with_sse_config(mut self, sse: SseConfig) -> Self {
        self.sse = sse;
        self
    }

    pub(crate) fn with_max_http_error_body_bytes(mut self, limit: usize) -> Result<Self, LlmError> {
        if limit == 0 {
            return Err(configuration("HTTP error body limit must be positive"));
        }
        self.max_http_error_body_bytes = limit;
        Ok(self)
    }
}

/// Protocol-specific, fail-closed builder for a provider definition.
pub struct ProviderDefinitionBuilder {
    provider_id: ProviderId,
    product_id: ProductId,
    protocol_id: ProtocolId,
    dialect: ProtocolDialect,
    protocol_contract: ResolvedProtocolContract,
    endpoint: Option<EndpointConfig>,
    credential_binding: Option<CredentialBinding>,
    bind_to_endpoint_origin: bool,
    auth_scheme: Option<AuthScheme>,
    provider_headers: Vec<HeaderOperation>,
    model_headers: Vec<HeaderOperation>,
    dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
    capabilities: Option<ProviderCapabilities>,
    model_capabilities: BTreeMap<ModelId, ModelCapabilityProfile>,
    catalog: Option<ModelCatalog>,
    allow_unregistered_models: bool,
    provider_compat: CompatPatch,
    model_compat: BTreeMap<ModelId, CompatPatch>,
    openrouter_routing: Option<OpenRouterRoutingContract>,
    transport: ProviderTransportOptions,
    rate_limit: RateLimitPolicy,
    idempotency: IdempotencyPolicy,
}

impl ProviderDefinitionBuilder {
    fn new(
        provider_id: ProviderId,
        product_id: ProductId,
        protocol_id: ProtocolId,
        dialect: ProtocolDialect,
        protocol_contract: ResolvedProtocolContract,
    ) -> Self {
        Self {
            provider_id,
            product_id,
            protocol_id,
            dialect,
            protocol_contract,
            endpoint: None,
            credential_binding: None,
            bind_to_endpoint_origin: false,
            auth_scheme: None,
            provider_headers: Vec::new(),
            model_headers: Vec::new(),
            dynamic_header_policy: None,
            capabilities: None,
            model_capabilities: BTreeMap::new(),
            catalog: None,
            allow_unregistered_models: false,
            provider_compat: CompatPatch::from_source(PolicySource::ProviderProfile),
            model_compat: BTreeMap::new(),
            openrouter_routing: None,
            transport: ProviderTransportOptions::secure_defaults(),
            rate_limit: RateLimitPolicy::standard_only(),
            idempotency: IdempotencyPolicy::unknown(),
        }
    }

    /// Fixes the production HTTPS endpoint.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: EndpointConfig) -> Self {
        self.endpoint = Some(endpoint);
        self
    }

    /// Fixes an explicit credential destination binding.
    #[must_use]
    pub fn with_credential_binding(mut self, binding: CredentialBinding) -> Self {
        self.credential_binding = Some(binding);
        self.bind_to_endpoint_origin = false;
        self
    }

    /// Derives an exact HTTPS-origin binding from the fixed endpoint.
    #[must_use]
    pub fn bind_credential_to_endpoint_origin(mut self) -> Self {
        self.credential_binding = None;
        self.bind_to_endpoint_origin = true;
        self
    }

    /// Declares the authentication shape without storing credential material.
    #[must_use]
    pub fn with_auth_scheme(mut self, scheme: AuthScheme) -> Self {
        self.auth_scheme = Some(scheme);
        self
    }

    pub(crate) fn with_provider_headers(mut self, headers: Vec<HeaderOperation>) -> Self {
        self.provider_headers = headers;
        self
    }

    pub(crate) fn with_model_headers(mut self, headers: Vec<HeaderOperation>) -> Self {
        self.model_headers = headers;
        self
    }

    /// Installs a controlled value-free dynamic header policy.
    #[must_use]
    pub fn with_dynamic_header_policy(mut self, policy: DynamicHeaderPolicy) -> Self {
        self.dynamic_header_policy = Some(Arc::new(policy));
        self
    }

    pub(crate) fn with_shared_dynamic_header_policy(
        mut self,
        policy: Option<Arc<DynamicHeaderPolicy>>,
    ) -> Self {
        self.dynamic_header_policy = policy;
        self
    }

    /// Declares conservative provider-level capabilities explicitly.
    #[must_use]
    pub fn with_capabilities(mut self, capabilities: ProviderCapabilities) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    /// Adds or replaces one exact-model capability declaration.
    #[must_use]
    pub fn with_model_capabilities(mut self, profile: ModelCapabilityProfile) -> Self {
        self.model_capabilities
            .insert(profile.model().clone(), profile);
        self
    }

    /// Installs an exact model catalog whose identities must match this definition.
    #[must_use]
    pub fn with_catalog(mut self, catalog: ModelCatalog) -> Self {
        self.catalog = Some(catalog);
        self.allow_unregistered_models = false;
        self
    }

    /// Explicitly permits models absent from a catalog.
    #[must_use]
    pub fn allow_unregistered_models(mut self) -> Self {
        self.catalog = None;
        self.allow_unregistered_models = true;
        self
    }

    /// Installs typed OpenAI-compatible provider deviations.
    #[must_use]
    pub fn with_provider_compat(mut self, compat: CompatPatch) -> Self {
        self.provider_compat = compat;
        self
    }

    /// Adds typed OpenAI-compatible deviations for one exact model.
    #[must_use]
    pub fn with_model_compat(mut self, model: ModelId, compat: CompatPatch) -> Self {
        self.model_compat.insert(model, compat);
        self
    }

    pub(crate) fn with_openrouter_routing(mut self, contract: OpenRouterRoutingContract) -> Self {
        self.openrouter_routing = Some(contract);
        self
    }

    pub(crate) fn with_transport_options(mut self, transport: ProviderTransportOptions) -> Self {
        self.transport = transport;
        self
    }

    /// Replaces the typed rate-limit observation policy.
    #[must_use]
    pub fn with_rate_limit_policy(mut self, policy: RateLimitPolicy) -> Self {
        self.rate_limit = policy;
        self
    }

    /// Replaces the typed idempotency policy.
    #[must_use]
    pub fn with_idempotency_policy(mut self, policy: IdempotencyPolicy) -> Self {
        self.idempotency = policy;
        self
    }

    /// Adds the protected Anthropic API version and removes beta opt-ins.
    pub fn with_anthropic_version(mut self, version: &str) -> Result<Self, LlmError> {
        if self.dialect != ProtocolDialect::AnthropicMessages
            || version.is_empty()
            || version.len() > 64
            || !version
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'-')
        {
            return Err(configuration("invalid Anthropic API version declaration"));
        }
        self.provider_headers.retain(|operation| {
            !matches!(
                operation.name().as_str(),
                "anthropic-version" | "anthropic-beta"
            )
        });
        self.provider_headers.push(HeaderOperation::set(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_str(version)
                .map_err(|_| configuration("invalid Anthropic API version declaration"))?,
        ));
        self.provider_headers
            .push(HeaderOperation::remove(HeaderName::from_static(
                "anthropic-beta",
            )));
        Ok(self)
    }

    /// Selects reviewed Anthropic Messages usage snapshot compatibility.
    ///
    /// Official Anthropic definitions should retain
    /// [`AnthropicUsageCompat::StrictStableFields`]. Compatible providers may
    /// opt into a narrower monotonic policy when their usage counters evolve
    /// across the same stream.
    pub fn with_anthropic_usage_compat(
        mut self,
        usage: AnthropicUsageCompat,
    ) -> Result<Self, LlmError> {
        let ResolvedProtocolContract::AnthropicMessages(contract) = self.protocol_contract else {
            return Err(configuration(
                "Anthropic usage compatibility requires an Anthropic Messages definition",
            ));
        };
        self.protocol_contract =
            ResolvedProtocolContract::AnthropicMessages(contract.with_usage_compat(usage));
        Ok(self)
    }

    /// Validates and freezes this secret-free definition.
    pub fn build(self) -> Result<ProviderDefinition, LlmError> {
        if !self.protocol_contract.matches_dialect(self.dialect) {
            return Err(configuration(
                "protocol dialect does not match resolved protocol contract",
            ));
        }
        let endpoint = self
            .endpoint
            .ok_or_else(|| configuration("provider definition requires an endpoint"))?;
        let auth_scheme = self
            .auth_scheme
            .ok_or_else(|| configuration("provider definition requires an auth scheme"))?;
        let capabilities = self
            .capabilities
            .ok_or_else(|| configuration("provider definition requires capabilities"))?;
        capabilities.validate()?;
        let catalog = match (self.catalog, self.allow_unregistered_models) {
            (Some(catalog), false) => catalog,
            (None, true) => ModelCatalog::default(),
            _ => {
                return Err(configuration(
                    "provider definition requires a catalog or explicit unregistered-model policy",
                ));
            }
        };
        validate_catalog(
            &catalog,
            &self.provider_id,
            &self.product_id,
            &self.protocol_id,
        )?;
        validate_model_capabilities(&self.model_capabilities)?;

        if matches!(self.dialect, ProtocolDialect::AnthropicMessages)
            && (!self.provider_compat.is_empty()
                || self.model_compat.values().any(|patch| !patch.is_empty())
                || self.openrouter_routing.is_some())
        {
            return Err(configuration(
                "Anthropic Messages definitions cannot carry OpenAI compatibility policy",
            ));
        }

        validate_header_owners(&auth_scheme, &self.provider_headers, &self.model_headers)?;
        if matches!(self.dialect, ProtocolDialect::AnthropicMessages)
            && !self
                .provider_headers
                .iter()
                .any(|operation| operation.name().as_str() == "anthropic-version")
        {
            return Err(configuration(
                "Anthropic Messages definition requires a protected anthropic-version header",
            ));
        }

        let resolved = resolve_definition_endpoint(&endpoint, &catalog)?;
        let credential_binding = if self.bind_to_endpoint_origin {
            CredentialBinding::exact_https_origin(&resolved)?
        } else {
            self.credential_binding
                .ok_or_else(|| configuration("provider definition requires a credential binding"))?
        };
        credential_binding.validate(&resolved)?;

        Ok(ProviderDefinition {
            provider_id: self.provider_id,
            product_id: self.product_id,
            protocol_id: self.protocol_id,
            dialect: self.dialect,
            protocol_contract: self.protocol_contract,
            endpoint,
            credential_binding,
            auth_scheme,
            provider_headers: self.provider_headers,
            model_headers: self.model_headers,
            dynamic_header_policy: self.dynamic_header_policy,
            capabilities,
            model_capabilities: self.model_capabilities,
            catalog,
            provider_compat: self.provider_compat,
            model_compat: self.model_compat,
            openrouter_routing: self.openrouter_routing,
            transport: self.transport,
            rate_limit: self.rate_limit,
            idempotency: self.idempotency,
        })
    }
}

fn resolve_definition_endpoint(
    endpoint: &EndpointConfig,
    catalog: &ModelCatalog,
) -> Result<super::ResolvedEndpoint, LlmError> {
    if endpoint.requires_mapping() {
        let entry = catalog.entries().next().ok_or_else(|| {
            configuration("endpoint template variables require an exact catalog entry")
        })?;
        let mapping = ResolvedModelMapping::from_entry(entry);
        resolve_official_for(endpoint, mapping.endpoint_values())
    } else {
        resolve_official(endpoint)
    }
}

fn validate_catalog(
    catalog: &ModelCatalog,
    provider_id: &ProviderId,
    product_id: &ProductId,
    protocol_id: &ProtocolId,
) -> Result<(), LlmError> {
    if catalog.entries().any(|entry| {
        &entry.key.provider_id != provider_id
            || &entry.key.product_id != product_id
            || &entry.protocol_id != protocol_id
    }) {
        Err(configuration(
            "catalog entry does not match provider product protocol",
        ))
    } else {
        Ok(())
    }
}

fn validate_model_capabilities(
    model_capabilities: &BTreeMap<ModelId, ModelCapabilityProfile>,
) -> Result<(), LlmError> {
    if model_capabilities
        .iter()
        .any(|(model, declaration)| model != declaration.model())
    {
        Err(configuration(
            "model capability map key does not match its declaration",
        ))
    } else {
        Ok(())
    }
}

fn validate_header_owners(
    auth_scheme: &AuthScheme,
    provider_headers: &[HeaderOperation],
    model_headers: &[HeaderOperation],
) -> Result<(), LlmError> {
    let provider_names = provider_headers
        .iter()
        .map(HeaderOperation::name)
        .cloned()
        .collect::<Vec<_>>();
    let pipeline =
        HeaderPipeline::with_registered_headers(auth_scheme.protected_headers(), provider_names);
    pipeline.resolve_without_auth_assumption(vec![
        HeaderLayer::new(
            HeaderSource::Protocol,
            vec![
                HeaderOperation::set(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                ),
                HeaderOperation::set(
                    header::ACCEPT,
                    HeaderValue::from_static("text/event-stream"),
                ),
            ],
        ),
        HeaderLayer::new(HeaderSource::Provider, provider_headers.to_vec()),
        HeaderLayer::new(HeaderSource::Model, model_headers.to_vec()),
    ])?;
    Ok(())
}

fn validate_deployment_limits(deployment: &ResolvedProviderDeployment) -> Result<(), LlmError> {
    deployment.resource_limits.validate()?;
    let limits = deployment.resource_limits;
    let maximum = ResourceLimits::default();
    let bounded = limits.max_request_body_bytes <= maximum.max_request_body_bytes
        && limits.max_messages <= maximum.max_messages
        && limits.max_total_text_bytes <= maximum.max_total_text_bytes
        && limits.max_tools <= maximum.max_tools
        && limits.max_tool_description_bytes <= maximum.max_tool_description_bytes
        && limits.max_schema_bytes <= maximum.max_schema_bytes
        && limits.max_schema_depth <= maximum.max_schema_depth
        && limits.max_tool_calls <= maximum.max_tool_calls
        && limits.max_tool_arguments_bytes <= maximum.max_tool_arguments_bytes
        && limits.max_all_tool_arguments_bytes <= maximum.max_all_tool_arguments_bytes
        && limits.max_json_array_items <= maximum.max_json_array_items
        && limits.max_images <= maximum.max_images
        && limits.max_inline_image_bytes <= maximum.max_inline_image_bytes
        && limits.max_image_url_bytes <= maximum.max_image_url_bytes
        && limits.max_structured_output_bytes <= maximum.max_structured_output_bytes;
    if !bounded {
        return Err(configuration(
            "deployment resource limits exceed the SDK production ceiling",
        ));
    }

    let sse = deployment.sse;
    let sse_maximum = SseConfig::default();
    let sse_bounded = sse.max_event_bytes() <= sse_maximum.max_event_bytes()
        && sse.max_line_bytes() <= sse_maximum.max_line_bytes()
        && sse.max_chunk_bytes() <= sse_maximum.max_chunk_bytes()
        && sse.max_bytes_per_poll() <= sse_maximum.max_bytes_per_poll()
        && sse.max_chunks_per_poll() <= sse_maximum.max_chunks_per_poll()
        && sse.max_events_per_poll() <= sse_maximum.max_events_per_poll()
        && matches!(
            (sse.max_fields_per_event(), sse_maximum.max_fields_per_event()),
            (Some(actual), Some(maximum)) if actual > 0 && actual <= maximum
        );
    if !sse_bounded {
        return Err(configuration(
            "deployment SSE limits must be positive and within the SDK production ceiling",
        ));
    }
    Ok(())
}

fn validate_auth_header_name(name: &HeaderName) -> Result<(), LlmError> {
    if name.as_str().len() > 128
        || matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "content-type"
                | "accept"
                | "transfer-encoding"
                | "connection"
                | "cookie"
                | "set-cookie"
                | "user-agent"
        )
    {
        Err(ValidationError::new(
            "auth.header_name",
            ValidationReason::ProtectedHeader,
            "header belongs to a non-authentication owner",
        )
        .into())
    } else {
        Ok(())
    }
}

fn header_set(headers: Vec<HeaderName>) -> HashSet<HeaderName> {
    headers.into_iter().collect()
}

fn configuration(message: &'static str) -> LlmError {
    LlmError::Configuration(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::auth::{ApiKey, ApiKeyHeaderAuth, BearerAuth, BearerCredential};

    fn openai_builder() -> ProviderDefinitionBuilder {
        ProviderDefinition::openai_chat(
            ProviderId::new("custom-openai").unwrap(),
            ProductId::new("chat").unwrap(),
        )
    }

    fn endpoint() -> EndpointConfig {
        EndpointConfig::absolute("https://llm.example.com/v1/chat/completions").unwrap()
    }

    #[test]
    fn required_definition_fields_fail_closed() {
        assert!(openai_builder().build().is_err());
        assert!(
            openai_builder()
                .with_endpoint(endpoint())
                .with_auth_scheme(AuthScheme::bearer())
                .allow_unregistered_models()
                .build()
                .is_err()
        );
    }

    #[test]
    fn both_protocols_compile_through_the_generic_builder() {
        let openai = openai_builder()
            .with_endpoint(endpoint())
            .bind_credential_to_endpoint_origin()
            .with_auth_scheme(AuthScheme::bearer())
            .with_capabilities(ProviderCapabilities::official_openai())
            .allow_unregistered_models()
            .build()
            .unwrap();
        let openai_auth = Arc::new(BearerAuth::new(BearerCredential::new(
            ApiKey::new("builder-openai-key").unwrap(),
            openai.credential_binding().clone(),
        )));
        let profile = openai
            .compile_resolved(ResolvedProviderDeployment::new(
                openai_auth,
                ClientIdentity::default(),
            ))
            .unwrap();
        assert_eq!(profile.dialect(), ProtocolDialect::OpenAiChatCompletions);

        let anthropic_endpoint =
            EndpointConfig::absolute("https://messages.example.com/v1/messages").unwrap();
        let anthropic = ProviderDefinition::anthropic_messages(
            ProviderId::new("custom-anthropic").unwrap(),
            ProductId::new("messages").unwrap(),
        )
        .with_endpoint(anthropic_endpoint)
        .bind_credential_to_endpoint_origin()
        .with_auth_scheme(AuthScheme::api_key_header(HeaderName::from_static("x-api-key")).unwrap())
        .with_provider_headers(vec![HeaderOperation::set(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static("2023-06-01"),
        )])
        .with_capabilities(ProviderCapabilities::official_anthropic())
        .allow_unregistered_models()
        .build()
        .unwrap();
        let anthropic_auth = Arc::new(
            ApiKeyHeaderAuth::new(
                HeaderName::from_static("x-api-key"),
                ApiKey::new("builder-anthropic-key").unwrap(),
                anthropic.credential_binding().clone(),
            )
            .unwrap(),
        );
        let profile = anthropic
            .compile_resolved(ResolvedProviderDeployment::new(
                anthropic_auth,
                ClientIdentity::default(),
            ))
            .unwrap();
        assert_eq!(profile.dialect(), ProtocolDialect::AnthropicMessages);
    }

    #[test]
    fn header_owner_conflicts_and_binding_drift_are_rejected() {
        let conflict = openai_builder()
            .with_endpoint(endpoint())
            .bind_credential_to_endpoint_origin()
            .with_auth_scheme(AuthScheme::bearer())
            .with_provider_headers(vec![HeaderOperation::set(
                header::AUTHORIZATION,
                HeaderValue::from_static("forbidden"),
            )])
            .with_capabilities(ProviderCapabilities::official_openai())
            .allow_unregistered_models()
            .build();
        assert!(conflict.is_err());

        let definition = openai_builder()
            .with_endpoint(endpoint())
            .bind_credential_to_endpoint_origin()
            .with_auth_scheme(AuthScheme::bearer())
            .with_capabilities(ProviderCapabilities::official_openai())
            .allow_unregistered_models()
            .build()
            .unwrap();
        let foreign = resolve_official(
            &EndpointConfig::absolute("https://other.example.com/v1/chat").unwrap(),
        )
        .unwrap();
        let auth = Arc::new(BearerAuth::new(BearerCredential::new(
            ApiKey::new("foreign-binding-key").unwrap(),
            CredentialBinding::exact_https_origin(&foreign).unwrap(),
        )));
        assert!(
            definition
                .compile_resolved(ResolvedProviderDeployment::new(
                    auth,
                    ClientIdentity::default(),
                ))
                .is_err()
        );
    }

    #[test]
    fn contract_mismatch_limits_and_debug_fail_safely() {
        let mut mismatch = openai_builder();
        mismatch.protocol_contract = ResolvedProtocolContract::strict_anthropic_messages();
        assert!(
            mismatch
                .with_endpoint(endpoint())
                .bind_credential_to_endpoint_origin()
                .with_auth_scheme(AuthScheme::bearer())
                .with_capabilities(ProviderCapabilities::official_openai())
                .allow_unregistered_models()
                .build()
                .is_err()
        );

        let definition = openai_builder()
            .with_endpoint(endpoint())
            .bind_credential_to_endpoint_origin()
            .with_auth_scheme(AuthScheme::bearer())
            .with_provider_headers(vec![HeaderOperation::set(
                HeaderName::from_static("x-provider-canary"),
                HeaderValue::from_static("provider-header-value-canary"),
            )])
            .with_capabilities(ProviderCapabilities::official_openai())
            .allow_unregistered_models()
            .build()
            .unwrap();
        let auth = Arc::new(BearerAuth::new(BearerCredential::new(
            ApiKey::new("definition-debug-secret-canary").unwrap(),
            definition.credential_binding().clone(),
        )));
        let mut limits = ResourceLimits::official();
        limits.max_messages = 0;
        let deployment = ResolvedProviderDeployment::new(auth, ClientIdentity::default())
            .with_resource_limits(limits);
        let debug = format!("{definition:?} {deployment:?}");
        assert!(debug.contains("x-provider-canary"));
        assert!(!debug.contains("provider-header-value-canary"));
        assert!(!debug.contains("definition-debug-secret-canary"));
        assert!(definition.compile_resolved(deployment).is_err());
    }
}
