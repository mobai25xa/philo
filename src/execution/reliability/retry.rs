//! Pure lifecycle-aware retry decision policy.

use std::time::Duration;

use crate::error::{
    CredentialFailure, ErrorStage, LlmError, RetryReason, TimeoutStage, ValidationError,
    ValidationReason,
};

use super::DeliveryState;

const MAX_ATTEMPTS_HARD_LIMIT: u32 = 8;

/// Immutable retry bounds for one SDK client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    max_attempts: u32,
    minimum_attempt_budget: Duration,
}

impl RetryPolicy {
    /// Creates the conservative default: one attempt and no automatic retry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_attempts: 1,
            minimum_attempt_budget: Duration::from_millis(100),
        }
    }

    /// Creates the standard bounded policy with up to three attempts.
    #[must_use]
    pub const fn standard() -> Self {
        Self {
            max_attempts: 3,
            minimum_attempt_budget: Duration::from_millis(100),
        }
    }

    /// Sets the total attempt limit, including the first attempt.
    ///
    /// # Errors
    ///
    /// Returns a validation error for zero or values above the SDK hard limit.
    pub fn with_max_attempts(mut self, value: u32) -> Result<Self, ValidationError> {
        if value == 0 {
            return Err(ValidationError::new(
                "max_attempts",
                ValidationReason::Zero,
                "max attempts must be positive",
            ));
        }
        if value > MAX_ATTEMPTS_HARD_LIMIT {
            return Err(ValidationError::new(
                "max_attempts",
                ValidationReason::OutOfRange,
                "max attempts exceeds the SDK hard limit",
            ));
        }
        self.max_attempts = value;
        Ok(self)
    }

    /// Sets the minimum remaining overall budget required for a new attempt.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the duration is zero.
    pub fn with_minimum_attempt_budget(mut self, value: Duration) -> Result<Self, ValidationError> {
        if value.is_zero() {
            return Err(ValidationError::new(
                "minimum_attempt_budget",
                ValidationReason::Zero,
                "minimum attempt budget must be positive",
            ));
        }
        self.minimum_attempt_budget = value;
        Ok(self)
    }

    /// Returns the total bounded attempt count.
    #[must_use]
    pub const fn max_attempts(self) -> u32 {
        self.max_attempts
    }

    /// Returns the minimum remaining overall budget for a new attempt.
    #[must_use]
    pub const fn minimum_attempt_budget(self) -> Duration {
        self.minimum_attempt_budget
    }

    pub(crate) fn tighten_with(self, request: Self) -> Self {
        Self {
            max_attempts: self.max_attempts.min(request.max_attempts),
            minimum_attempt_budget: self
                .minimum_attempt_budget
                .max(request.minimum_attempt_budget),
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetryDecision {
    Fail,
    Retry(RetryReason),
}

pub(crate) fn decide_retry(
    error: &LlmError,
    delivery: DeliveryState,
    attempt_number: u32,
    policy: RetryPolicy,
    remaining: Option<Duration>,
    replay_safe: bool,
) -> RetryDecision {
    if delivery == DeliveryState::DomainEventDelivered
        || attempt_number >= policy.max_attempts
        || remaining.is_some_and(|value| value < policy.minimum_attempt_budget)
    {
        return RetryDecision::Fail;
    }

    let reason = match error {
        LlmError::Transport(error) if error.stage() == ErrorStage::Connect => {
            RetryReason::ConnectFailure
        }
        LlmError::Transport(error) if error.stage() == ErrorStage::Body && replay_safe => {
            RetryReason::EarlyBodyFailure
        }
        LlmError::Timeout(error)
            if matches!(
                error.timeout_stage(),
                TimeoutStage::Connect | TimeoutStage::ResponseHeader | TimeoutStage::FirstEvent
            ) && !error.overall_limited() =>
        {
            RetryReason::StageTimeout
        }
        LlmError::Credential(error) if error.kind() == CredentialFailure::Timeout => {
            RetryReason::CredentialTimeout
        }
        LlmError::HttpStatus(error) if error.status() == 408 && replay_safe => {
            RetryReason::RequestTimeoutStatus
        }
        LlmError::HttpStatus(error) if error.status() == 429 => RetryReason::RateLimited,
        LlmError::HttpStatus(error) if matches!(error.status(), 502..=504) && replay_safe => {
            RetryReason::TransientServerError
        }
        LlmError::TruncatedStream(_) if replay_safe => RetryReason::EarlyTruncation,
        LlmError::Cancelled
        | LlmError::Configuration(_)
        | LlmError::ProviderConfig(_)
        | LlmError::ProviderRegistry(_)
        | LlmError::Validation(_)
        | LlmError::Capability(_)
        | LlmError::Schema(_)
        | LlmError::ToolValidation(_)
        | LlmError::History(_)
        | LlmError::StructuredOutput(_)
        | LlmError::Cost(_)
        | LlmError::Authentication(_)
        | LlmError::Credential(_)
        | LlmError::HeaderPolicy(_)
        | LlmError::Transport(_)
        | LlmError::HttpStatus(_)
        | LlmError::Protocol(_)
        | LlmError::UnsupportedResponseSemantics(_)
        | LlmError::UnknownFinishReason(_)
        | LlmError::TruncatedStream(_)
        | LlmError::Timeout(_) => return RetryDecision::Fail,
    };
    RetryDecision::Retry(reason)
}

#[cfg(test)]
mod tests {
    use crate::error::{BodySummary, HttpStatusError, RetriableHint};

    use super::{DeliveryState, RetryDecision, RetryPolicy, decide_retry};

    #[test]
    fn delivery_guard_cannot_be_overridden_by_status_or_policy() {
        let error = HttpStatusError::new(
            503,
            BodySummary::from_bytes(b"temporary", 32),
            None,
            RetriableHint::Maybe,
        )
        .into();
        assert_eq!(
            decide_retry(
                &error,
                DeliveryState::DomainEventDelivered,
                1,
                RetryPolicy::standard(),
                None,
                true,
            ),
            RetryDecision::Fail
        );
    }
}
