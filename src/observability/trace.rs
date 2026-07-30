use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use crate::domain::{
    FinishReason, GenerationId, LocalRequestId, ModelId, ProtocolId, ProviderId, ProviderRequestId,
    TraceId,
};
use crate::error::{AuthFailureKind, ErrorStage, LlmError, RetryReason, TimeoutStage};
use crate::provider::{HeaderSource, IdempotencyCapability, IdempotencyKeySource};
use http::HeaderName;

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
    /// Response semantics unsupported by this SDK implementation.
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

/// SDK-generated identifier for one provider HTTP attempt.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AttemptId(String);

impl AttemptId {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// Returns the opaque attempt identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Typed identity for one one-based provider attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptIdentity {
    id: AttemptId,
    number: u32,
}

/// Stable reason why the retry boundary did not authorize another attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RetryStopReason {
    /// The failed attempt was not classified as retryable.
    NonRetryable,
    /// A domain event had already crossed the public delivery boundary.
    DeliveryBoundaryClosed,
    /// The configured attempt limit had been reached.
    AttemptsExhausted,
    /// The overall deadline could not fit the wait and minimum next-attempt budget.
    DeadlineInsufficient,
    /// Replaying this logical request was not considered safe.
    ReplayUnsafe,
}

impl AttemptIdentity {
    pub(crate) fn new(id: AttemptId, number: u32) -> Self {
        debug_assert!(number > 0);
        Self { id, number }
    }

    /// Returns the SDK-generated attempt ID.
    #[must_use]
    pub fn id(&self) -> &AttemptId {
        &self.id
    }

    /// Returns the one-based attempt number.
    #[must_use]
    pub const fn number(&self) -> u32 {
        self.number
    }
}

impl LifecycleErrorCategory {
    pub(crate) fn from_error(error: &LlmError) -> Self {
        match error {
            LlmError::Configuration(_)
            | LlmError::ProviderConfig(_)
            | LlmError::ProviderRegistry(_)
            | LlmError::Credential(_)
            | LlmError::HeaderPolicy(_) => Self::Configuration,
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
    /// Logical request idempotency was resolved without exposing the key value.
    IdempotencyPrepared {
        /// Reviewed provider capability.
        capability: IdempotencyCapability,
        /// Whether this logical request carries a key.
        present: bool,
        /// Caller or SDK source when a key is present.
        source: Option<IdempotencyKeySource>,
    },
    /// A fresh provider attempt began.
    AttemptStarted {
        /// Typed identity unique within the logical request.
        attempt: AttemptIdentity,
    },
    /// Dynamic credential and protected header resolution completed.
    CredentialResolved {
        /// Attempt whose credential/header snapshot was resolved.
        attempt: AttemptIdentity,
    },
    /// The immutable endpoint was selected.
    EndpointResolved,
    /// Header and authentication layers resolved successfully.
    HeadersResolved {
        /// Ordered value-free `(name, source, present, protected, sensitive)` records.
        #[allow(clippy::type_complexity)]
        steps: Arc<[(HeaderName, HeaderSource, bool, bool, bool)]>,
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
    /// Typed rate-limit response metadata was observed without retaining raw header values.
    RateLimitObserved {
        /// Whether this attempt received HTTP 429.
        status_is_rate_limited: bool,
        /// Whether a valid standard or typed provider retry delay was present.
        retry_after_valid: bool,
        /// Whether any typed provider quota/reset field was present.
        provider_fields_present: bool,
    },
    /// One attempt failed before logical request completion.
    AttemptFailed {
        /// Identity of the failed attempt.
        attempt: AttemptIdentity,
        /// Stable value-free error category.
        category: LifecycleErrorCategory,
    },
    /// One attempt ended in a precisely classified timeout.
    AttemptTimedOut {
        /// Identity of the timed-out attempt.
        attempt: AttemptIdentity,
        /// Precise lifecycle stage whose budget elapsed.
        stage: TimeoutStage,
        /// Whether the overall deadline shortened the stage timeout.
        overall_limited: bool,
    },
    /// The retry policy made a value-free decision for a failed attempt.
    RetryDecided {
        /// Identity of the failed attempt.
        attempt: AttemptIdentity,
        /// Retry reason when another attempt was authorized.
        reason: Option<RetryReason>,
        /// Stable stop reason when another attempt was not authorized.
        stop_reason: Option<RetryStopReason>,
    },
    /// The runner authorized a new attempt without changing provider route.
    RetryScheduled {
        /// Identity of the failed attempt.
        previous_attempt: AttemptIdentity,
        /// One-based number of the next attempt.
        next_attempt_number: u32,
        /// Stable low-cardinality decision reason.
        reason: RetryReason,
        /// Effective bounded wait before the next attempt.
        delay: Duration,
        /// Whether a valid server delay contributed to the effective wait.
        server_delay_applied: bool,
        /// Whether a valid server delay was reduced to the configured safety cap.
        server_delay_capped: bool,
    },
    /// A retryable failure could not open another attempt within the active bounds.
    RetryExhausted {
        /// Identity of the final failed attempt.
        attempt: AttemptIdentity,
        /// Stable reason that closed the retry path.
        reason: RetryStopReason,
    },
    /// The first parsed domain event became available.
    FirstSseEvent {
        /// Provider response-header identifier, when available.
        provider_request_id: Option<ProviderRequestId>,
        /// Generation body identifier, when available.
        generation_id: Option<GenerationId>,
    },
    /// The first parsed domain event crossed the public delivery boundary.
    FirstDomainEventDelivered,
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
        /// Whether at least one domain event was returned to the caller.
        partial_output: bool,
    },
    /// The caller cancelled or dropped the request.
    RequestCancelled {
        /// Whether at least one domain event was returned to the caller.
        partial_output: bool,
    },
    /// The overall deadline elapsed.
    RequestTimedOut {
        /// Whether at least one domain event was returned to the caller.
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

pub(crate) fn record_safely(observer: &dyn LifecycleObserver, event: &LifecycleEvent) {
    let _ = catch_unwind(AssertUnwindSafe(|| observer.record(event)));
}
