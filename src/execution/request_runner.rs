//! Logical-request orchestration over independently rebuilt HTTP attempts.

use std::sync::Arc;

use futures_util::{StreamExt as _, stream};
use uuid::Uuid;

use crate::domain::LocalRequestId;
use crate::error::{LlmError, TimeoutStage, TruncatedStreamError};
use crate::observability::{
    AttemptId, AttemptIdentity, LifecycleErrorCategory, LifecycleEventKind, RetryStopReason,
};
use crate::protocol::{EventStream, PreparedCall, ResponseSession};
use crate::provider::{ProviderRuntime, ResolvedIdempotency};
use crate::transport::{RequestLifecycle, Transport, await_stream_with_stage};

use super::executor::{AttemptContext, AttemptExecutor, AttemptObservation};
use super::reliability::{
    RequestExecutionState, RetryDecision, RetryPolicy, RetryWaitPolicy, TimeoutPolicy,
    calculate_retry_wait, decide_retry, remaining, wait_for_retry,
};

/// Owns the immutable logical call and rebuilds all attempt-local state.
pub(crate) struct RequestRunner {
    runtime: Arc<ProviderRuntime>,
    executor: AttemptExecutor,
    prepared: PreparedCall,
    local_request_id: LocalRequestId,
    lifecycle: RequestLifecycle,
    timeouts: TimeoutPolicy,
    retries: RetryPolicy,
    retry_wait: RetryWaitPolicy,
    idempotency: ResolvedIdempotency,
    total_waited: std::time::Duration,
    observation: Option<AttemptObservation>,
    execution_state: Arc<RequestExecutionState>,
}

impl RequestRunner {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        runtime: Arc<ProviderRuntime>,
        transport: Arc<dyn Transport>,
        prepared: PreparedCall,
        local_request_id: LocalRequestId,
        lifecycle: RequestLifecycle,
        timeouts: TimeoutPolicy,
        retries: RetryPolicy,
        retry_wait: RetryWaitPolicy,
        idempotency: ResolvedIdempotency,
        observation: Option<AttemptObservation>,
        execution_state: Arc<RequestExecutionState>,
    ) -> Self {
        Self {
            runtime,
            executor: AttemptExecutor::new(transport),
            prepared,
            local_request_id,
            lifecycle,
            timeouts,
            retries,
            retry_wait,
            idempotency,
            total_waited: std::time::Duration::ZERO,
            observation,
            execution_state,
        }
    }

    pub(crate) async fn start(mut self) -> Result<EventStream, LlmError> {
        crate::transport::lifecycle_preflight(&self.lifecycle)?;
        self.emit(LifecycleEventKind::IdempotencyPrepared {
            capability: self.idempotency.capability(),
            present: self.idempotency.is_present(),
            source: self.idempotency.source(),
        });
        let mut attempt_number = 1;
        loop {
            let (attempt, result) = self.open_attempt(attempt_number).await;
            match result {
                Ok(active) => {
                    let state = ActiveAttempt {
                        runner: self,
                        attempt,
                        active,
                        event_seen: false,
                        terminal: false,
                    };
                    return Ok(Box::pin(stream::unfold(state, poll_active_attempt)));
                }
                Err(error) => {
                    let Some(next) = self.retry_after_failure(&error, &attempt).await? else {
                        return Err(error);
                    };
                    attempt_number = next;
                }
            }
        }
    }

    async fn open_attempt(
        &self,
        attempt_number: u32,
    ) -> (AttemptIdentity, Result<EventStream, LlmError>) {
        let attempt =
            AttemptIdentity::new(AttemptId::new(Uuid::new_v4().to_string()), attempt_number);
        self.execution_state.begin_attempt();
        self.emit(LifecycleEventKind::AttemptStarted {
            attempt: attempt.clone(),
        });
        let response = self
            .executor
            .execute(
                &self.runtime,
                self.prepared.clone(),
                AttemptContext {
                    local_request_id: self.local_request_id.clone(),
                    attempt: attempt.clone(),
                    lifecycle: self.lifecycle.clone(),
                    timeouts: self.timeouts,
                    observation: self.observation.clone(),
                    idempotency: self.idempotency.clone(),
                },
            )
            .await
            .and_then(ResponseSession::open);
        (attempt, response)
    }

    async fn retry_after_failure(
        &mut self,
        error: &LlmError,
        attempt: &AttemptIdentity,
    ) -> Result<Option<u32>, LlmError> {
        self.emit(LifecycleEventKind::AttemptFailed {
            attempt: attempt.clone(),
            category: LifecycleErrorCategory::from_error(error),
        });
        if let LlmError::Timeout(timeout) = error {
            self.emit(LifecycleEventKind::AttemptTimedOut {
                attempt: attempt.clone(),
                stage: timeout.timeout_stage(),
                overall_limited: timeout.overall_limited(),
            });
        }
        let Some(next_attempt_number) = attempt.number().checked_add(1) else {
            self.emit_retry_stopped(attempt, RetryStopReason::AttemptsExhausted);
            return Ok(None);
        };
        match decide_retry(
            error,
            self.execution_state.delivery_state(),
            attempt.number(),
            self.retries,
            remaining(&self.lifecycle),
            self.idempotency.replay_safe(),
        ) {
            RetryDecision::Fail => {
                let reason = self.retry_stop_reason(error, attempt.number());
                self.emit_retry_stopped(attempt, reason);
                Ok(None)
            }
            RetryDecision::Retry(reason) => {
                self.emit(LifecycleEventKind::RetryDecided {
                    attempt: attempt.clone(),
                    reason: Some(reason),
                    stop_reason: None,
                });
                // The first retry after attempt one uses retry_index zero.
                let retry_index = attempt.number().saturating_sub(1);
                let server_delay = match error {
                    LlmError::HttpStatus(error) => error.retry_after(),
                    _ => None,
                };
                let Some(wait) = calculate_retry_wait(
                    self.retry_wait,
                    retry_index,
                    rand::random::<u128>(),
                    server_delay,
                    self.total_waited,
                    remaining(&self.lifecycle),
                    self.retries.minimum_attempt_budget(),
                ) else {
                    self.emit(LifecycleEventKind::RetryExhausted {
                        attempt: attempt.clone(),
                        reason: RetryStopReason::DeadlineInsufficient,
                    });
                    return Ok(None);
                };
                self.emit(LifecycleEventKind::RetryScheduled {
                    previous_attempt: attempt.clone(),
                    next_attempt_number,
                    reason,
                    delay: wait.effective_delay,
                    server_delay_applied: wait.server_delay_valid
                        && server_delay.is_some_and(|delay| delay >= wait.client_delay),
                    server_delay_capped: wait.server_delay_capped,
                });
                wait_for_retry(&self.lifecycle, wait.effective_delay).await?;
                self.total_waited = self.total_waited.saturating_add(wait.effective_delay);
                Ok(Some(next_attempt_number))
            }
        }
    }

    fn emit(&self, kind: LifecycleEventKind) {
        if let Some(observation) = &self.observation {
            observation.emit(kind);
        }
    }

    fn emit_retry_stopped(&self, attempt: &AttemptIdentity, reason: RetryStopReason) {
        self.emit(LifecycleEventKind::RetryDecided {
            attempt: attempt.clone(),
            reason: None,
            stop_reason: Some(reason),
        });
        self.emit(LifecycleEventKind::RetryExhausted {
            attempt: attempt.clone(),
            reason,
        });
    }

    fn retry_stop_reason(&self, error: &LlmError, attempt_number: u32) -> RetryStopReason {
        if self.execution_state.delivery_state()
            == super::reliability::DeliveryState::DomainEventDelivered
        {
            RetryStopReason::DeliveryBoundaryClosed
        } else if attempt_number >= self.retries.max_attempts() {
            RetryStopReason::AttemptsExhausted
        } else if remaining(&self.lifecycle)
            .is_some_and(|value| value < self.retries.minimum_attempt_budget())
        {
            RetryStopReason::DeadlineInsufficient
        } else if !self.idempotency.replay_safe() && requires_replay_safety(error) {
            RetryStopReason::ReplayUnsafe
        } else {
            RetryStopReason::NonRetryable
        }
    }
}

fn requires_replay_safety(error: &LlmError) -> bool {
    matches!(
        error,
        LlmError::Transport(error) if error.stage() == crate::error::ErrorStage::Body
    ) || matches!(
        error,
        LlmError::HttpStatus(error) if error.status() == 408 || matches!(error.status(), 502..=504)
    ) || matches!(error, LlmError::TruncatedStream(_))
}

struct ActiveAttempt {
    runner: RequestRunner,
    attempt: AttemptIdentity,
    active: EventStream,
    event_seen: bool,
    terminal: bool,
}

async fn poll_active_attempt(
    mut state: ActiveAttempt,
) -> Option<(
    Result<crate::domain::AssistantEvent, LlmError>,
    ActiveAttempt,
)> {
    if state.terminal {
        return None;
    }

    loop {
        let timeout_stage = if state.event_seen {
            TimeoutStage::IdleStream
        } else {
            TimeoutStage::FirstEvent
        };
        let timeout = if state.event_seen {
            state.runner.timeouts.idle_stream_timeout()
        } else {
            state.runner.timeouts.first_event_timeout()
        };
        let delivered = state.runner.execution_state.delivery_state()
            == super::reliability::DeliveryState::DomainEventDelivered;
        let result = await_stream_with_stage(
            &state.runner.lifecycle,
            timeout_stage,
            timeout,
            state.attempt.number(),
            delivered,
            state.active.next(),
        )
        .await;

        let error = match result {
            Ok(Some(Ok(event))) => {
                state.event_seen = true;
                return Some((Ok(event), state));
            }
            Ok(Some(Err(error))) | Err(error) => error,
            Ok(None) => LlmError::from(TruncatedStreamError),
        };
        let failed_attempt_stream = std::mem::replace(&mut state.active, Box::pin(stream::empty()));
        drop(failed_attempt_stream);

        let Some(mut next_attempt_number) = (match state
            .runner
            .retry_after_failure(&error, &state.attempt)
            .await
        {
            Ok(next) => next,
            Err(wait_error) => {
                state.terminal = true;
                return Some((Err(wait_error), state));
            }
        }) else {
            state.terminal = true;
            return Some((Err(error), state));
        };

        loop {
            let (attempt, result) = state.runner.open_attempt(next_attempt_number).await;
            match result {
                Ok(active) => {
                    state.attempt = attempt;
                    state.active = active;
                    state.event_seen = false;
                    break;
                }
                Err(error) => {
                    let Some(next) = (match state.runner.retry_after_failure(&error, &attempt).await
                    {
                        Ok(next) => next,
                        Err(wait_error) => {
                            state.terminal = true;
                            return Some((Err(wait_error), state));
                        }
                    }) else {
                        state.terminal = true;
                        return Some((Err(error), state));
                    };
                    next_attempt_number = next;
                }
            }
        }
    }
}
