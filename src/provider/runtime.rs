//! Validated immutable provider runtime.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;
use std::sync::Arc;

use http::{HeaderMap, HeaderValue, Method, header};

use crate::domain::{ProtocolId, ProviderId};
use crate::error::LlmError;

use super::auth::{AuthContext, AuthProvider, BearerAuth, ClientIdentity};
use super::capability::{ProtocolDialect, ProviderCapabilities, ProviderTransportOptions};
use super::endpoint::{ResolvedEndpoint, resolve_official, resolve_test_only};
use super::headers::{HeaderLayer, HeaderOperation, HeaderPipeline, HeaderSource, ResolvedHeaders};
use super::profile::ProviderProfile;

/// Immutable, concurrency-safe provider runtime.
#[derive(Clone)]
pub struct ProviderRuntime {
    provider_id: ProviderId,
    protocol_id: ProtocolId,
    endpoint: ResolvedEndpoint,
    auth: Arc<dyn AuthProvider>,
    client_identity: ClientIdentity,
    provider_headers: Arc<[HeaderOperation]>,
    model_headers: Arc<[HeaderOperation]>,
    capabilities: ProviderCapabilities,
    dialect: ProtocolDialect,
    transport: ProviderTransportOptions,
    pipeline: HeaderPipeline,
}

impl ProviderRuntime {
    /// Validates and freezes a profile.
    pub fn build(profile: ProviderProfile) -> Result<Self, LlmError> {
        profile.capabilities.validate()?;
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
            endpoint,
            auth,
            client_identity: profile.client_identity,
            provider_headers: profile.provider_headers.into(),
            model_headers: profile.model_headers.into(),
            capabilities: profile.capabilities,
            dialect: profile.dialect,
            transport: profile.transport,
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
    pub fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
    }

    /// Returns dialect.
    pub fn dialect(&self) -> ProtocolDialect {
        self.dialect
    }

    /// Returns transport options.
    pub fn transport_options(&self) -> ProviderTransportOptions {
        self.transport
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
        let mut model_operations = self.model_headers.to_vec();
        model_operations.extend(model);
        let request_operations = request
            .iter()
            .map(|(name, value)| HeaderOperation::set(name.clone(), value.clone()))
            .collect();
        let auth = self.auth.operation(AuthContext::new(&self.endpoint))?;
        self.pipeline.resolve(vec![
            HeaderLayer::new(HeaderSource::Protocol, protocol),
            HeaderLayer::new(HeaderSource::Provider, self.provider_headers.to_vec()),
            HeaderLayer::new(
                HeaderSource::ClientIdentity,
                vec![self.client_identity.operation()?],
            ),
            HeaderLayer::new(HeaderSource::Model, model_operations),
            HeaderLayer::new(HeaderSource::Request, request_operations),
            HeaderLayer::new(HeaderSource::Auth, vec![auth]),
        ])
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
            .field("dialect", &self.dialect)
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}
