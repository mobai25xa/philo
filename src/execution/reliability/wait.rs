//! Bounded retry delay calculation, Retry-After parsing, and cancellable waiting.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::{HeaderMap, HeaderName, header};

use crate::error::{LlmError, ValidationError, ValidationReason};
use crate::transport::{RequestLifecycle, await_with_lifecycle};

const MAX_CONFIGURED_WAIT: Duration = Duration::from_hours(1);

/// Immutable bounds for exponential retry backoff and server-directed waits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryWaitPolicy {
    base_delay: Duration,
    max_delay: Duration,
    server_delay_cap: Duration,
    max_total_wait: Duration,
}

impl RetryWaitPolicy {
    /// Creates the default bounded full-jitter policy.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(5),
            server_delay_cap: Duration::from_mins(1),
            max_total_wait: Duration::from_mins(1),
        }
    }

    /// Sets the exponential backoff base delay.
    ///
    /// # Errors
    ///
    /// Returns a validation error for zero or values above the SDK hard limit.
    pub fn with_base_delay(mut self, value: Duration) -> Result<Self, ValidationError> {
        validate_wait("base_delay", value)?;
        self.base_delay = value;
        Ok(self)
    }

    /// Sets the maximum client-generated backoff delay.
    ///
    /// # Errors
    ///
    /// Returns a validation error for zero or values above the SDK hard limit.
    pub fn with_max_delay(mut self, value: Duration) -> Result<Self, ValidationError> {
        validate_wait("max_delay", value)?;
        self.max_delay = value;
        Ok(self)
    }

    /// Sets the maximum accepted server-directed delay.
    ///
    /// # Errors
    ///
    /// Returns a validation error for zero or values above the SDK hard limit.
    pub fn with_server_delay_cap(mut self, value: Duration) -> Result<Self, ValidationError> {
        validate_wait("server_delay_cap", value)?;
        self.server_delay_cap = value;
        Ok(self)
    }

    /// Sets the cumulative retry-wait ceiling for one logical request.
    ///
    /// # Errors
    ///
    /// Returns a validation error for zero or values above the SDK hard limit.
    pub fn with_max_total_wait(mut self, value: Duration) -> Result<Self, ValidationError> {
        validate_wait("max_total_wait", value)?;
        self.max_total_wait = value;
        Ok(self)
    }

    /// Returns the exponential backoff base delay.
    #[must_use]
    pub const fn base_delay(self) -> Duration {
        self.base_delay
    }

    /// Returns the client-generated per-retry delay ceiling.
    #[must_use]
    pub const fn max_delay(self) -> Duration {
        self.max_delay
    }

    /// Returns the accepted server-directed delay ceiling.
    #[must_use]
    pub const fn server_delay_cap(self) -> Duration {
        self.server_delay_cap
    }

    /// Returns the cumulative retry-wait ceiling.
    #[must_use]
    pub const fn max_total_wait(self) -> Duration {
        self.max_total_wait
    }

    pub(crate) fn tighten_with(self, request: Self) -> Self {
        Self {
            base_delay: self.base_delay.max(request.base_delay),
            max_delay: self.max_delay.min(request.max_delay),
            server_delay_cap: self.server_delay_cap.min(request.server_delay_cap),
            max_total_wait: self.max_total_wait.min(request.max_total_wait),
        }
    }
}

impl Default for RetryWaitPolicy {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_wait(field: &'static str, value: Duration) -> Result<(), ValidationError> {
    if value.is_zero() {
        return Err(ValidationError::new(
            field,
            ValidationReason::Zero,
            "retry wait duration must be positive",
        ));
    }
    if value > MAX_CONFIGURED_WAIT {
        return Err(ValidationError::new(
            field,
            ValidationReason::OutOfRange,
            "retry wait duration exceeds the SDK hard limit",
        ));
    }
    Ok(())
}

/// Safe result of inspecting a server retry-delay header.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RetryAfterObservation {
    pub(crate) present: bool,
    pub(crate) valid: bool,
    pub(crate) delay: Option<Duration>,
}

/// Typed declaration for a standard or provider-specific retry-delay header.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum RetryAfterHeader {
    Standard,
    ProviderDeltaSeconds(HeaderName),
    ProviderUnixSeconds(HeaderName),
}

impl RetryAfterHeader {
    fn name(&self) -> &HeaderName {
        match self {
            Self::Standard => &header::RETRY_AFTER,
            Self::ProviderDeltaSeconds(name) | Self::ProviderUnixSeconds(name) => name,
        }
    }
}

/// Fully bounded retry wait selected before sleeping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetryWaitPlan {
    pub(crate) client_delay: Duration,
    pub(crate) server_delay_valid: bool,
    pub(crate) server_delay_capped: bool,
    pub(crate) effective_delay: Duration,
}

/// Calculates full jitter for a zero-based retry index using an injected random sample.
pub(crate) fn calculate_retry_wait(
    policy: RetryWaitPolicy,
    retry_index: u32,
    random_sample: u128,
    server_delay: Option<Duration>,
    total_waited: Duration,
    remaining_deadline: Option<Duration>,
    minimum_attempt_budget: Duration,
) -> Option<RetryWaitPlan> {
    let exponent = retry_index.min(63);
    let multiplier = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
    let multiplier = u32::try_from(multiplier).unwrap_or(u32::MAX);
    let exponential = policy.base_delay.saturating_mul(multiplier);
    let exponential_cap = exponential.min(policy.max_delay);
    let cap_nanos = exponential_cap.as_nanos();
    let client_nanos = if cap_nanos == 0 {
        0
    } else {
        random_sample % cap_nanos.saturating_add(1)
    };
    let client_delay = duration_from_nanos(client_nanos);
    let server_delay_capped = server_delay.is_some_and(|delay| delay > policy.server_delay_cap);
    let server_delay = server_delay.map(|delay| delay.min(policy.server_delay_cap));
    let effective_delay = client_delay
        .max(server_delay.unwrap_or_default())
        .min(policy.server_delay_cap);
    if total_waited.saturating_add(effective_delay) > policy.max_total_wait {
        return None;
    }
    if remaining_deadline
        .is_some_and(|remaining| effective_delay.saturating_add(minimum_attempt_budget) > remaining)
    {
        return None;
    }
    Some(RetryWaitPlan {
        client_delay,
        server_delay_valid: server_delay.is_some(),
        server_delay_capped,
        effective_delay,
    })
}

fn duration_from_nanos(value: u128) -> Duration {
    let seconds =
        u64::try_from((value / 1_000_000_000).min(u128::from(u64::MAX))).unwrap_or(u64::MAX);
    let nanos = u32::try_from(value % 1_000_000_000).unwrap_or(999_999_999);
    Duration::new(seconds, nanos)
}

/// Parses the first declared retry-delay header without retaining its raw value.
pub(crate) fn parse_retry_after(
    headers: &HeaderMap,
    declarations: &[RetryAfterHeader],
    now: SystemTime,
) -> RetryAfterObservation {
    let mut present = false;
    let mut selected = None;
    for declaration in declarations {
        for raw in &headers.get_all(declaration.name()) {
            present = true;
            let Ok(value) = raw.to_str() else {
                return RetryAfterObservation {
                    present: true,
                    valid: false,
                    delay: None,
                };
            };
            let delay = match declaration {
                RetryAfterHeader::Standard => parse_delta_seconds(value)
                    .or_else(|| parse_http_date(value).map(|date| duration_until(now, date))),
                RetryAfterHeader::ProviderDeltaSeconds(_) => parse_delta_seconds(value),
                RetryAfterHeader::ProviderUnixSeconds(_) => parse_unix_seconds(value, now),
            };
            let Some(delay) = delay else {
                return RetryAfterObservation {
                    present: true,
                    valid: false,
                    delay: None,
                };
            };
            if selected.is_some_and(|selected| selected != delay) {
                return RetryAfterObservation {
                    present: true,
                    valid: false,
                    delay: None,
                };
            }
            selected = Some(delay);
        }
    }
    RetryAfterObservation {
        present,
        valid: selected.is_some(),
        delay: selected,
    }
}

fn parse_delta_seconds(value: &str) -> Option<Duration> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok().map(Duration::from_secs)
}

fn parse_unix_seconds(value: &str, now: SystemTime) -> Option<Duration> {
    let timestamp = value.parse::<u64>().ok()?;
    let target = UNIX_EPOCH.checked_add(Duration::from_secs(timestamp))?;
    Some(duration_until(now, target))
}

fn duration_until(now: SystemTime, target: SystemTime) -> Duration {
    target.duration_since(now).unwrap_or_default()
}

// RFC 9110's preferred IMF-fixdate form. Obsolete forms are rejected safely.
fn parse_http_date(value: &str) -> Option<SystemTime> {
    let mut parts = value.split_ascii_whitespace();
    let weekday = parts.next()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parse_month(parts.next()?)?;
    let year = parts.next()?.parse::<i32>().ok()?;
    let time = parts.next()?;
    let zone = parts.next()?;
    if parts.next().is_some() || !weekday.ends_with(',') || zone != "GMT" {
        return None;
    }
    let mut clock = time.split(':');
    let hour = clock.next()?.parse::<u32>().ok()?;
    let minute = clock.next()?.parse::<u32>().ok()?;
    let second = clock.next()?.parse::<u32>().ok()?;
    if clock.next().is_some()
        || day == 0
        || day > days_in_month(year, month)?
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    if seconds < 0 {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::from_secs(seconds.cast_unsigned()))
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    Some(match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) => 29,
        2 => 28,
        _ => return None,
    })
}

fn parse_month(value: &str) -> Option<u32> {
    Some(match value {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

// Howard Hinnant's civil-date conversion, shifted to the Unix epoch.
fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    let adjusted_year = year - i32::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = i32::try_from(month).ok()? + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i32::try_from(day).ok()? - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(i64::from(era * 146_097 + day_of_era - 719_468))
}

/// Sleeps without escaping the logical request's cancellation or overall deadline.
pub(crate) async fn wait_for_retry(
    lifecycle: &RequestLifecycle,
    delay: Duration,
) -> Result<(), LlmError> {
    await_with_lifecycle(lifecycle, tokio::time::sleep(delay)).await
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use http::{HeaderMap, HeaderName, HeaderValue, header};

    use super::{RetryAfterHeader, RetryWaitPolicy, calculate_retry_wait, parse_retry_after};

    #[test]
    fn full_jitter_is_bounded_and_exponential_math_saturates() {
        let policy = RetryWaitPolicy::new();
        for retry_index in [0, 1, 2, 31, 63, u32::MAX] {
            let plan = calculate_retry_wait(
                policy,
                retry_index,
                u128::MAX,
                None,
                Duration::ZERO,
                None,
                Duration::from_millis(100),
            )
            .unwrap();
            assert!(plan.client_delay <= policy.max_delay());
        }
    }

    #[test]
    fn retry_after_accepts_delta_date_and_typed_reset_headers() {
        let now = UNIX_EPOCH + Duration::from_secs(784_111_717);
        let mut headers = HeaderMap::new();
        headers.insert(header::RETRY_AFTER, HeaderValue::from_static("120"));
        assert_eq!(
            parse_retry_after(&headers, &[RetryAfterHeader::Standard], now).delay,
            Some(Duration::from_mins(2))
        );

        headers.insert(
            header::RETRY_AFTER,
            HeaderValue::from_static("Sun, 06 Nov 1994 08:49:37 GMT"),
        );
        assert_eq!(
            parse_retry_after(&headers, &[RetryAfterHeader::Standard], now).delay,
            Some(Duration::from_mins(1))
        );

        let reset = HeaderName::from_static("x-provider-reset");
        headers.insert(&reset, HeaderValue::from_static("784111837"));
        assert_eq!(
            parse_retry_after(
                &headers,
                &[RetryAfterHeader::ProviderUnixSeconds(reset)],
                now,
            )
            .delay,
            Some(Duration::from_mins(2))
        );
    }

    #[test]
    fn invalid_and_past_retry_after_values_are_safe() {
        let now = UNIX_EPOCH + Duration::from_secs(784_111_717);
        let mut headers = HeaderMap::new();
        headers.insert(header::RETRY_AFTER, HeaderValue::from_static("-1"));
        let invalid = parse_retry_after(&headers, &[RetryAfterHeader::Standard], now);
        assert!(invalid.present);
        assert!(!invalid.valid);

        headers.insert(
            header::RETRY_AFTER,
            HeaderValue::from_static("Sun, 06 Nov 1994 08:47:37 GMT"),
        );
        assert_eq!(
            parse_retry_after(&headers, &[RetryAfterHeader::Standard], now).delay,
            Some(Duration::ZERO)
        );

        headers.append(header::RETRY_AFTER, HeaderValue::from_static("120"));
        let conflicting = parse_retry_after(&headers, &[RetryAfterHeader::Standard], now);
        assert!(conflicting.present);
        assert!(!conflicting.valid);
        assert_eq!(conflicting.delay, None);
    }

    #[test]
    fn injected_samples_produce_distinct_bounded_delays() {
        let policy = RetryWaitPolicy::new();
        let first = calculate_retry_wait(
            policy,
            0,
            1,
            None,
            Duration::ZERO,
            None,
            Duration::from_millis(100),
        )
        .unwrap();
        let second = calculate_retry_wait(
            policy,
            0,
            2,
            None,
            Duration::ZERO,
            None,
            Duration::from_millis(100),
        )
        .unwrap();
        assert_ne!(first.client_delay, second.client_delay);
    }

    #[test]
    fn deadline_and_total_wait_reject_doomed_retry() {
        let policy = RetryWaitPolicy::new()
            .with_max_total_wait(Duration::from_secs(1))
            .unwrap();
        assert!(
            calculate_retry_wait(
                policy,
                0,
                0,
                Some(Duration::from_secs(2)),
                Duration::ZERO,
                None,
                Duration::from_millis(100),
            )
            .is_none()
        );
        assert!(
            calculate_retry_wait(
                RetryWaitPolicy::new(),
                0,
                0,
                Some(Duration::from_secs(1)),
                Duration::ZERO,
                Some(Duration::from_millis(500)),
                Duration::from_millis(100),
            )
            .is_none()
        );
    }
}
