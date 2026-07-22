//! Validated immutable provider runtime.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use http::{HeaderMap, HeaderValue, Method, header};

use crate::domain::{
    DialectPolicy, GenerateRequest, HistoryPolicy, ModelId, PolicySource, ProtocolId, ProviderId,
};
use crate::error::LlmError;
use crate::protocol::RequestFacts;
use crate::transport::SseConfig;

use super::auth::{AuthContext, AuthProvider, BearerAuth, ClientIdentity};
use super::call_policy::{
    CallPolicySnapshot, ProtocolKind, ResolvedCompat, ResolvedLimits, ResolvedTarget,
};
use super::capability::{
    ModelCapabilityProfile, ProtocolDialect, ProviderCapabilities, ProviderTransportOptions,
};
use super::endpoint::{ResolvedEndpoint, resolve_official, resolve_test_only};
use super::headers::{HeaderLayer, HeaderOperation, HeaderPipeline, HeaderSource, ResolvedHeaders};
use super::profile::ProviderProfile;

/// Immutable, concurrency-safe provider runtime.
#[derive(Clone)]
pub struct ProviderRuntime {
    provider_id: ProviderId,
    protocol_id: ProtocolId,
    protocol_kind: ProtocolKind,
    endpoint: ResolvedEndpoint,
    auth: Arc<dyn AuthProvider>,
    client_identity: ClientIdentity,
    provider_headers: Arc<[HeaderOperation]>,
    model_headers: Arc<[HeaderOperation]>,
    capabilities: ProviderCapabilities,
    model_capabilities: BTreeMap<ModelId, ModelCapabilityProfile>,
    dialect: ProtocolDialect,
    transport: ProviderTransportOptions,
    resource_limits: crate::domain::ResourceLimits,
    sse: SseConfig,
    max_http_error_body_bytes: usize,
    pipeline: HeaderPipeline,
}

impl ProviderRuntime {
    /// Validates and freezes a profile.
    pub fn build(profile: ProviderProfile) -> Result<Self, LlmError> {
        profile.capabilities.validate()?;
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
        let endpoint = if profile.test_only {
            resolve_test_only(&profile.endpoint)?
        } else {
            resolve_official(&profile.endpoint)?
        };
        profile.audience.validate(&endpoint)?;
        let auth = Arc::new(BearerAuth::new(profile.credential));
        Ok(Self {
            provider_id: profile.provider_id,
            protocol_id: profile.protocol_id,
            protocol_kind,
            endpoint,
            auth,
            client_identity: profile.client_identity,
            provider_headers: profile.provider_headers.into(),
            model_headers: profile.model_headers.into(),
            capabilities: profile.capabilities,
            model_capabilities: profile.model_capabilities,
            dialect: profile.dialect,
            transport: profile.transport,
            resource_limits: profile.resource_limits,
            sse: profile.sse,
            max_http_error_body_bytes: profile.max_http_error_body_bytes,
            pipeline: HeaderPipeline::new(),
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
    pub(crate) fn plan_policy_for(
        &self,
        request: &GenerateRequest,
    ) -> Result<CallPolicySnapshot, LlmError> {
        if request.model().provider() != &self.provider_id {
            return Err(crate::error::ValidationError::new(
                "model.provider",
                crate::error::ValidationReason::ProviderMismatch,
                "request provider does not match configured client runtime",
            )
            .into());
        }

        let capabilities = self.capabilities_for(request.model().model());
        capabilities.validate()?;
        let dialect = match self.dialect {
            ProtocolDialect::OpenAiChatCompletions => DialectPolicy::official_openai(),
        };
        let limits = ResolvedLimits::compile(
            self.resource_limits,
            self.sse,
            self.max_http_error_body_bytes,
        )?;
        let mut history = HistoryPolicy::official_openai();
        history.max_messages = limits.request.max_messages;
        history.max_total_text_bytes = limits.request.max_text_bytes;

        Ok(CallPolicySnapshot {
            target: ResolvedTarget {
                provider_id: self.provider_id.clone(),
                protocol_id: self.protocol_id.clone(),
                protocol_kind: self.protocol_kind,
                domain_model: request.model().model().clone(),
                wire_model: request.model().model().clone(),
            },
            capabilities,
            compat: ResolvedCompat { dialect },
            history,
            limits,
            response_format: request.options().response_format().clone(),
        })
    }

    pub(crate) fn policy_provenance_for(&self, model: &ModelId) -> (PolicySource, bool) {
        let model_override_applied = self.model_capabilities.contains_key(model);
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
        let auth = self.auth.operation(AuthContext::new(&self.endpoint))?;
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
            HeaderLayer::new(HeaderSource::Auth, vec![auth]),
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
        Ok(self.endpoint.clone())
    }
}

impl fmt::Debug for ProviderRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderRuntime")
            .field("provider_id", &self.provider_id)
            .field("protocol_id", &self.protocol_id)
            .field("endpoint", &self.endpoint)
            .field("auth", &"[REDACTED]")
            .field("client_identity", &self.client_identity)
            .field("capabilities", &self.capabilities)
            .field("model_capability_count", &self.model_capabilities.len())
            .field("dialect", &self.dialect)
            .field("transport", &self.transport)
            .field("resource_limits", &self.resource_limits)
            .field("sse", &self.sse)
            .field("max_http_error_body_bytes", &self.max_http_error_body_bytes)
            .finish_non_exhaustive()
    }
}
