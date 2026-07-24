//! Validated immutable provider runtime.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use http::{HeaderMap, HeaderValue, Method, header};

use crate::domain::{
    GenerateRequest, HistoryPolicy, LocalRequestId, ModelId, PolicySource, ProtocolId, ProviderId,
};
use crate::error::LlmError;
use crate::protocol::RequestFacts;
use crate::transport::{RequestLifecycle, SseConfig};

use super::auth::{AuthContext, AuthProvider, ClientIdentity};
use super::call_policy::{
    CallPolicySnapshot, ProtocolKind, ResolvedCompat, ResolvedLimits, ResolvedTarget,
};
use super::capability::{
    ModelCapabilityProfile, ProtocolDialect, ProviderCapabilities, ProviderTransportOptions,
};
use super::catalog::{ModelCatalog, ModelEntry, ModelKey, ProductId, ProviderModelId};
use super::compat::{
    CompatPatch, OpenRouterRoutingContract, ProviderRequestOptions, resolve_compat, validate_compat,
};
use super::endpoint::{
    EndpointConfig, EndpointMode, EndpointValues, ResolvedEndpoint, ResolvedModelMapping,
    resolve_official, resolve_official_for, resolve_test_only, resolve_test_only_for,
};
use super::headers::{
    DynamicHeaderContext, DynamicHeaderPolicy, DynamicResponseFormat, HeaderLayer, HeaderOperation,
    HeaderPipeline, HeaderSource, ResolvedHeaders,
};
use super::profile::ProviderProfile;

/// Immutable, concurrency-safe provider runtime.
#[derive(Clone)]
pub struct ProviderRuntime {
    provider_id: ProviderId,
    product_id: ProductId,
    protocol_id: ProtocolId,
    protocol_kind: ProtocolKind,
    endpoint: ResolvedEndpoint,
    endpoint_config: EndpointConfig,
    endpoint_mode: EndpointMode,
    auth: Arc<dyn AuthProvider>,
    client_identity: ClientIdentity,
    provider_headers: Arc<[HeaderOperation]>,
    model_headers: Arc<[HeaderOperation]>,
    dynamic_header_policy: Option<Arc<DynamicHeaderPolicy>>,
    capabilities: ProviderCapabilities,
    model_capabilities: BTreeMap<ModelId, ModelCapabilityProfile>,
    catalog: ModelCatalog,
    provider_compat: CompatPatch,
    model_compat: BTreeMap<ModelId, CompatPatch>,
    openrouter_routing: Option<OpenRouterRoutingContract>,
    dialect: ProtocolDialect,
    transport: ProviderTransportOptions,
    resource_limits: crate::domain::ResourceLimits,
    sse: SseConfig,
    max_http_error_body_bytes: usize,
    pipeline: HeaderPipeline,
}

pub(crate) struct HeaderAttemptContext<'a> {
    pub(crate) endpoint: &'a ResolvedEndpoint,
    pub(crate) facts: &'a RequestFacts,
    pub(crate) lifecycle: &'a RequestLifecycle,
    pub(crate) model_id: &'a ModelId,
    pub(crate) local_request_id: &'a LocalRequestId,
    pub(crate) attempt_number: u32,
}

impl ProviderRuntime {
    /// Validates and freezes a profile.
    pub fn build(profile: ProviderProfile) -> Result<Self, LlmError> {
        profile.capabilities.validate()?;
        for entry in profile.catalog.entries() {
            if entry.key.provider_id != profile.provider_id
                || entry.key.product_id != profile.product_id
                || entry.protocol_id != profile.protocol_id
            {
                return Err(LlmError::Configuration(
                    "catalog entry does not match provider product protocol".to_owned(),
                ));
            }
        }
        for (model, declaration) in &profile.model_capabilities {
            if model != declaration.model() {
                return Err(LlmError::Configuration(
                    "model capability map key does not match its declaration".to_owned(),
                ));
            }
        }
        let protocol_kind = match profile.dialect {
            ProtocolDialect::OpenAiChatCompletions
                if profile.protocol_id.as_str() == "openai-chat-completions" =>
            {
                ProtocolKind::OpenAiChatCompletions
            }
            ProtocolDialect::OpenAiChatCompletions => {
                return Err(LlmError::Configuration(
                    "OpenAI Chat dialect requires the openai-chat-completions protocol id"
                        .to_owned(),
                ));
            }
        };
        let endpoint_mode = if profile.test_only {
            EndpointMode::TestOnly
        } else {
            EndpointMode::Official
        };
        let endpoint = if profile.endpoint.requires_mapping() {
            let first = profile.catalog.entries().next().ok_or_else(|| {
                LlmError::Configuration(
                    "endpoint template variables require an exact catalog entry".to_owned(),
                )
            })?;
            resolve_entry_endpoint(&profile.endpoint, endpoint_mode, first)?
        } else if profile.test_only {
            resolve_test_only(&profile.endpoint)?
        } else {
            resolve_official(&profile.endpoint)?
        };
        profile.audience.validate(&endpoint)?;
        profile.auth.validate_endpoint(&endpoint)?;
        for entry in profile.catalog.entries() {
            let mapped = resolve_entry_endpoint(&profile.endpoint, endpoint_mode, entry)?;
            profile.audience.validate(&mapped)?;
            profile.auth.validate_endpoint(&mapped)?;
        }
        let auth_headers = profile.auth.protected_headers();
        let provider_header_names = profile
            .provider_headers
            .iter()
            .map(HeaderOperation::name)
            .cloned()
            .collect::<Vec<_>>();
        let auth = profile.auth;
        Ok(Self {
            provider_id: profile.provider_id,
            product_id: profile.product_id,
            protocol_id: profile.protocol_id,
            protocol_kind,
            endpoint,
            endpoint_config: profile.endpoint,
            endpoint_mode,
            auth,
            client_identity: profile.client_identity,
            provider_headers: profile.provider_headers.into(),
            model_headers: profile.model_headers.into(),
            dynamic_header_policy: profile.dynamic_header_policy,
            capabilities: profile.capabilities,
            model_capabilities: profile.model_capabilities,
            catalog: profile.catalog,
            provider_compat: profile.provider_compat,
            model_compat: profile.model_compat,
            openrouter_routing: profile.openrouter_routing,
            dialect: profile.dialect,
            transport: profile.transport,
            resource_limits: profile.resource_limits,
            sse: profile.sse,
            max_http_error_body_bytes: profile.max_http_error_body_bytes,
            pipeline: HeaderPipeline::with_registered_headers(auth_headers, provider_header_names),
        })
    }

    /// Returns provider identifier.
    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    /// Returns protocol identifier.
    pub fn protocol_id(&self) -> &ProtocolId {
        &self.protocol_id
    }

    /// Returns the exact provider product identifier.
    pub fn product_id(&self) -> &ProductId {
        &self.product_id
    }

    /// Returns the immutable catalog snapshot.
    pub fn catalog(&self) -> &ModelCatalog {
        &self.catalog
    }

    /// Returns an exact catalog entry for a domain model.
    pub fn model_entry(&self, model: &ModelId) -> Option<&ModelEntry> {
        self.catalog.get(&ModelKey {
            provider_id: self.provider_id.clone(),
            product_id: self.product_id.clone(),
            domain_model_id: model.clone(),
        })
    }

    /// Returns resolved endpoint.
    pub fn endpoint(&self) -> &ResolvedEndpoint {
        &self.endpoint
    }

    /// Returns the phase-one HTTP method.
    pub fn method(&self) -> Method {
        Method::POST
    }

    /// Returns immutable capabilities.
    pub fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    /// Resolves provider defaults plus an exact model capability declaration.
    pub fn capabilities_for(&self, model: &ModelId) -> ProviderCapabilities {
        let mut capabilities = self.capabilities.clone();
        if let Some(entry) = self.model_entry(model) {
            capabilities.apply_catalog(&entry.capabilities);
        }
        if let Some(profile) = self.model_capabilities.get(model) {
            capabilities.apply_model(profile);
        }
        capabilities
    }

    /// Returns dialect.
    pub fn dialect(&self) -> ProtocolDialect {
        self.dialect
    }

    /// Returns transport options.
    pub fn transport_options(&self) -> ProviderTransportOptions {
        self.transport
    }

    /// Compiles the immutable, target-aware policy used for one logical call.
    pub(crate) fn plan_policy_for_with_options(
        &self,
        request: &GenerateRequest,
        provider_options: &ProviderRequestOptions,
    ) -> Result<CallPolicySnapshot, LlmError> {
        if request.model().provider() != &self.provider_id {
            return Err(crate::error::ValidationError::new(
                "model.provider",
                crate::error::ValidationReason::ProviderMismatch,
                "request provider does not match configured client runtime",
            )
            .into());
        }

        let entry = self.model_entry(request.model().model());
        let capabilities = self.capabilities_for(request.model().model());
        capabilities.validate()?;
        let mut compat_layers = vec![self.provider_compat.clone()];
        if let Some(entry) = entry {
            compat_layers.push(entry.compat_overrides.clone());
        }
        if let Some(model) = self.model_compat.get(request.model().model()) {
            compat_layers.push(model.clone());
        }
        let compat_profile = match self.dialect {
            ProtocolDialect::OpenAiChatCompletions => resolve_compat(&compat_layers),
        };
        validate_compat(&compat_profile, &capabilities)?;
        let dialect = compat_profile.dialect_policy();
        let model_limits = entry.map(|entry| entry.limits).unwrap_or_default();
        let limits = ResolvedLimits::compile(
            model_limits.apply_to(self.resource_limits),
            self.sse,
            self.max_http_error_body_bytes,
            model_limits.max_output_tokens,
            entry.and_then(|entry| entry.default_max_output_tokens),
        )?;
        let mut history = HistoryPolicy::official_openai();
        history.max_messages = limits.request.max_messages;
        history.max_total_text_bytes = limits.request.max_text_bytes;
        let request_routing = provider_options.openrouter_routing();
        let openrouter_routing = match (&self.openrouter_routing, request_routing) {
            (Some(contract), request) => {
                let resolved = contract.resolve(request)?;
                (!resolved.is_empty()).then_some(resolved)
            }
            (None, Some(_)) => {
                return Err(crate::error::ValidationError::new(
                    "provider_options.openrouter_routing",
                    crate::error::ValidationReason::CapabilityUnsupported,
                    "selected profile does not declare OpenRouter routing support",
                )
                .into());
            }
            (None, None) => None,
        };

        Ok(CallPolicySnapshot {
            target: ResolvedTarget {
                provider_id: self.provider_id.clone(),
                product_id: self.product_id.clone(),
                protocol_id: self.protocol_id.clone(),
                protocol_kind: self.protocol_kind,
                domain_model: request.model().model().clone(),
                provider_model: entry.map_or_else(
                    || ProviderModelId::new(request.model().model().as_str()),
                    |entry| Ok(entry.provider_model_id.clone()),
                )?,
                deployment_id: entry.and_then(|entry| entry.deployment_id.clone()),
                wire_model: entry
                    .map(|entry| ModelId::new(entry.wire_model_value.as_str()))
                    .transpose()?
                    .unwrap_or_else(|| request.model().model().clone()),
            },
            capabilities,
            compat: ResolvedCompat {
                dialect,
                profile: compat_profile,
            },
            history,
            limits,
            response_format: request.options().response_format().clone(),
            provider_routing: openrouter_routing,
        })
    }

    pub(crate) fn policy_provenance_for(&self, model: &ModelId) -> (PolicySource, bool) {
        let model_override_applied = self.model_capabilities.contains_key(model)
            || self.model_entry(model).is_some()
            || self.model_compat.contains_key(model);
        let source = if model_override_applied {
            PolicySource::ModelProfile
        } else {
            PolicySource::ProviderProfile
        };
        (source, model_override_applied)
    }

    /// Resolves a fresh header map and trace for one request.
    pub fn resolve_headers(
        &self,
        model: Vec<HeaderOperation>,
        request: &HeaderMap,
    ) -> Result<ResolvedHeaders, LlmError> {
        let mut protocol = HeaderMap::new();
        protocol.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        protocol.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );
        self.resolve_headers_with_protocol(&protocol, model, request)
    }

    /// Resolves headers using protocol intents produced by a validated adapter.
    pub fn resolve_headers_with_protocol(
        &self,
        protocol: &HeaderMap,
        model: Vec<HeaderOperation>,
        request: &HeaderMap,
    ) -> Result<ResolvedHeaders, LlmError> {
        let protocol = protocol
            .iter()
            .map(|(name, value)| HeaderOperation::set(name.clone(), value.clone()))
            .collect();
        self.resolve_headers_with_protocol_operations(protocol, model, request, None)
    }

    /// Resolves headers from typed protocol operations produced by a driver.
    pub(crate) fn resolve_headers_with_protocol_operations(
        &self,
        protocol: Vec<HeaderOperation>,
        model: Vec<HeaderOperation>,
        request: &HeaderMap,
        facts: Option<&RequestFacts>,
    ) -> Result<ResolvedHeaders, LlmError> {
        let mut model_operations = self.model_headers.to_vec();
        model_operations.extend(model);
        let request_operations = request
            .iter()
            .map(|(name, value)| HeaderOperation::set(name.clone(), value.clone()))
            .collect();
        let auth = self
            .auth
            .resolve_immediate(AuthContext::new(&self.endpoint))?;
        let _ = facts;
        let resolved = self.pipeline.resolve_without_auth_assumption(vec![
            HeaderLayer::new(HeaderSource::Transport, Vec::new()),
            HeaderLayer::new(HeaderSource::Protocol, protocol),
            HeaderLayer::new(HeaderSource::Provider, self.provider_headers.to_vec()),
            HeaderLayer::new(
                HeaderSource::ClientIdentity,
                vec![self.client_identity.operation()?],
            ),
            HeaderLayer::new(HeaderSource::Model, model_operations),
            HeaderLayer::new(HeaderSource::DynamicPolicy, Vec::new()),
            HeaderLayer::new(HeaderSource::Request, request_operations),
            HeaderLayer::new(HeaderSource::Auth, auth),
        ])?;
        if resolved.headers().get(header::CONTENT_TYPE)
            != Some(&HeaderValue::from_static("application/json"))
        {
            return Err(crate::error::ValidationError::new(
                "request_headers.content-type",
                crate::error::ValidationReason::ProtectedHeader,
                "OpenAI Chat protocol must set application/json",
            )
            .into());
        }
        self.auth.validate_final(resolved.headers())?;
        Ok(resolved)
    }

    pub(crate) async fn resolve_headers_for_attempt(
        &self,
        protocol: Vec<HeaderOperation>,
        model: Vec<HeaderOperation>,
        request: &HeaderMap,
        attempt: HeaderAttemptContext<'_>,
    ) -> Result<ResolvedHeaders, LlmError> {
        let mut model_operations = self.model_headers.to_vec();
        model_operations.extend(model);
        let request_operations = request
            .iter()
            .map(|(name, value)| HeaderOperation::set(name.clone(), value.clone()))
            .collect();
        let auth = self
            .auth
            .resolve(AuthContext::for_attempt(
                attempt.endpoint,
                &self.provider_id,
                &self.product_id,
                attempt.lifecycle,
            ))
            .await?;
        let dynamic = if let Some(policy) = &self.dynamic_header_policy {
            policy
                .resolve(
                    DynamicHeaderContext::for_attempt(
                        self.provider_id.clone(),
                        self.product_id.clone(),
                        attempt.model_id.clone(),
                        self.protocol_id.clone(),
                        attempt.local_request_id.clone(),
                        attempt.attempt_number,
                        attempt.facts.contains_tools,
                        attempt.facts.contains_images,
                        attempt.facts.reasoning_enabled,
                        DynamicResponseFormat::from(attempt.facts.response_format),
                        attempt.lifecycle,
                    ),
                    attempt.lifecycle,
                )
                .await?
        } else {
            Vec::new()
        };
        let resolved = self.pipeline.resolve_without_auth_assumption(vec![
            HeaderLayer::new(HeaderSource::Transport, Vec::new()),
            HeaderLayer::new(HeaderSource::Protocol, protocol),
            HeaderLayer::new(HeaderSource::Provider, self.provider_headers.to_vec()),
            HeaderLayer::new(
                HeaderSource::ClientIdentity,
                vec![self.client_identity.operation()?],
            ),
            HeaderLayer::new(HeaderSource::Model, model_operations),
            HeaderLayer::new(HeaderSource::DynamicPolicy, dynamic),
            HeaderLayer::new(HeaderSource::Request, request_operations),
            HeaderLayer::new(HeaderSource::Auth, auth),
        ])?;
        if resolved.headers().get(header::CONTENT_TYPE)
            != Some(&HeaderValue::from_static("application/json"))
        {
            return Err(crate::error::ValidationError::new(
                "request_headers.content-type",
                crate::error::ValidationReason::ProtectedHeader,
                "OpenAI Chat protocol must set application/json",
            )
            .into());
        }
        self.auth.validate_final(resolved.headers())?;
        Ok(resolved)
    }

    pub(crate) fn resolve_target_endpoint(
        &self,
        target: &ResolvedTarget,
    ) -> Result<ResolvedEndpoint, LlmError> {
        if target.provider_id != self.provider_id || target.protocol_id != self.protocol_id {
            return Err(LlmError::Configuration(
                "prepared call target does not match provider runtime".to_owned(),
            ));
        }
        if target.product_id != self.product_id {
            return Err(LlmError::Configuration(
                "prepared call product does not match provider runtime".to_owned(),
            ));
        }
        let values = EndpointValues::new(
            &target.product_id,
            &target.provider_model,
            target.deployment_id.as_ref(),
        );
        match self.endpoint_mode {
            EndpointMode::Official => resolve_official_for(&self.endpoint_config, values),
            EndpointMode::TestOnly => resolve_test_only_for(&self.endpoint_config, values),
        }
    }
}

fn resolve_entry_endpoint(
    config: &EndpointConfig,
    mode: EndpointMode,
    entry: &ModelEntry,
) -> Result<ResolvedEndpoint, LlmError> {
    let mapping = ResolvedModelMapping::from_entry(entry);
    let values = mapping.endpoint_values();
    match mode {
        EndpointMode::Official => resolve_official_for(config, values),
        EndpointMode::TestOnly => resolve_test_only_for(config, values),
    }
}

impl fmt::Debug for ProviderRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderRuntime")
            .field("provider_id", &self.provider_id)
            .field("product_id", &self.product_id)
            .field("protocol_id", &self.protocol_id)
            .field("endpoint", &self.endpoint)
            .field("auth", &"[REDACTED]")
            .field("client_identity", &self.client_identity)
            .field("capabilities", &self.capabilities)
            .field("model_capability_count", &self.model_capabilities.len())
            .field("catalog_entry_count", &self.catalog.entries().count())
            .field("dialect", &self.dialect)
            .field("transport", &self.transport)
            .field("resource_limits", &self.resource_limits)
            .field("sse", &self.sse)
            .field("max_http_error_body_bytes", &self.max_http_error_body_bytes)
            .finish_non_exhaustive()
    }
}
