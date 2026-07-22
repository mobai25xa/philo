//! Layered header resolution with protected-field enforcement and value-free tracing.
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt;

use http::{HeaderMap, HeaderName, HeaderValue};

use crate::error::{LlmError, ValidationError, ValidationReason};

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

/// Value-free operation kind stored in a resolution trace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceOperation {
    /// A Set operation.
    Set,
    /// A Remove operation.
    Remove,
}

/// Final decision for a header operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceDecision {
    /// Value is present after this operation.
    Set,
    /// Value is absent after this operation.
    Removed,
}

/// One value-free header resolution record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeaderTraceEntry {
    name: HeaderName,
    source: HeaderSource,
    operation: TraceOperation,
    decision: TraceDecision,
    protected: bool,
    sensitive: bool,
}

impl HeaderTraceEntry {
    /// Returns normalized header name.
    pub fn name(&self) -> &HeaderName {
        &self.name
    }
    /// Returns source layer.
    pub fn source(&self) -> HeaderSource {
        self.source
    }
    /// Returns operation kind.
    pub fn operation(&self) -> TraceOperation {
        self.operation
    }
    /// Returns final operation decision.
    pub fn decision(&self) -> TraceDecision {
        self.decision
    }
    /// Returns whether policy protects the header.
    pub fn is_protected(&self) -> bool {
        self.protected
    }
    /// Returns whether the value was sensitive.
    pub fn is_sensitive(&self) -> bool {
        self.sensitive
    }
}

/// Resolved per-request headers and a value-free trace.
#[derive(Clone)]
pub struct ResolvedHeaders {
    headers: HeaderMap,
    trace: Vec<HeaderTraceEntry>,
}

impl ResolvedHeaders {
    /// Returns final headers for the transport boundary.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }
    /// Returns the value-free resolution trace.
    pub fn trace(&self) -> &[HeaderTraceEntry] {
        &self.trace
    }
    /// Consumes the result into final headers and its value-free trace.
    pub fn into_parts(self) -> (HeaderMap, Vec<HeaderTraceEntry>) {
        (self.headers, self.trace)
    }
    /// Returns the highest-priority source that set a final header.
    pub fn final_source(&self, name: &HeaderName) -> Option<HeaderSource> {
        if !self.headers.contains_key(name) {
            return None;
        }
        self.trace
            .iter()
            .rev()
            .find(|entry| entry.name() == name && entry.operation() == TraceOperation::Set)
            .map(HeaderTraceEntry::source)
    }
}

impl fmt::Debug for ResolvedHeaders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<_> = self.headers.keys().map(HeaderName::as_str).collect();
        f.debug_struct("ResolvedHeaders")
            .field("header_names", &names)
            .field("trace", &self.trace)
            .finish()
    }
}

/// Protected-header policy.
#[derive(Clone, Debug, Default)]
pub struct HeaderPolicy;

impl HeaderPolicy {
    /// Returns whether a header is protected from ordinary layers.
    pub fn is_protected(&self, name: &HeaderName) -> bool {
        matches!(
            name.as_str(),
            "authorization"
                | "proxy-authorization"
                | "host"
                | "content-length"
                | "content-type"
                | "transfer-encoding"
                | "connection"
                | "cookie"
        )
    }

    fn allows(&self, source: HeaderSource, name: &HeaderName) -> bool {
        match name.as_str() {
            "authorization" => source == HeaderSource::Auth,
            "content-type" => source == HeaderSource::Protocol,
            _ if self.is_protected(name) => false,
            _ => true,
        }
    }
}

/// Immutable header pipeline that resolves fresh state for every request.
#[derive(Clone, Debug, Default)]
pub struct HeaderPipeline {
    policy: HeaderPolicy,
}

impl HeaderPipeline {
    /// Creates the phase-one header policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves layers by source priority and validates the final protected headers.
    pub fn resolve(&self, layers: Vec<HeaderLayer>) -> Result<ResolvedHeaders, LlmError> {
        let resolved = self.resolve_layers(layers)?;
        Self::validate_bearer_compatibility(&resolved.headers)?;
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
        let mut trace = Vec::new();
        for layer in layers {
            for operation in layer.operations {
                match operation {
                    HeaderOperation::Set { name, value } => {
                        self.validate_operation(layer.source, &name)?;
                        headers.insert(name.clone(), value.value().clone());
                        trace.push(HeaderTraceEntry {
                            protected: self.policy.is_protected(&name),
                            name,
                            source: layer.source,
                            operation: TraceOperation::Set,
                            decision: TraceDecision::Set,
                            sensitive: value.is_sensitive(),
                        });
                    }
                    HeaderOperation::Remove { name } => {
                        self.validate_operation(layer.source, &name)?;
                        headers.remove(&name);
                        trace.push(HeaderTraceEntry {
                            protected: self.policy.is_protected(&name),
                            name,
                            source: layer.source,
                            operation: TraceOperation::Remove,
                            decision: TraceDecision::Removed,
                            sensitive: false,
                        });
                    }
                }
            }
        }
        Ok(ResolvedHeaders { headers, trace })
    }

    fn validate_operation(&self, source: HeaderSource, name: &HeaderName) -> Result<(), LlmError> {
        if self.policy.allows(source, name) {
            Ok(())
        } else {
            Err(validation(
                format!("request_headers.{name}"),
                ValidationReason::ProtectedHeader,
                "header is protected for this source",
            ))
        }
    }

    fn validate_bearer_compatibility(headers: &HeaderMap) -> Result<(), LlmError> {
        if headers.get(http::header::CONTENT_TYPE)
            != Some(&HeaderValue::from_static("application/json"))
        {
            return Err(validation(
                "request_headers.content-type",
                ValidationReason::ProtectedHeader,
                "protocol must set application/json",
            ));
        }
        if !headers.contains_key(http::header::AUTHORIZATION) {
            return Err(validation(
                "request_headers.authorization",
                ValidationReason::ProtectedHeader,
                "auth layer must set authorization",
            ));
        }
        Ok(())
    }
}

fn validation(
    field: impl Into<String>,
    reason: ValidationReason,
    summary: &'static str,
) -> LlmError {
    ValidationError::new(field, reason, summary).into()
}
