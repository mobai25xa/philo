//! Typed, value-free provider rate-limit response metadata.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::{HeaderMap, HeaderName, StatusCode};

use crate::error::{ValidationError, ValidationReason};

const MAX_DECLARATIONS: usize = 16;
const MAX_RESET_DELAY: Duration = Duration::from_hours(24);

/// State of one optional rate-limit response field.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum RateLimitValue<T> {
    /// The provider did not supply a declared value.
    #[default]
    Unknown,
    /// A declared value was parsed and bounded successfully.
    Valid(T),
    /// A declared value was present but malformed, conflicting, negative, or out of bounds.
    Invalid,
}

/// Unit attached to a typed remaining-quota declaration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RateLimitUnit {
    /// Provider requests.
    Requests,
    /// Model input or output tokens.
    Tokens,
    /// A provider-defined unit that must not drive generic quota arithmetic.
    ProviderDefined,
}

/// Parsed remaining quota with an explicit unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateLimitQuota {
    remaining: u64,
    unit: RateLimitUnit,
}

impl RateLimitQuota {
    /// Returns the non-negative remaining amount.
    #[must_use]
    pub const fn remaining(self) -> u64 {
        self.remaining
    }

    /// Returns the declared unit.
    #[must_use]
    pub const fn unit(self) -> RateLimitUnit {
        self.unit
    }
}

/// Safely parsed provider reset hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RateLimitReset {
    /// Duration from response observation time.
    After(Duration),
    /// Absolute provider reset time.
    At(SystemTime),
}

/// Where a rate-limit observation came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RateLimitSourceKind {
    /// Only standard HTTP status or `Retry-After` semantics were involved.
    Standard,
    /// Only typed provider-profile declarations were involved.
    ProviderProfile,
    /// Standard HTTP and typed provider declarations both contributed.
    StandardAndProviderProfile,
}

/// Typed, value-free metadata parsed independently for one HTTP attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateLimitObservation {
    status_is_rate_limited: bool,
    retry_after: RateLimitValue<Duration>,
    remaining_requests: RateLimitValue<u64>,
    remaining_units: RateLimitValue<RateLimitQuota>,
    reset: RateLimitValue<RateLimitReset>,
    source: RateLimitSourceKind,
}

impl RateLimitObservation {
    /// Returns whether this attempt received HTTP 429.
    #[must_use]
    pub const fn status_is_rate_limited(self) -> bool {
        self.status_is_rate_limited
    }

    /// Returns the standard or typed provider retry delay state.
    #[must_use]
    pub const fn retry_after(self) -> RateLimitValue<Duration> {
        self.retry_after
    }

    /// Returns a typed remaining-request count.
    #[must_use]
    pub const fn remaining_requests(self) -> RateLimitValue<u64> {
        self.remaining_requests
    }

    /// Returns a typed remaining quota whose unit is explicit.
    #[must_use]
    pub const fn remaining_units(self) -> RateLimitValue<RateLimitQuota> {
        self.remaining_units
    }

    /// Returns the bounded reset hint.
    #[must_use]
    pub const fn reset(self) -> RateLimitValue<RateLimitReset> {
        self.reset
    }

    /// Returns the safe source classification.
    #[must_use]
    pub const fn source(self) -> RateLimitSourceKind {
        self.source
    }

    pub(crate) const fn retry_after_delay(self) -> Option<Duration> {
        match self.retry_after {
            RateLimitValue::Valid(delay) => Some(delay),
            RateLimitValue::Unknown | RateLimitValue::Invalid => None,
        }
    }
}

/// Encoding of one reviewed provider-specific rate-limit header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RateLimitHeaderKind {
    /// Non-negative remaining request count.
    RemainingRequests,
    /// Non-negative remaining amount with an explicit unit.
    RemainingUnits(RateLimitUnit),
    /// Reset duration encoded as integer seconds.
    ResetAfterSeconds,
    /// Reset time encoded as Unix seconds.
    ResetAtUnixSeconds,
    /// Retry delay encoded as integer seconds.
    RetryAfterSeconds,
    /// Retry time encoded as Unix seconds.
    RetryAtUnixSeconds,
}

/// One typed provider-profile response-header declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitHeaderSpec {
    name: HeaderName,
    kind: RateLimitHeaderKind,
}

impl RateLimitHeaderSpec {
    /// Creates one declaration from a normalized HTTP header name and typed encoding.
    #[must_use]
    pub const fn new(name: HeaderName, kind: RateLimitHeaderKind) -> Self {
        Self { name, kind }
    }

    /// Returns the normalized header name.
    #[must_use]
    pub const fn name(&self) -> &HeaderName {
        &self.name
    }

    /// Returns the declared encoding and unit.
    #[must_use]
    pub const fn kind(&self) -> RateLimitHeaderKind {
        self.kind
    }
}

/// Immutable typed declarations used to parse one provider's response metadata.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct RateLimitPolicy {
    headers: Vec<RateLimitHeaderSpec>,
}

impl RateLimitPolicy {
    /// Creates a policy that observes HTTP 429 and standard `Retry-After` only.
    #[must_use]
    pub const fn standard_only() -> Self {
        Self {
            headers: Vec::new(),
        }
    }

    /// Adds one reviewed provider-specific header declaration.
    ///
    /// # Errors
    ///
    /// Returns a validation error for duplicate names or excessive declarations.
    pub fn with_header(mut self, spec: RateLimitHeaderSpec) -> Result<Self, ValidationError> {
        if self.headers.len() >= MAX_DECLARATIONS {
            return Err(ValidationError::new(
                "rate_limit.headers",
                ValidationReason::OutOfRange,
                "rate-limit header declaration count exceeds the SDK limit",
            ));
        }
        if self
            .headers
            .iter()
            .any(|existing| existing.name == spec.name)
        {
            return Err(ValidationError::new(
                "rate_limit.headers",
                ValidationReason::Conflict,
                "rate-limit header names must be unique",
            ));
        }
        self.headers.push(spec);
        Ok(self)
    }

    /// Returns the number of provider-specific declarations.
    #[must_use]
    pub const fn provider_header_count(&self) -> usize {
        self.headers.len()
    }

    pub(crate) fn headers(&self) -> &[RateLimitHeaderSpec] {
        &self.headers
    }
}

impl fmt::Debug for RateLimitPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RateLimitPolicy")
            .field("provider_header_count", &self.headers.len())
            .finish()
    }
}

pub(crate) fn observe_rate_limit(
    status: StatusCode,
    headers: &HeaderMap,
    policy: &RateLimitPolicy,
    retry_after: RateLimitValue<Duration>,
    now: SystemTime,
) -> RateLimitObservation {
    let mut remaining_requests = RateLimitValue::Unknown;
    let mut remaining_units = RateLimitValue::Unknown;
    let mut reset = RateLimitValue::Unknown;
    let mut provider_present = false;

    for spec in policy.headers() {
        let values = headers.get_all(spec.name());
        if values.iter().next().is_none() {
            continue;
        }
        provider_present = true;
        match spec.kind() {
            RateLimitHeaderKind::RemainingRequests => {
                merge_value(&mut remaining_requests, parse_u64(values.iter()));
            }
            RateLimitHeaderKind::RemainingUnits(unit) => {
                let value = parse_u64(values.iter())
                    .map_valid(|remaining| RateLimitQuota { remaining, unit });
                merge_value(&mut remaining_units, value);
            }
            RateLimitHeaderKind::ResetAfterSeconds => {
                let value = parse_duration(values.iter()).map_valid(RateLimitReset::After);
                merge_value(&mut reset, value);
            }
            RateLimitHeaderKind::ResetAtUnixSeconds => {
                let value = parse_unix_time(values.iter(), now).map_valid(RateLimitReset::At);
                merge_value(&mut reset, value);
            }
            RateLimitHeaderKind::RetryAfterSeconds | RateLimitHeaderKind::RetryAtUnixSeconds => {}
        }
    }

    let standard_present =
        status == StatusCode::TOO_MANY_REQUESTS || !matches!(retry_after, RateLimitValue::Unknown);
    let source = match (standard_present, provider_present) {
        (true, true) => RateLimitSourceKind::StandardAndProviderProfile,
        (false, true) => RateLimitSourceKind::ProviderProfile,
        (true | false, false) => RateLimitSourceKind::Standard,
    };
    RateLimitObservation {
        status_is_rate_limited: status == StatusCode::TOO_MANY_REQUESTS,
        retry_after,
        remaining_requests,
        remaining_units,
        reset,
        source,
    }
}

trait MapValid<T> {
    fn map_valid<U>(self, map: impl FnOnce(T) -> U) -> RateLimitValue<U>;
}

impl<T> MapValid<T> for RateLimitValue<T> {
    fn map_valid<U>(self, map: impl FnOnce(T) -> U) -> RateLimitValue<U> {
        match self {
            Self::Unknown => RateLimitValue::Unknown,
            Self::Valid(value) => RateLimitValue::Valid(map(value)),
            Self::Invalid => RateLimitValue::Invalid,
        }
    }
}

fn merge_value<T: Copy + Eq>(target: &mut RateLimitValue<T>, incoming: RateLimitValue<T>) {
    match (*target, incoming) {
        (RateLimitValue::Unknown, value) => *target = value,
        (RateLimitValue::Valid(left), RateLimitValue::Valid(right)) if left == right => {}
        (_, RateLimitValue::Unknown) => {}
        _ => *target = RateLimitValue::Invalid,
    }
}

fn parse_u64<'a>(values: impl Iterator<Item = &'a http::HeaderValue>) -> RateLimitValue<u64> {
    parse_consistent(values, |value| {
        (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.parse::<u64>().ok())
            .flatten()
    })
}

fn parse_duration<'a>(
    values: impl Iterator<Item = &'a http::HeaderValue>,
) -> RateLimitValue<Duration> {
    parse_u64(values)
        .map_valid(Duration::from_secs)
        .map_valid(|delay| delay.min(MAX_RESET_DELAY))
}

fn parse_unix_time<'a>(
    values: impl Iterator<Item = &'a http::HeaderValue>,
    now: SystemTime,
) -> RateLimitValue<SystemTime> {
    parse_u64(values).map_valid(|seconds| {
        let target = UNIX_EPOCH
            .checked_add(Duration::from_secs(seconds))
            .unwrap_or(now);
        let delay = target.duration_since(now).unwrap_or_default();
        now.checked_add(delay.min(MAX_RESET_DELAY)).unwrap_or(now)
    })
}

fn parse_consistent<'a, T: Copy + Eq>(
    values: impl Iterator<Item = &'a http::HeaderValue>,
    parse: impl Fn(&str) -> Option<T>,
) -> RateLimitValue<T> {
    let mut selected = None;
    for raw in values {
        let Ok(raw) = raw.to_str() else {
            return RateLimitValue::Invalid;
        };
        let Some(value) = parse(raw) else {
            return RateLimitValue::Invalid;
        };
        if selected.is_some_and(|selected| selected != value) {
            return RateLimitValue::Invalid;
        }
        selected = Some(value);
    }
    selected.map_or(RateLimitValue::Unknown, RateLimitValue::Valid)
}
