//! Retry policy, exponential backoff, and server retry hints.

use std::{
    collections::BTreeSet,
    fmt,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use reqwest::header::HeaderMap;

use crate::Error;

/// An additional rule that can opt an error into retries.
pub type RetryPredicate = Arc<dyn Fn(&Error) -> bool + Send + Sync>;

/// Configuration shared by synchronous and asynchronous request retries.
///
/// `max_retries` counts retries after the initial attempt. The optional `timeout`
/// budget includes requests and waits; a retry whose delay reaches the budget is
/// skipped. Request timeouts are configured separately on the client.
#[derive(Clone)]
pub struct RetryPolicy {
    /// Maximum retries after the initial attempt. Zero disables retries.
    pub max_retries: u32,
    /// First exponential backoff delay. Zero disables backoff.
    pub backoff_initial: Duration,
    /// Maximum exponential backoff delay. Zero disables backoff.
    pub backoff_max: Duration,
    /// Fraction of each backoff randomly subtracted, between zero and one.
    pub backoff_jitter: f64,
    /// HTTP status codes that trigger a retry.
    pub http_statuses: BTreeSet<u16>,
    /// Honor `Retry-After` and `retry-after-ms` response headers.
    pub respect_retry_after: bool,
    /// Maximum accepted server delay. Larger hints fall back to backoff.
    /// `None` accepts any representable server delay.
    pub max_retry_after: Option<Duration>,
    /// Retry request connection and response-body delivery failures.
    pub api_connection_error: bool,
    /// Retry request timeouts, independently from connection failures.
    pub api_timeout_error: bool,
    /// Optional additional retry rule; built-in rules still apply.
    /// Cancellation is never retried, including by this predicate.
    pub predicate: Option<RetryPredicate>,
    /// Total retry budget including the first request, or `None` for no limit.
    pub timeout: Option<Duration>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            backoff_initial: Duration::from_millis(500),
            backoff_max: Duration::from_secs(5),
            backoff_jitter: 0.25,
            http_statuses: [408, 429].into_iter().chain(500..600).collect(),
            respect_retry_after: true,
            max_retry_after: Some(Duration::from_secs(60)),
            api_connection_error: true,
            api_timeout_error: true,
            predicate: None,
            timeout: Some(Duration::from_secs(30)),
        }
    }
}

impl fmt::Debug for RetryPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetryPolicy")
            .field("max_retries", &self.max_retries)
            .field("backoff_initial", &self.backoff_initial)
            .field("backoff_max", &self.backoff_max)
            .field("backoff_jitter", &self.backoff_jitter)
            .field("http_statuses", &self.http_statuses)
            .field("respect_retry_after", &self.respect_retry_after)
            .field("max_retry_after", &self.max_retry_after)
            .field("api_connection_error", &self.api_connection_error)
            .field("api_timeout_error", &self.api_timeout_error)
            .field("predicate", &self.predicate.as_ref().map(|_| "<predicate>"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl RetryPolicy {
    /// A policy with no retries, retaining the other defaults.
    pub fn no_retries() -> Self {
        Self {
            max_retries: 0,
            ..Self::default()
        }
    }

    /// Validate jitter, status codes, and timer-compatible delays and budget.
    pub fn validate(&self) -> Result<(), Error> {
        if !(0.0..=1.0).contains(&self.backoff_jitter) {
            return Err(Error::Configuration(
                "backoff_jitter must be between zero and one".into(),
            ));
        }
        if self.timeout.is_some_and(|timeout| timeout.is_zero()) {
            return Err(Error::Configuration(
                "retry timeout must be greater than zero".into(),
            ));
        }
        if self
            .http_statuses
            .iter()
            .any(|status| !(100..=999).contains(status))
        {
            return Err(Error::Configuration(
                "retry http_statuses must contain HTTP codes between 100 and 999".into(),
            ));
        }
        let now = Instant::now();
        for (name, duration) in [
            ("backoff_initial", Some(self.backoff_initial)),
            ("backoff_max", Some(self.backoff_max)),
            ("max_retry_after", self.max_retry_after),
            ("retry timeout", self.timeout),
        ] {
            if duration.is_some_and(|duration| now.checked_add(duration).is_none()) {
                return Err(Error::Configuration(format!(
                    "{name} is too large for the platform's timer"
                )));
            }
        }
        Ok(())
    }

    /// Whether the error matches a built-in or additional retry rule.
    ///
    /// This checks error classification only. The request loop separately applies
    /// the attempt limit and retry budget.
    pub fn should_retry(&self, error: &Error) -> bool {
        let builtin = match error {
            Error::Cancelled => return false,
            Error::Timeout { .. } => self.api_timeout_error,
            Error::Connection(_) => self.api_connection_error,
            Error::Api(error) => self.http_statuses.contains(&error.status.as_u16()),
            Error::ResponseValidation { response, .. } => {
                self.http_statuses.contains(&response.status.as_u16())
            }
            _ => false,
        };
        builtin
            || self
                .predicate
                .as_ref()
                .is_some_and(|predicate| predicate(error))
    }

    /// Delay before a zero-based retry attempt, honoring permitted server hints.
    ///
    /// Attempt zero uses `backoff_initial`, attempt one twice that value, and so
    /// on up to `backoff_max`, with random downward jitter. Very large attempt
    /// numbers and header values are handled without arithmetic overflow.
    pub fn delay(&self, attempt: u32, headers: Option<&HeaderMap>) -> Duration {
        if self.respect_retry_after {
            if let Some(delay) = headers.and_then(parse_retry_after) {
                if self.max_retry_after.is_none_or(|maximum| delay <= maximum) {
                    return delay;
                }
            }
        }
        self.backoff(attempt, fastrand::f64())
    }

    fn backoff(&self, attempt: u32, random: f64) -> Duration {
        let nanoseconds = self
            .backoff_initial
            .as_nanos()
            .saturating_mul(2_u128.saturating_pow(attempt))
            .min(self.backoff_max.as_nanos());
        let exponential = Duration::new(
            (nanoseconds / 1_000_000_000) as u64,
            (nanoseconds % 1_000_000_000) as u32,
        );
        let seconds = exponential.as_secs_f64() * (1.0 - random * self.backoff_jitter);
        Duration::try_from_secs_f64(seconds)
            .unwrap_or(exponential)
            .min(exponential)
    }
}

/// Parse a server retry delay, preferring `retry-after-ms` over `Retry-After`.
///
/// Supports nonnegative fractional milliseconds or seconds and HTTP dates. An
/// expired date means zero delay. Invalid, non-finite, and unrepresentable values
/// are ignored; an invalid millisecond hint falls back to the seconds/date hint.
pub fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    parse_retry_after_at(headers, SystemTime::now())
}

fn parse_retry_after_at(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    if let Some(delay) = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| parse_seconds(raw, 0.001))
    {
        return Some(delay);
    }
    let raw = headers.get("retry-after")?.to_str().ok()?.trim();
    if let Some(delay) = parse_seconds(raw, 1.0) {
        return Some(delay);
    }
    let date = httpdate::parse_http_date(raw).ok()?;
    timer_duration(date.duration_since(now).unwrap_or(Duration::ZERO))
}

fn parse_seconds(raw: &str, multiplier: f64) -> Option<Duration> {
    let raw = raw.trim();
    let value = if raw.is_empty() {
        0.0
    } else {
        raw.parse::<f64>().ok()?
    };
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    timer_duration(Duration::try_from_secs_f64(value * multiplier).ok()?)
}

fn timer_duration(duration: Duration) -> Option<Duration> {
    Instant::now().checked_add(duration).map(|_| duration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApiError;
    use reqwest::StatusCode;
    use serde_json::Value;

    fn headers(ms: Option<&str>, seconds: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(ms) = ms {
            headers.insert("retry-after-ms", ms.parse().unwrap());
        }
        if let Some(seconds) = seconds {
            headers.insert("retry-after", seconds.parse().unwrap());
        }
        headers
    }

    #[test]
    fn parses_fractional_delays_and_prefers_milliseconds() {
        assert_eq!(
            parse_retry_after(&headers(Some("250.5"), Some("3"))),
            Some(Duration::from_micros(250_500))
        );
        assert_eq!(
            parse_retry_after(&headers(None, Some("1.25"))),
            Some(Duration::from_millis(1250))
        );
        assert_eq!(
            parse_retry_after(&headers(Some(" "), Some("3"))),
            Some(Duration::ZERO)
        );
        assert_eq!(
            parse_retry_after(&headers(Some("invalid"), Some("3"))),
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn handles_hostile_header_numbers_without_panicking() {
        for raw in ["NaN", "inf", "-inf", "1e300", "-1", "1e99999999999"] {
            assert_eq!(parse_retry_after(&headers(None, Some(raw))), None, "{raw}");
            assert_eq!(
                parse_retry_after(&headers(Some(raw), Some("2"))),
                Some(Duration::from_secs(2)),
                "{raw}"
            );
        }
        assert_eq!(
            parse_retry_after(&headers(None, Some("18446744073709549568"))),
            None
        );
    }

    #[test]
    fn parses_http_dates_and_clamps_past_dates() {
        let date = "Wed, 21 Oct 2015 07:28:00 GMT";
        let timestamp = httpdate::parse_http_date(date).unwrap();
        let headers = headers(None, Some(date));
        assert_eq!(
            parse_retry_after_at(&headers, timestamp - Duration::from_secs(5)),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            parse_retry_after_at(&headers, timestamp + Duration::from_secs(5)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn exponential_backoff_caps_and_supports_disabled_waits() {
        let mut policy = RetryPolicy {
            backoff_jitter: 0.0,
            ..RetryPolicy::default()
        };
        for (attempt, milliseconds) in [(0, 500), (1, 1000), (2, 2000), (4, 5000), (u32::MAX, 5000)]
        {
            assert_eq!(
                policy.delay(attempt, None),
                Duration::from_millis(milliseconds)
            );
        }
        policy.backoff_initial = Duration::ZERO;
        assert_eq!(policy.delay(u32::MAX, None), Duration::ZERO);
        policy.backoff_initial = Duration::MAX;
        policy.backoff_max = Duration::MAX;
        assert_eq!(policy.delay(u32::MAX, None), Duration::MAX);
        policy.backoff_max = Duration::ZERO;
        assert_eq!(policy.delay(u32::MAX, None), Duration::ZERO);
    }

    #[test]
    fn jitter_only_reduces_capped_backoff() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.backoff(0, 0.0), Duration::from_millis(500));
        assert_eq!(policy.backoff(0, 1.0), Duration::from_millis(375));
        assert_eq!(policy.backoff(10, 1.0), Duration::from_millis(3750));
    }

    #[test]
    fn server_hints_respect_the_configured_limit() {
        let mut policy = RetryPolicy {
            backoff_jitter: 0.0,
            ..RetryPolicy::default()
        };
        assert_eq!(
            policy.delay(0, Some(&headers(None, Some("60")))),
            Duration::from_secs(60)
        );
        assert_eq!(
            policy.delay(0, Some(&headers(None, Some("61")))),
            Duration::from_millis(500)
        );
        policy.max_retry_after = None;
        assert_eq!(
            policy.delay(0, Some(&headers(None, Some("61")))),
            Duration::from_secs(61)
        );
        policy.respect_retry_after = false;
        assert_eq!(
            policy.delay(0, Some(&headers(None, Some("10")))),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn validates_jitter_and_budget() {
        for jitter in [f64::NAN, f64::INFINITY, -0.01, 1.01] {
            assert!(RetryPolicy {
                backoff_jitter: jitter,
                ..RetryPolicy::default()
            }
            .validate()
            .is_err());
        }
        assert!(RetryPolicy {
            timeout: Some(Duration::ZERO),
            ..RetryPolicy::default()
        }
        .validate()
        .is_err());
        assert!(RetryPolicy {
            timeout: None,
            backoff_jitter: 1.0,
            ..RetryPolicy::default()
        }
        .validate()
        .is_ok());
        assert!(RetryPolicy {
            backoff_initial: Duration::MAX,
            ..RetryPolicy::default()
        }
        .validate()
        .is_err());
        assert!(RetryPolicy {
            backoff_max: Duration::MAX,
            ..RetryPolicy::default()
        }
        .validate()
        .is_err());
        assert!(RetryPolicy {
            max_retry_after: Some(Duration::MAX),
            ..RetryPolicy::default()
        }
        .validate()
        .is_err());
        assert!(RetryPolicy {
            timeout: Some(Duration::MAX),
            ..RetryPolicy::default()
        }
        .validate()
        .is_err());
        assert!(RetryPolicy {
            http_statuses: [99, 1000].into_iter().collect(),
            ..RetryPolicy::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn status_rules_and_custom_predicates_are_additive_but_never_cancelled() {
        let mut policy = RetryPolicy::default();
        for (status, retry) in [
            (400, false),
            (401, false),
            (408, true),
            (429, true),
            (500, true),
            (599, true),
        ] {
            let error = ApiError::new(
                StatusCode::from_u16(status).unwrap(),
                Value::Null,
                HeaderMap::new(),
                None,
            )
            .into();
            assert_eq!(policy.should_retry(&error), retry, "{status}");
        }
        let error = Error::InvalidRequest("custom".into());
        assert!(!policy.should_retry(&error));
        policy.predicate = Some(Arc::new(|_| true));
        assert!(policy.should_retry(&error));
        assert!(!policy.should_retry(&Error::Cancelled));
    }

    #[test]
    fn successful_validation_errors_require_explicit_retry_rules() {
        let error = Error::ResponseValidation {
            field_path: "answers".into(),
            response: Box::new(crate::RawResponse {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                body: Default::default(),
            }),
        };
        let mut policy = RetryPolicy::default();
        assert!(!policy.should_retry(&error));
        policy.http_statuses.insert(200);
        assert!(policy.should_retry(&error));
    }
}
