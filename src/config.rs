use crate::{
    CancellationToken, Error, HeaderMap, RetryPolicy, DEFAULT_BASE_URL, DEFAULT_MODEL,
    DEFAULT_TIMEOUT,
};
use std::{fmt, time::Duration};

/// Per-client log filtering. Messages use the `typesafe_ai_rs` target of the `log` facade.
/// Applications install their own logger; this library never installs a global logger.
pub type LogLevel = log::LevelFilter;

/// Per-call overrides. An omitted field inherits the client's setting.
#[derive(Clone, Default)]
pub struct RequestOptions {
    /// Complete retry policy override; use struct update syntax for partial changes.
    pub retry: Option<RetryPolicy>,
    /// Timeout per attempt, including reading the response body.
    pub timeout: Option<Duration>,
    /// Additional headers. SDK protocol and authentication headers remain protected.
    pub headers: HeaderMap,
    /// Cancels async requests, response reads, and retry delays.
    /// Blocking calls reject this option; use the async client for cancellation.
    pub cancellation_token: Option<CancellationToken>,
}

impl fmt::Debug for RequestOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestOptions")
            .field("retry", &self.retry)
            .field("timeout", &self.timeout)
            .field("header_count", &self.headers.len())
            .field("cancellable", &self.cancellation_token.is_some())
            .finish()
    }
}

#[derive(Clone, Default)]
pub(crate) struct ConfigBuilder {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub timeout: Option<Duration>,
    pub retry: RetryPolicy,
    pub headers: HeaderMap,
    pub log_level: Option<LogLevel>,
}

impl fmt::Debug for ConfigBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigBuilder").finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub(crate) struct Config {
    pub authorization: reqwest::header::HeaderValue,
    pub base_url: String,
    pub model: String,
    pub timeout: Duration,
    pub retry: RetryPolicy,
    pub headers: HeaderMap,
    pub log_level: LogLevel,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

pub(crate) fn validate_timeout(timeout: Duration) -> Result<(), Error> {
    if timeout.is_zero() || std::time::Instant::now().checked_add(timeout).is_none() {
        return Err(Error::Configuration(
            "timeout must be positive and representable".into(),
        ));
    }
    Ok(())
}

impl ConfigBuilder {
    pub fn resolve(self) -> Result<Config, Error> {
        let key = self
            .api_key
            .or_else(|| env("TYPESAFE_API_KEY"))
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                Error::Configuration("Pass an API key or set TYPESAFE_API_KEY".into())
            })?;
        let mut authorization = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| {
                Error::Configuration("API key contains invalid HTTP header characters".into())
            })?;
        authorization.set_sensitive(true);
        let base_url = self
            .base_url
            .or_else(|| env("TYPESAFE_BASE_URL"))
            .unwrap_or_else(|| DEFAULT_BASE_URL.into());
        let url = reqwest::Url::parse(&base_url)
            .map_err(|_| Error::Configuration("base_url must be an absolute HTTP(S) URL".into()))?;
        if !matches!(url.scheme(), "https" | "http")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Configuration(
                "base_url must be an HTTP(S) root without credentials, query, or fragment".into(),
            ));
        }
        let timeout = self.timeout.unwrap_or(DEFAULT_TIMEOUT);
        validate_timeout(timeout)?;
        self.retry.validate()?;
        let log_level = match self.log_level {
            Some(level) => level,
            None => match env("TYPESAFE_LOG_LEVEL").as_deref() {
                None => LogLevel::Warn,
                Some("debug") => LogLevel::Debug,
                Some("info") => LogLevel::Info,
                Some("warn" | "warning") => LogLevel::Warn,
                Some("error") => LogLevel::Error,
                Some("off") => LogLevel::Off,
                Some(_) => {
                    return Err(Error::Configuration(
                        "TYPESAFE_LOG_LEVEL must be debug, info, warn, warning, error, or off"
                            .into(),
                    ))
                }
            },
        };
        Ok(Config {
            authorization,
            base_url: url.as_str().trim_end_matches('/').to_owned(),
            model: self
                .model
                .or_else(|| env("TYPESAFE_DEFAULT_MODEL"))
                .unwrap_or_else(|| DEFAULT_MODEL.into()),
            timeout,
            retry: self.retry,
            headers: self.headers,
            log_level,
        })
    }
}

// Both transports expose the same configuration without maintaining two copies.
macro_rules! config_methods {
    () => {
        /// API key; defaults to `TYPESAFE_API_KEY`.
        pub fn api_key(mut self, value: impl Into<String>) -> Self {
            self.config.api_key = Some(value.into());
            self
        }
        /// API root; defaults to `TYPESAFE_BASE_URL` or the public API.
        pub fn base_url(mut self, value: impl Into<String>) -> Self {
            self.config.base_url = Some(value.into());
            self
        }
        /// Default model; defaults to `TYPESAFE_DEFAULT_MODEL` or `jev-latest`.
        pub fn model(mut self, value: impl Into<String>) -> Self {
            self.config.model = Some(value.into());
            self
        }
        /// Timeout for each complete HTTP attempt.
        pub fn timeout(mut self, value: std::time::Duration) -> Self {
            self.config.timeout = Some(value);
            self
        }
        /// Retry policy for all calls unless overridden per request.
        pub fn retry(mut self, value: crate::RetryPolicy) -> Self {
            self.config.retry = value;
            self
        }
        /// Default headers; request headers take precedence except protected protocol headers.
        pub fn default_headers(mut self, value: crate::HeaderMap) -> Self {
            self.config.headers = value;
            self
        }
        /// Filter messages emitted through the application's `log` implementation.
        pub fn log_level(mut self, value: crate::LogLevel) -> Self {
            self.config.log_level = Some(value);
            self
        }
    };
}
pub(crate) use config_methods;
