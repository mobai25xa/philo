use std::sync::Arc;
use std::time::Duration;

use crate::domain::{
    FinishReason, GenerationId, LocalRequestId, ModelId, ProtocolId, ProviderId, ProviderRequestId,
    TraceId,
};
use crate::error::{AuthFailureKind, ErrorStage, LlmError};
use crate::provider::HeaderTraceEntry;

/// Stable, low-cardinality failure category for lifecycle diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LifecycleErrorCategory {
    /// SDK or provider configuration.
    Configuration,
    /// Domain validation.
    Validation,
    /// Capability preflight.
    Capability,
    /// Authentication, permission, quota, or rate limiting.
    Authentication(AuthFailureKind),
    /// Network transport stage.
    Transport(ErrorStage),
    /// Non-success HTTP response.
    HttpStatus,
    /// SSE, JSON, or protocol state.
    Protocol(ErrorStage),
    /// Response semantics unsupported by this SDK phase.
    UnsupportedResponseSemantics,
    /// Unknown completion reason.
    UnknownFinishReason,
    /// Stream ended without a successful terminal event.
    TruncatedStream,
    /// Overall deadline elapsed.
    Timeout,
    /// Caller cancellation.
    Cancelled,
}

impl LifecycleErrorCategory {
    pub(crate) fn from_error(error: &LlmError) -> Self {
        match error {
            LlmError::Configuration(_) => Self::Configuration,
            LlmError::Validation(_)
            | LlmError::Schema(_)
            | LlmError::ToolValidation(_)
            | LlmError::History(_)
            | LlmError::StructuredOutput(_)
            | LlmError::Cost(_) => Self::Validation,
            LlmError::Capability(_) => Self::Capability,
            LlmError::Authentication(error) => Self::Authentication(error.kind()),
            LlmError::Transport(error) => Self::Transport(error.stage()),
            LlmError::HttpStatus(_) => Self::HttpStatus,
            LlmError::Protocol(error) => Self::Protocol(error.stage()),
            LlmError::UnsupportedResponseSemantics(_) => Self::UnsupportedResponseSemantics,
            LlmError::UnknownFinishReason(_) => Self::UnknownFinishReason,
            LlmError::TruncatedStream(_) => Self::TruncatedStream,
            LlmError::Timeout(_) => Self::Timeout,
            LlmError::Cancelled => Self::Cancelled,
        }
    }
}

/// Immutable identifiers shared by lifecycle events for one SDK call.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::struct_field_names)]
pub struct LifecycleIdentity {
    local_request_id: LocalRequestId,
    trace_id: Option<TraceId>,
    provider_id: ProviderId,
    model_id: ModelId,
    protocol_id: ProtocolId,
}

impl LifecycleIdentity {
    pub(crate) fn new(
        local_request_id: LocalRequestId,
        trace_id: Option<TraceId>,
        provider_id: ProviderId,
        model_id: ModelId,
        protocol_id: ProtocolId,
    ) -> Self {
        Self {
            local_request_id,
            trace_id,
            provider_id,
            model_id,
            protocol_id,
        }
    }

    /// Returns the SDK-generated ID for this call.
    #[must_use]
    pub fn local_request_id(&self) -> &LocalRequestId {
        &self.local_request_id
    }

    /// Returns caller telemetry correlation, when supplied.
    #[must_use]
    pub fn trace_id(&self) -> Option<&TraceId> {
        self.trace_id.as_ref()
    }

    /// Returns the configured provider ID.
    #[must_use]
    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    /// Returns the requested model ID.
    #[must_use]
    pub fn model_id(&self) -> &ModelId {
        &self.model_id
    }

    /// Returns the configured protocol ID.
    #[must_use]
    pub fn protocol_id(&self) -> &ProtocolId {
        &self.protocol_id
    }
}

/// Structured lifecycle transition without prompt, output, body, or header values.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LifecycleEventKind {
    /// Request orchestration began and a local request ID was allocated.
    RequestStarted,
    /// Domain and capability validation completed.
    ValidationCompleted,
    /// The immutable endpoint was selected.
    EndpointResolved,
    /// Header and authentication layers resolved successfully.
    HeadersResolved {
        /// Value-free header resolution records.
        trace: Arc<[HeaderTraceEntry]>,
    },
    /// Transport execution began.
    TransportStarted,
    /// Response status and provider request ID were captured before body reads.
    StatusReceived {
        /// Numeric HTTP status.
        status: u16,
        /// Provider response-header identifier, when valid.
        provider_request_id: Option<ProviderRequestId>,
    },
    /// The first parsed domain event became available.
    FirstSseEvent {
        /// Provider response-header identifier, when available.
        provider_request_id: Option<ProviderRequestId>,
        /// Generation body identifier, when available.
        generation_id: Option<GenerationId>,
    },
    /// A supported finish reason was observed.
    FinishSeen {
        /// Normalized supported reason.
        finish_reason: FinishReason,
    },
    /// The protocol completion marker was accepted.
    DoneSeen,
    /// The request completed successfully.
    RequestCompleted {
        /// Number of domain events returned, including Done.
        event_count: u64,
        /// Whether the provider supplied usage.
        usage_known: bool,
    },
    /// The request failed before successful completion.
    RequestFailed {
        /// Stable error classification.
        category: LifecycleErrorCategory,
        /// Whether at least one non-empty text delta was returned.
        partial_output: bool,
    },
    /// The caller cancelled or dropped the request.
    RequestCancelled {
        /// Whether at least one non-empty text delta was returned.
        partial_output: bool,
    },
    /// The overall deadline elapsed.
    RequestTimedOut {
        /// Whether at least one non-empty text delta was returned.
        partial_output: bool,
    },
}

/// One lifecycle event with shared request identity and elapsed time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleEvent {
    identity: Arc<LifecycleIdentity>,
    elapsed: Duration,
    kind: LifecycleEventKind,
}

impl LifecycleEvent {
    pub(crate) fn new(
        identity: Arc<LifecycleIdentity>,
        elapsed: Duration,
        kind: LifecycleEventKind,
    ) -> Self {
        Self {
            identity,
            elapsed,
            kind,
        }
    }

    /// Returns stable identifiers for this call.
    #[must_use]
    pub fn identity(&self) -> &LifecycleIdentity {
        &self.identity
    }

    /// Returns elapsed time since request orchestration began.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Returns the structured transition.
    #[must_use]
    pub fn kind(&self) -> &LifecycleEventKind {
        &self.kind
    }
}

/// Synchronous sink for small, value-free lifecycle events.
///
/// Implementations must avoid blocking. The SDK performs no diagnostic body,
/// prompt, or output copies when no observer is configured.
pub trait LifecycleObserver: Send + Sync {
    /// Records one lifecycle transition.
    fn record(&self, event: &LifecycleEvent);
}
