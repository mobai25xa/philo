//! Optional adapter from value-free lifecycle events to `tracing` events.

use tracing::{Level, event};

use super::{LifecycleEvent, LifecycleEventKind, LifecycleObserver};

/// Stateless observer that emits value-free structured events to `tracing`.
///
/// The adapter does not install a subscriber, exporter, sampler, or retention
/// policy. Request, attempt, trace, and provider request IDs are correlation
/// fields and must not be promoted to metrics labels by downstream consumers.
#[derive(Clone, Copy, Debug, Default)]
pub struct TracingObserver;

impl LifecycleObserver for TracingObserver {
    fn record(&self, lifecycle: &LifecycleEvent) {
        let identity = lifecycle.identity();
        event!(
            target: "philo::lifecycle",
            Level::INFO,
            event = event_name(lifecycle.kind()),
            elapsed_micros = lifecycle.elapsed().as_micros(),
            local_request_id = identity.local_request_id().as_str(),
            trace_id = identity.trace_id().map_or("", |value| value.as_str()),
            provider_id = identity.provider_id().as_str(),
            model_id = identity.model_id().as_str(),
            protocol_id = identity.protocol_id().as_str(),
        );
    }
}

const fn event_name(kind: &LifecycleEventKind) -> &'static str {
    match kind {
        LifecycleEventKind::RequestStarted => "request.started",
        LifecycleEventKind::ValidationCompleted => "validation.completed",
        LifecycleEventKind::IdempotencyPrepared { .. } => "idempotency.prepared",
        LifecycleEventKind::AttemptStarted { .. } => "attempt.started",
        LifecycleEventKind::CredentialResolved { .. } => "credential.resolved",
        LifecycleEventKind::EndpointResolved => "endpoint.resolved",
        LifecycleEventKind::HeadersResolved { .. } => "headers.resolved",
        LifecycleEventKind::TransportStarted => "transport.started",
        LifecycleEventKind::StatusReceived { .. } => "status.received",
        LifecycleEventKind::RateLimitObserved { .. } => "rate_limit.observed",
        LifecycleEventKind::AttemptFailed { .. } => "attempt.failed",
        LifecycleEventKind::AttemptTimedOut { .. } => "attempt.timed_out",
        LifecycleEventKind::RetryDecided { .. } => "retry.decided",
        LifecycleEventKind::RetryScheduled { .. } => "retry.scheduled",
        LifecycleEventKind::RetryExhausted { .. } => "retry.exhausted",
        LifecycleEventKind::FirstSseEvent { .. } => "stream.first_event",
        LifecycleEventKind::FirstDomainEventDelivered => "stream.first_event_delivered",
        LifecycleEventKind::FinishSeen { .. } => "stream.finish_seen",
        LifecycleEventKind::DoneSeen => "stream.done_seen",
        LifecycleEventKind::RequestCompleted { .. } => "request.completed",
        LifecycleEventKind::RequestFailed { .. } => "request.failed",
        LifecycleEventKind::RequestCancelled { .. } => "request.cancelled",
        LifecycleEventKind::RequestTimedOut { .. } => "request.timed_out",
    }
}
