//! Provider request idempotency identity and private wire policy.

use std::fmt;

use http::{HeaderName, HeaderValue};
use uuid::Uuid;

use crate::error::{LlmError, ValidationError, ValidationReason};

use super::HeaderOperation;

const MAX_KEY_BYTES: usize = 128;
static STANDARD_HEADER: HeaderName = HeaderName::from_static("idempotency-key");

/// Opaque, validated provider-request idempotency key.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Validates a caller-supplied key.
    ///
    /// # Errors
    ///
    /// Returns a validation error for empty, oversized, or non-ASCII-safe values.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty() {
            return Err(validation(
                ValidationReason::Empty,
                "idempotency key is empty",
            ));
        }
        if value.len() > MAX_KEY_BYTES {
            return Err(validation(
                ValidationReason::OutOfRange,
                "idempotency key exceeds the SDK byte limit",
            ));
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
        {
            return Err(validation(
                ValidationReason::InvalidIdentifier,
                "idempotency key contains an unsupported character",
            ));
        }
        Ok(Self(value))
    }

    /// Generates a cryptographically random key without using request content.
    #[must_use]
    pub fn generate() -> Self {
        Self(format!("philo_{}", Uuid::new_v4().simple()))
    }

    pub(crate) fn header_value(&self) -> Result<HeaderValue, LlmError> {
        HeaderValue::from_str(&self.0).map_err(|_| {
            validation(
                ValidationReason::InvalidHeader,
                "idempotency key cannot be encoded as an HTTP header",
            )
            .into()
        })
    }
}

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("IdempotencyKey([REDACTED])")
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Provider capability for request-level idempotency keys.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum IdempotencyCapability {
    /// The reviewed provider profile declares supported wire behavior.
    Supported,
    /// The provider explicitly does not support request idempotency keys.
    Unsupported,
    /// Support has not been established and is treated fail-closed.
    Unknown,
}

/// Source of a logical request's opaque key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum IdempotencyKeySource {
    /// Supplied explicitly by the SDK caller.
    Caller,
    /// Generated randomly by the SDK for this logical request.
    SdkGenerated,
}

/// Immutable provider idempotency capability and private encoding policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdempotencyPolicy {
    capability: IdempotencyCapability,
    wire: Option<IdempotencyWire>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum IdempotencyWire {
    StandardHeader,
}

impl IdempotencyPolicy {
    /// Creates a fail-closed policy whose support is unknown.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            capability: IdempotencyCapability::Unknown,
            wire: None,
        }
    }

    /// Creates a policy for a provider known not to support idempotency keys.
    #[must_use]
    pub const fn unsupported() -> Self {
        Self {
            capability: IdempotencyCapability::Unsupported,
            wire: None,
        }
    }

    /// Creates the reviewed standard-header encoding policy.
    #[must_use]
    pub const fn standard_header() -> Self {
        Self {
            capability: IdempotencyCapability::Supported,
            wire: Some(IdempotencyWire::StandardHeader),
        }
    }

    /// Returns the declared provider capability.
    #[must_use]
    pub const fn capability(&self) -> IdempotencyCapability {
        self.capability
    }

    pub(crate) fn header_name(&self) -> Option<&HeaderName> {
        match self.wire {
            Some(IdempotencyWire::StandardHeader) => Some(&STANDARD_HEADER),
            None => None,
        }
    }
}

impl Default for IdempotencyPolicy {
    fn default() -> Self {
        Self::unknown()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedIdempotency {
    key: Option<IdempotencyKey>,
    source: Option<IdempotencyKeySource>,
    capability: IdempotencyCapability,
    header: Option<HeaderName>,
}

impl ResolvedIdempotency {
    pub(crate) fn resolve(
        policy: &IdempotencyPolicy,
        caller_key: Option<&IdempotencyKey>,
        generate: bool,
        automatic_for_retry: bool,
    ) -> Result<Self, LlmError> {
        let requested = caller_key.is_some() || generate;
        if requested && policy.capability != IdempotencyCapability::Supported {
            let reason = match policy.capability {
                IdempotencyCapability::Unsupported => ValidationReason::CapabilityUnsupported,
                IdempotencyCapability::Unknown => ValidationReason::CapabilityUnknown,
                IdempotencyCapability::Supported => unreachable!(),
            };
            return Err(validation(
                reason,
                "selected provider does not declare idempotency-key support",
            )
            .into());
        }
        let (key, source) = if let Some(key) = caller_key {
            (Some(key.clone()), Some(IdempotencyKeySource::Caller))
        } else if policy.capability == IdempotencyCapability::Supported
            && (generate || automatic_for_retry)
        {
            (
                Some(IdempotencyKey::generate()),
                Some(IdempotencyKeySource::SdkGenerated),
            )
        } else {
            (None, None)
        };
        Ok(Self {
            key,
            source,
            capability: policy.capability,
            header: policy.header_name().cloned(),
        })
    }

    pub(crate) fn operation(&self) -> Result<Option<HeaderOperation>, LlmError> {
        match (&self.key, &self.header) {
            (Some(key), Some(name)) => Ok(Some(HeaderOperation::set_sensitive(
                name.clone(),
                key.header_value()?,
            ))),
            (None, _) | (_, None) => Ok(None),
        }
    }

    pub(crate) const fn source(&self) -> Option<IdempotencyKeySource> {
        self.source
    }

    pub(crate) const fn capability(&self) -> IdempotencyCapability {
        self.capability
    }

    pub(crate) const fn is_present(&self) -> bool {
        self.key.is_some()
    }

    pub(crate) fn replay_safe(&self) -> bool {
        self.key.is_some() && self.capability == IdempotencyCapability::Supported
    }
}

fn validation(reason: ValidationReason, summary: &'static str) -> ValidationError {
    ValidationError::new("idempotency_key", reason, summary)
}
