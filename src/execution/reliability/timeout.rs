//! Public stage-timeout policy compiled into each logical call.

use std::time::Duration;

use crate::error::{ValidationError, ValidationReason};

const MAX_STAGE_TIMEOUT: Duration = Duration::from_hours(1);

/// Immutable per-client stage timeout policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeoutPolicy {
    credential: Duration,
    response_header: Duration,
    first_event: Duration,
    idle_stream: Duration,
}

impl TimeoutPolicy {
    /// Creates secure SDK defaults for bounded request stages.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            credential: Duration::from_secs(10),
            response_header: Duration::from_secs(30),
            first_event: Duration::from_secs(30),
            idle_stream: Duration::from_mins(1),
        }
    }

    /// Sets the credential/header-auth stage limit.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the duration is zero or above the hard limit.
    pub fn with_credential_timeout(mut self, value: Duration) -> Result<Self, ValidationError> {
        validate("credential_timeout", value)?;
        self.credential = value;
        Ok(self)
    }

    /// Sets the response-status/header stage limit.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the duration is zero or above the hard limit.
    pub fn with_response_header_timeout(
        mut self,
        value: Duration,
    ) -> Result<Self, ValidationError> {
        validate("response_header_timeout", value)?;
        self.response_header = value;
        Ok(self)
    }

    /// Sets the first parsed domain-event limit.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the duration is zero or above the hard limit.
    pub fn with_first_event_timeout(mut self, value: Duration) -> Result<Self, ValidationError> {
        validate("first_event_timeout", value)?;
        self.first_event = value;
        Ok(self)
    }

    /// Sets the parsed stream-progress idle limit.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the duration is zero or above the hard limit.
    pub fn with_idle_stream_timeout(mut self, value: Duration) -> Result<Self, ValidationError> {
        validate("idle_stream_timeout", value)?;
        self.idle_stream = value;
        Ok(self)
    }

    /// Returns the credential stage limit.
    #[must_use]
    pub const fn credential_timeout(self) -> Duration {
        self.credential
    }

    /// Returns the response-header stage limit.
    #[must_use]
    pub const fn response_header_timeout(self) -> Duration {
        self.response_header
    }

    /// Returns the first-event stage limit.
    #[must_use]
    pub const fn first_event_timeout(self) -> Duration {
        self.first_event
    }

    /// Returns the idle-stream stage limit.
    #[must_use]
    pub const fn idle_stream_timeout(self) -> Duration {
        self.idle_stream
    }

    pub(crate) fn tighten_with(self, request: Self) -> Self {
        Self {
            credential: self.credential.min(request.credential),
            response_header: self.response_header.min(request.response_header),
            first_event: self.first_event.min(request.first_event),
            idle_stream: self.idle_stream.min(request.idle_stream),
        }
    }
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        Self::new()
    }
}

fn validate(field: &'static str, value: Duration) -> Result<(), ValidationError> {
    if value.is_zero() {
        return Err(ValidationError::new(
            field,
            ValidationReason::Zero,
            "stage timeout must be positive",
        ));
    }
    if value > MAX_STAGE_TIMEOUT {
        return Err(ValidationError::new(
            field,
            ValidationReason::OutOfRange,
            "stage timeout exceeds the SDK hard limit",
        ));
    }
    Ok(())
}
