//! Structured, redacted error taxonomy.
#![allow(clippy::must_use_candidate)]

use std::fmt;
use thiserror::Error;

use crate::domain::ProviderRequestId;

/// Whether an operation may be retried by a caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RetriableHint {
    /// Retrying is not expected to help.
    No,
    /// Retrying may help, subject to caller policy and lifecycle state.
    Maybe,
}

/// Phase where an error occurred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorStage {
    /// SDK or provider configuration.
    Configuration,
    /// Domain request validation.
    Validation,
    /// Capability preflight validation.
    Capability,
    /// Network connection setup.
    Connect,
    /// TLS setup or handshake.
    Tls,
    /// Response body reading.
    Body,
    /// HTTP status processing.
    Http,
    /// Server-sent event framing.
    Sse,
    /// JSON decoding.
    Json,
    /// Protocol or event state transition.
    Protocol,
    /// Overall timeout or deadline.
    Timeout,
}

/// Specific authentication-family failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AuthFailureKind {
    /// Credentials were absent or rejected.
    Authentication,
    /// The credential lacks permission for the operation.
    Permission,
    /// Account or project quota was exhausted.
    Quota,
    /// The provider asked the caller to slow down.
    RateLimit,
}

/// Validation reason code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ValidationReason {
    /// Required input is empty.
    Empty,
    /// Input has leading or trailing whitespace.
    BoundaryWhitespace,
    /// Model reference has no provider.
    MissingProvider,
    /// Request model provider does not match the configured client runtime.
    ProviderMismatch,
    /// Message list has no user message.
    MissingUserMessage,
    /// User text contains only whitespace.
    EmptyUserText,
    /// Message has an unsupported number of text parts.
    TextPartCount,
    /// Floating-point value is NaN or infinite.
    NonFinite,
    /// Numeric value is outside the accepted range.
    OutOfRange,
    /// Positive value was zero.
    Zero,
    /// Value cannot be represented safely.
    Overflow,
    /// Deadline has elapsed.
    Expired,
    /// Header is structurally invalid.
    InvalidHeader,
    /// Header is owned by authentication, transport, or the adapter.
    ProtectedHeader,
    /// Selected model explicitly does not support the option.
    CapabilityUnsupported,
    /// Selected model support for the option is unknown.
    CapabilityUnknown,
    /// Local metadata key is invalid.
    InvalidMetadata,
    /// Identifier does not match its frozen character set.
    InvalidIdentifier,
    /// A request declared the same tool name more than once.
    DuplicateToolName,
    /// Tool-choice or parallel options were set without tools.
    EmptyToolList,
    /// A specific tool choice referenced an undeclared tool.
    UnknownTool,
}
impl fmt::Display for ValidationReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A safe validation failure with a field path and non-sensitive summary.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[error("{field}: {reason} ({summary})")]
pub struct ValidationError {
    field: String,
    reason: ValidationReason,
    summary: String,
}
impl ValidationError {
    /// Creates a validation error. The summary must not contain user or secret values.
    pub fn new(
        field: impl Into<String>,
        reason: ValidationReason,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            reason,
            summary: summary.into(),
        }
    }
    /// Returns the field path.
    pub fn field(&self) -> &str {
        &self.field
    }
    /// Returns the stable reason code.
    pub fn reason(&self) -> ValidationReason {
        self.reason
    }
    /// Returns the safe summary.
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

/// A capability validation failure.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[error("{field}: capability {capability} is {state}")]
pub struct CapabilityError {
    field: String,
    capability: String,
    state: String,
}
impl CapabilityError {
    /// Creates a capability error without retaining a request value.
    pub fn new(
        field: impl Into<String>,
        capability: impl Into<String>,
        state: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            capability: capability.into(),
            state: state.into(),
        }
    }
    /// Returns the field path.
    pub fn field(&self) -> &str {
        &self.field
    }
    /// Returns capability name.
    pub fn capability(&self) -> &str {
        &self.capability
    }
    /// Returns capability state.
    pub fn state(&self) -> &str {
        &self.state
    }
}

/// A bounded, safe response-body summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BodySummary(String);
impl BodySummary {
    /// Captures at most `limit` bytes and marks truncation.
    pub fn from_bytes(bytes: &[u8], limit: usize) -> Self {
        let truncated = bytes.len() > limit;
        let shown = &bytes[..bytes.len().min(limit)];
        Self::from_prefix(shown, truncated)
    }
    /// Builds a summary from an already bounded prefix and explicit truncation state.
    pub fn from_prefix(bytes: &[u8], truncated: bool) -> Self {
        let mut value = redact_body(&String::from_utf8_lossy(bytes));
        if truncated {
            value.push_str("... [truncated]");
        }
        Self(value)
    }
    /// Returns the safe body summary.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn redact_body(value: &str) -> String {
    const SENSITIVE_MARKERS: [&str; 7] = [
        "authorization",
        "api_key",
        "api-key",
        "bearer ",
        "secret",
        "access_token",
        "sk-",
    ];
    let lower = value.to_ascii_lowercase();
    if SENSITIVE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        "<redacted error body>".to_owned()
    } else {
        value.to_owned()
    }
}
impl fmt::Display for BodySummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Authentication, permission, quota, or rate-limit failure.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[error("authentication/permission failure at {stage:?}")]
pub struct AuthenticationError {
    kind: AuthFailureKind,
    stage: ErrorStage,
    hint: RetriableHint,
}
impl AuthenticationError {
    /// Creates an authentication failure.
    pub fn new(kind: AuthFailureKind, stage: ErrorStage, hint: RetriableHint) -> Self {
        Self { kind, stage, hint }
    }
    /// Returns stage.
    pub fn stage(&self) -> ErrorStage {
        self.stage
    }
    /// Returns the authentication-family kind.
    pub fn kind(&self) -> AuthFailureKind {
        self.kind
    }
    /// Returns retry hint.
    pub fn retriable(&self) -> RetriableHint {
        self.hint
    }
}

/// Transport failure with a classified stage.
#[derive(Error)]
#[error("transport failure at {stage:?}")]
pub struct TransportError {
    stage: ErrorStage,
    hint: RetriableHint,
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Debug for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransportError")
            .field("stage", &self.stage)
            .field("hint", &self.hint)
            .field("has_source", &self.source.is_some())
            .finish()
    }
}
impl TransportError {
    /// Creates a transport error without a source.
    pub fn new(stage: ErrorStage, hint: RetriableHint) -> Self {
        Self {
            stage,
            hint,
            source: None,
        }
    }
    /// Creates a transport error with a source chain.
    pub fn with_source(
        stage: ErrorStage,
        hint: RetriableHint,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            stage,
            hint,
            source: Some(Box::new(source)),
        }
    }
    /// Returns stage.
    pub fn stage(&self) -> ErrorStage {
        self.stage
    }
    /// Returns retry hint.
    pub fn retriable(&self) -> RetriableHint {
        self.hint
    }
}

/// HTTP status failure with a bounded body summary.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[error("HTTP status {status} ({stage:?}): {body}")]
pub struct HttpStatusError {
    status: u16,
    stage: ErrorStage,
    body: BodySummary,
    request_id: Option<ProviderRequestId>,
    hint: RetriableHint,
}
impl HttpStatusError {
    /// Creates an HTTP status error with a bounded body.
    pub fn new(
        status: u16,
        body: BodySummary,
        request_id: Option<ProviderRequestId>,
        hint: RetriableHint,
    ) -> Self {
        Self {
            status,
            stage: ErrorStage::Http,
            body,
            request_id,
            hint,
        }
    }
    /// Returns status code.
    pub fn status(&self) -> u16 {
        self.status
    }
    /// Returns bounded body summary.
    pub fn body(&self) -> &BodySummary {
        &self.body
    }
    /// Returns provider request id.
    pub fn request_id(&self) -> Option<&ProviderRequestId> {
        self.request_id.as_ref()
    }
    /// Returns retry hint.
    pub fn retriable(&self) -> RetriableHint {
        self.hint
    }
}

/// Protocol or event-state failure.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[error("protocol error at {stage:?}: {message}")]
pub struct ProtocolError {
    stage: ErrorStage,
    message: String,
}
impl ProtocolError {
    /// Creates a protocol error from a safe message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            stage: ErrorStage::Protocol,
            message: message.into(),
        }
    }
    /// Creates a protocol error classified at a specific parsing stage.
    pub fn at_stage(stage: ErrorStage, message: impl Into<String>) -> Self {
        debug_assert!(matches!(
            stage,
            ErrorStage::Sse | ErrorStage::Json | ErrorStage::Protocol
        ));
        Self {
            stage,
            message: message.into(),
        }
    }
    /// Returns the parsing or state-machine stage.
    pub fn stage(&self) -> ErrorStage {
        self.stage
    }
    /// Returns diagnostic message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A response semantic the current phase cannot represent.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[error("unsupported response semantics: {raw}")]
pub struct UnsupportedResponseSemantics {
    raw: String,
}
impl UnsupportedResponseSemantics {
    /// Creates an unsupported-semantics error.
    pub fn new(raw: impl Into<String>) -> Self {
        Self { raw: raw.into() }
    }
    /// Returns raw semantic label.
    pub fn raw(&self) -> &str {
        &self.raw
    }
}

/// A finish reason that was not recognized.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[error("unknown finish reason: {raw}")]
pub struct UnknownFinishReason {
    raw: String,
}
impl UnknownFinishReason {
    /// Creates an unknown finish reason.
    pub fn new(raw: impl Into<String>) -> Self {
        Self { raw: raw.into() }
    }
    /// Returns original reason.
    pub fn raw(&self) -> &str {
        &self.raw
    }
}

/// A stream ended without a terminal Done event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[error("stream ended before Done")]
pub struct TruncatedStreamError;

/// Timeout stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[error("timed out at {stage:?}")]
pub struct TimeoutError {
    stage: ErrorStage,
}
impl TimeoutError {
    /// Creates a timeout error.
    pub fn new(stage: ErrorStage) -> Self {
        Self { stage }
    }
    /// Returns timeout stage.
    pub fn stage(&self) -> ErrorStage {
        self.stage
    }
}

/// Local schema compilation or strictness failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SchemaFailure {
    /// Schema root or node is not a JSON object.
    NotAnObject,
    /// A supported keyword has an invalid type or combination.
    InvalidKeywordType,
    /// A keyword is outside the frozen phase-two subset.
    UnsupportedKeyword,
    /// A remote `$ref` was present.
    RemoteReference,
    /// A local `$ref` could not be resolved.
    UnresolvedLocalReference,
    /// Schema size or maxItems exceeded limits.
    TooLarge,
    /// Schema nesting exceeded limits.
    TooDeep,
    /// Strict mode requires `additionalProperties: false`.
    StrictObjectMissingAdditionalPropertiesFalse,
    /// Strict mode requires every property to be listed in `required`.
    StrictPropertyNotRequired,
}

/// A safe schema failure with a field path and non-sensitive summary.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[error("{field}: {reason:?} ({message})")]
pub struct SchemaError {
    field: String,
    reason: SchemaFailure,
    path: Option<String>,
    message: &'static str,
}

impl SchemaError {
    /// Creates a schema error. The message must not contain schema values.
    pub fn new(
        field: impl Into<String>,
        reason: SchemaFailure,
        path: Option<String>,
        message: &'static str,
    ) -> Self {
        Self {
            field: field.into(),
            reason,
            path,
            message,
        }
    }

    /// Returns the field path.
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Returns the stable failure code.
    pub fn reason(&self) -> SchemaFailure {
        self.reason
    }

    /// Returns the optional JSON Pointer path.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Returns the safe summary.
    pub fn message(&self) -> &'static str {
        self.message
    }
}

/// Tool-call argument validation failure codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ToolValidationFailure {
    /// Arguments are not complete JSON.
    InvalidJson,
    /// Arguments do not satisfy the tool schema.
    SchemaViolation,
    /// No matching tool was declared.
    UnknownTool,
    /// Arguments exceeded the configured size limit.
    ArgumentsTooLarge,
    /// Arguments exceeded the configured nesting limit.
    ArgumentsTooDeep,
}

/// A safe tool argument validation failure.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[error("{field}: {reason:?} ({message})")]
pub struct ToolValidationError {
    field: String,
    reason: ToolValidationFailure,
    path: Option<String>,
    message: &'static str,
}

impl ToolValidationError {
    /// Creates a tool validation error without retaining argument values.
    pub fn new(
        field: impl Into<String>,
        reason: ToolValidationFailure,
        path: Option<String>,
        message: &'static str,
    ) -> Self {
        Self {
            field: field.into(),
            reason,
            path,
            message,
        }
    }

    /// Returns the field path.
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Returns the stable failure code.
    pub fn reason(&self) -> ToolValidationFailure {
        self.reason
    }

    /// Returns the optional JSON Pointer path.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Returns the safe summary.
    pub fn message(&self) -> &'static str {
        self.message
    }
}

/// All public SDK failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LlmError {
    /// Configuration failure.
    #[error("configuration: {0}")]
    Configuration(String),
    /// Request validation failure.
    #[error("validation: {0}")]
    Validation(#[from] ValidationError),
    /// Capability failure.
    #[error("capability: {0}")]
    Capability(#[from] CapabilityError),
    /// Schema compilation failure.
    #[error("schema: {0}")]
    Schema(#[from] SchemaError),
    /// Tool argument validation failure.
    #[error("tool validation: {0}")]
    ToolValidation(#[from] ToolValidationError),
    /// Authentication, permission, quota, or rate-limit failure.
    #[error("{0}")]
    Authentication(#[from] AuthenticationError),
    /// Transport failure.
    #[error("{0}")]
    Transport(#[from] TransportError),
    /// HTTP status failure.
    #[error("{0}")]
    HttpStatus(#[from] HttpStatusError),
    /// SSE, JSON, or event-state protocol failure.
    #[error("{0}")]
    Protocol(#[from] ProtocolError),
    /// Unsupported response semantics.
    #[error("{0}")]
    UnsupportedResponseSemantics(#[from] UnsupportedResponseSemantics),
    /// Unknown finish reason.
    #[error("{0}")]
    UnknownFinishReason(#[from] UnknownFinishReason),
    /// Truncated stream.
    #[error("{0}")]
    TruncatedStream(#[from] TruncatedStreamError),
    /// Timeout.
    #[error("{0}")]
    Timeout(#[from] TimeoutError),
    /// Caller cancellation.
    #[error("cancelled")]
    Cancelled,
}
