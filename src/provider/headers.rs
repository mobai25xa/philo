//! Layered header resolution with protected-field enforcement.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::collections::HashSet;
use std::fmt;

use http::{HeaderMap, HeaderName, HeaderValue};

use crate::error::{LlmError, ValidationError, ValidationReason};

mod dynamic;
mod identity;

pub use dynamic::{
    DynamicHeaderContext, DynamicHeaderFuture, DynamicHeaderPolicy, DynamicHeaderSource,
    DynamicResponseFormat,
};
pub use identity::{ClientIdentity, ClientIdentityFragment};

/// Header source in ascending priority order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HeaderSource {
    /// HTTP transport safety defaults.
    Transport,
    /// Protocol-required headers.
    Protocol,
    /// Provider profile defaults.
    Provider,
    /// SDK or configured client identity.
    ClientIdentity,
    /// Model-specific override.
    Model,
    /// Value-free request-fact policy generated for one attempt.
    DynamicPolicy,
    /// Per-request non-sensitive override.
    Request,
    /// Authentication, applied last.
    Auth,
}

/// Header value whose formatting is always redacted.
#[derive(Clone)]
pub struct SensitiveHeaderValue {
    value: HeaderValue,
    sensitive: bool,
}

impl SensitiveHeaderValue {
    /// Wraps a structurally validated HTTP header value.
    pub fn new(value: HeaderValue, sensitive: bool) -> Self {
        Self { value, sensitive }
    }
    /// Parses bytes as an HTTP header value.
    pub fn from_bytes(bytes: &[u8], sensitive: bool) -> Result<Self, LlmError> {
        let value = HeaderValue::from_bytes(bytes).map_err(|_| {
            validation(
                "request_headers",
                ValidationReason::InvalidHeader,
                "invalid header value",
            )
        })?;
        Ok(Self::new(value, sensitive))
    }
    /// Returns whether the value must be treated as sensitive.
    pub fn is_sensitive(&self) -> bool {
        self.sensitive
    }
    pub(crate) fn value(&self) -> &HeaderValue {
        &self.value
    }
}

impl fmt::Debug for SensitiveHeaderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl fmt::Display for SensitiveHeaderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Set or remove operation applied within one header layer.
#[derive(Clone, Debug)]
pub enum HeaderOperation {
    /// Set a header value.
    Set {
        /// Case-insensitive HTTP header name.
        name: HeaderName,
        /// Redaction-aware header value.
        value: SensitiveHeaderValue,
    },
    /// Remove a header established by a lower-priority layer.
    Remove {
        /// Case-insensitive HTTP header name.
        name: HeaderName,
    },
}

impl HeaderOperation {
    /// Creates a non-sensitive Set operation.
    pub fn set(name: HeaderName, value: HeaderValue) -> Self {
        Self::Set {
            name,
            value: SensitiveHeaderValue::new(value, false),
        }
    }
    /// Creates a sensitive Set operation.
    pub fn set_sensitive(name: HeaderName, value: HeaderValue) -> Self {
        Self::Set {
            name,
            value: SensitiveHeaderValue::new(value, true),
        }
    }
    /// Creates a Remove operation.
    pub fn remove(name: HeaderName) -> Self {
        Self::Remove { name }
    }

    pub(crate) fn name(&self) -> &HeaderName {
        match self {
            Self::Set { name, .. } | Self::Remove { name } => name,
        }
    }
}

/// Operations contributed by one priority layer.
#[derive(Clone, Debug)]
pub struct HeaderLayer {
    source: HeaderSource,
    operations: Vec<HeaderOperation>,
}

impl HeaderLayer {
    /// Creates a layer while preserving operation order within that layer.
    pub fn new(source: HeaderSource, operations: Vec<HeaderOperation>) -> Self {
        Self { source, operations }
    }
    /// Returns the source.
    pub fn source(&self) -> HeaderSource {
        self.source
    }
    /// Returns operations.
    pub fn operations(&self) -> &[HeaderOperation] {
        &self.operations
    }
}

/// Resolved per-request headers and private lifecycle observation facts.
#[derive(Clone)]
pub struct ResolvedHeaders {
    headers: HeaderMap,
    steps: Vec<(HeaderName, HeaderSource, bool, bool, bool)>,
}

impl ResolvedHeaders {
    /// Returns final headers for the transport boundary.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }
    /// Consumes the result into transport headers and value-free lifecycle facts.
    #[allow(clippy::type_complexity)]
    pub(crate) fn into_parts(
        self,
    ) -> (HeaderMap, Vec<(HeaderName, HeaderSource, bool, bool, bool)>) {
        (self.headers, self.steps)
    }
    /// Returns the highest-priority source that set a final header.
    pub fn final_source(&self, name: &HeaderName) -> Option<HeaderSource> {
        if !self.headers.contains_key(name) {
            return None;
        }
        self.steps
            .iter()
            .rev()
            .find(|(entry_name, _, present, _, _)| entry_name == name && *present)
            .map(|(_, source, _, _, _)| *source)
    }
}

impl fmt::Debug for ResolvedHeaders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<_> = self.headers.keys().map(HeaderName::as_str).collect();
        f.debug_struct("ResolvedHeaders")
            .field("header_names", &names)
            .field("lifecycle_steps", &self.steps)
            .finish()
    }
}

/// Protected-header policy with explicit authentication-header registration.
#[derive(Clone, Debug)]
pub struct HeaderPolicy {
    auth_headers: HashSet<HeaderName>,
    provider_headers: HashSet<HeaderName>,
}

impl HeaderPolicy {
    /// Creates the compatibility policy with Bearer auth registered.
    pub fn new() -> Self {
        Self::with_auth_headers([http::header::AUTHORIZATION])
    }

    /// Creates a policy registering the exact authentication headers owned by Auth.
    pub fn with_auth_headers<I>(headers: I) -> Self
    where
        I: IntoIterator<Item = HeaderName>,
    {
        Self {
            auth_headers: headers.into_iter().collect(),
            provider_headers: HashSet::new(),
        }
    }

    /// Creates a policy with exact Auth and Provider-owned header registrations.
    pub fn with_registered_headers<A, P>(auth_headers: A, provider_headers: P) -> Self
    where
        A: IntoIterator<Item = HeaderName>,
        P: IntoIterator<Item = HeaderName>,
    {
        Self {
            auth_headers: auth_headers.into_iter().collect(),
            provider_headers: provider_headers.into_iter().collect(),
        }
    }

    /// Returns whether Auth owns this header.
    pub fn is_auth_header(&self, name: &HeaderName) -> bool {
        self.auth_headers.contains(name)
    }

    /// Returns whether a header is protected from ordinary layers.
    pub fn is_protected(&self, name: &HeaderName) -> bool {
        crate::protected::is_protected_header(name)
            || self.is_auth_header(name)
            || self.provider_headers.contains(name)
    }

    fn allows(&self, source: HeaderSource, name: &HeaderName) -> bool {
        match name.as_str() {
            "authorization" | "proxy-authorization" => {
                source == HeaderSource::Auth && self.is_auth_header(name)
            }
            "content-type" | "accept" => source == HeaderSource::Protocol,
            "user-agent" => source == HeaderSource::ClientIdentity,
            "host" | "content-length" | "transfer-encoding" | "connection" | "cookie"
            | "set-cookie" => false,
            _ if self.is_auth_header(name) => source == HeaderSource::Auth,
            _ if self.provider_headers.contains(name) => source == HeaderSource::Provider,
            _ => match source {
                HeaderSource::Transport | HeaderSource::Auth => false,
                HeaderSource::Protocol => true,
                HeaderSource::Provider => {
                    self.provider_headers.contains(name) || ordinary_header(name)
                }
                HeaderSource::Model | HeaderSource::DynamicPolicy | HeaderSource::Request => {
                    ordinary_header(name)
                }
                HeaderSource::ClientIdentity => name.as_str().starts_with("x-"),
            },
        }
    }
}

impl Default for HeaderPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable header pipeline that resolves fresh state for every request.
#[derive(Clone, Debug, Default)]
pub struct HeaderPipeline {
    policy: HeaderPolicy,
}

impl HeaderPipeline {
    /// Creates the official `OpenAI` header policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a pipeline using the Auth provider's registered protected headers.
    pub fn with_auth_headers<I>(headers: I) -> Self
    where
        I: IntoIterator<Item = HeaderName>,
    {
        Self {
            policy: HeaderPolicy::with_auth_headers(headers),
        }
    }

    /// Creates a pipeline with exact Auth and Provider-owned header registrations.
    pub fn with_registered_headers<A, P>(auth_headers: A, provider_headers: P) -> Self
    where
        A: IntoIterator<Item = HeaderName>,
        P: IntoIterator<Item = HeaderName>,
    {
        Self {
            policy: HeaderPolicy::with_registered_headers(auth_headers, provider_headers),
        }
    }

    /// Resolves layers by source priority and validates the final protected headers.
    pub fn resolve(&self, layers: Vec<HeaderLayer>) -> Result<ResolvedHeaders, LlmError> {
        let resolved = self.resolve_layers(layers)?;
        self.validate_compatibility(&resolved.headers)?;
        Ok(resolved)
    }

    pub(crate) fn resolve_without_auth_assumption(
        &self,
        layers: Vec<HeaderLayer>,
    ) -> Result<ResolvedHeaders, LlmError> {
        self.resolve_layers(layers)
    }

    fn resolve_layers(&self, mut layers: Vec<HeaderLayer>) -> Result<ResolvedHeaders, LlmError> {
        layers.sort_by_key(HeaderLayer::source);
        let mut headers = HeaderMap::new();
        let mut steps = Vec::new();
        for layer in layers {
            for operation in layer.operations {
                match operation {
                    HeaderOperation::Set { name, value } => {
                        self.validate_operation(layer.source, &name, value.is_sensitive())?;
                        headers.insert(name.clone(), value.value().clone());
                        let protected = self.policy.is_protected(&name);
                        steps.push((name, layer.source, true, protected, value.is_sensitive()));
                    }
                    HeaderOperation::Remove { name } => {
                        self.validate_operation(layer.source, &name, false)?;
                        headers.remove(&name);
                        let protected = self.policy.is_protected(&name);
                        steps.push((name, layer.source, false, protected, false));
                    }
                }
            }
        }
        Ok(ResolvedHeaders { headers, steps })
    }

    fn validate_operation(
        &self,
        source: HeaderSource,
        name: &HeaderName,
        sensitive: bool,
    ) -> Result<(), LlmError> {
        if self.policy.allows(source, name) {
            if source == HeaderSource::Auth && !sensitive {
                return Err(validation(
                    format!("request_headers.{name}"),
                    ValidationReason::ProtectedHeader,
                    "authentication headers must be sensitive",
                ));
            }
            let provider_owned_sensitive =
                source == HeaderSource::Provider && self.policy.provider_headers.contains(name);
            if source != HeaderSource::Auth && sensitive && !provider_owned_sensitive {
                return Err(validation(
                    format!("request_headers.{name}"),
                    ValidationReason::ProtectedHeader,
                    "ordinary header layers cannot carry sensitive values",
                ));
            }
            Ok(())
        } else {
            Err(validation(
                format!("request_headers.{name}"),
                ValidationReason::ProtectedHeader,
                "header is protected for this source",
            ))
        }
    }

    fn validate_compatibility(&self, headers: &HeaderMap) -> Result<(), LlmError> {
        if headers.get(http::header::CONTENT_TYPE)
            != Some(&HeaderValue::from_static("application/json"))
        {
            return Err(validation(
                "request_headers.content-type",
                ValidationReason::ProtectedHeader,
                "protocol must set application/json",
            ));
        }
        if !self.policy.auth_headers.is_empty()
            && !self
                .policy
                .auth_headers
                .iter()
                .any(|name| headers.contains_key(name))
        {
            return Err(validation(
                "request_headers.auth",
                ValidationReason::ProtectedHeader,
                "auth layer must set a registered authentication header",
            ));
        }
        Ok(())
    }
}

fn ordinary_header(name: &HeaderName) -> bool {
    name.as_str().starts_with("x-") || name.as_str() == "idempotency-key"
}

fn validation(
    field: impl Into<String>,
    reason: ValidationReason,
    summary: &'static str,
) -> LlmError {
    ValidationError::new(field, reason, summary).into()
}
