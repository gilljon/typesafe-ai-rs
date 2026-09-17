use crate::{
    config::{validate_timeout, Config},
    ApiError, Error, HeaderMap, RawResponse, RequestOptions, RetryPolicy, VERSION,
};
use reqwest::{header, Method};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub(crate) struct Prepared {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    pub retry: RetryPolicy,
}

pub(crate) fn prepare(
    config: &Config,
    method: Method,
    path: &str,
    body: Option<Value>,
    options: &RequestOptions,
) -> Result<Prepared, Error> {
    let timeout = options.timeout.unwrap_or(config.timeout);
    validate_timeout(timeout)?;
    let retry = options
        .retry
        .clone()
        .unwrap_or_else(|| config.retry.clone());
    retry.validate()?;
    let mut headers = config.headers.clone();
    headers.extend(options.headers.clone());
    headers.remove("x-typesafe-retry-count");
    headers.insert(header::AUTHORIZATION, config.authorization.clone());
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static("application/json"),
    );
    let identity = header::HeaderValue::from_str(&format!("typesafe-ai-rs/{VERSION}"))
        .expect("valid package version");
    headers.insert(header::USER_AGENT, identity.clone());
    headers.insert("x-typesafe-sdk", identity);
    headers.insert(
        "x-typesafe-runtime",
        header::HeaderValue::from_str(&format!(
            "rust ({}; {})",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
        .expect("valid target identifiers"),
    );
    let body = body
        .map(|body| serde_json::to_vec(&body))
        .transpose()
        .map_err(|e| Error::InvalidRequest(format!("Cannot encode request as JSON: {e}")))?;
    if body.is_some() {
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
    }
    Ok(Prepared {
        method,
        url: format!("{}{path}", config.base_url),
        headers,
        body,
        timeout,
        retry,
    })
}

impl Prepared {
    pub fn headers(&self, attempt: u32) -> HeaderMap {
        let mut headers = self.headers.clone();
        if attempt > 0 {
            headers.insert(
                "x-typesafe-retry-count",
                header::HeaderValue::from_str(&attempt.to_string()).expect("valid retry count"),
            );
        }
        headers
    }

    pub fn retry_delay(&self, error: &Error, attempt: u32, started: Instant) -> Option<Duration> {
        if attempt >= self.retry.max_retries || !self.retry.should_retry(error) {
            return None;
        }
        let headers = match error {
            Error::Api(error) => Some(&error.headers),
            Error::ResponseValidation { response, .. } => Some(&response.headers),
            _ => None,
        };
        let delay = self.retry.delay(attempt, headers);
        if self
            .retry
            .timeout
            .is_some_and(|budget| started.elapsed().saturating_add(delay) >= budget)
        {
            return None;
        }
        Some(delay)
    }

    pub fn map_error(&self, error: reqwest::Error) -> Error {
        // reqwest error URLs can contain details supplied by a custom HTTP client.
        let error = error.without_url();
        if error.is_timeout() {
            Error::Timeout {
                timeout: self.timeout,
                source: error,
            }
        } else {
            Error::Connection(error)
        }
    }

    pub fn finish<T>(
        &self,
        raw: RawResponse,
        decode: impl Fn(RawResponse) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if !raw.status.is_success() {
            let body = if raw.body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&raw.body).unwrap_or_else(|_| {
                    Value::String(String::from_utf8_lossy(&raw.body).into_owned())
                })
            };
            return Err(Error::Api(Box::new(ApiError::new(
                raw.status,
                body,
                raw.headers,
                Some(format!("{} {}", self.method, self.url)),
            ))));
        }
        decode(raw)
    }

    pub fn log_request(&self, config: &Config, headers: &HeaderMap, attempt: u32) {
        if config.log_level >= log::LevelFilter::Debug
            && log::log_enabled!(target: "typesafe_ai_rs", log::Level::Debug)
        {
            log::debug!(target: "typesafe_ai_rs", "{} {} attempt={} headers={:?} body={:?}", self.method, self.url, u64::from(attempt) + 1, redact_headers(headers), self.body.as_ref().map(|b| String::from_utf8_lossy(b)));
        }
    }

    pub fn log_response(&self, config: &Config, raw: &RawResponse, started: Instant) {
        if config.log_level >= log::LevelFilter::Info {
            log::info!(target: "typesafe_ai_rs", "{} {} status={} elapsed_ms={} request_id={}", self.method, self.url, raw.status.as_u16(), started.elapsed().as_millis(), raw.request_id().unwrap_or("-"));
        }
        if config.log_level >= log::LevelFilter::Debug
            && log::log_enabled!(target: "typesafe_ai_rs", log::Level::Debug)
        {
            log::debug!(target: "typesafe_ai_rs", "response headers={:?} body={:?}", redact_headers(&raw.headers), String::from_utf8_lossy(&raw.body));
        }
    }
}

fn redact_headers(headers: &HeaderMap) -> BTreeMap<&str, String> {
    headers
        .iter()
        .map(|(name, value)| {
            let key = name.as_str();
            let secret = value.is_sensitive()
                || matches!(
                    key,
                    "authorization"
                        | "proxy-authorization"
                        | "api-key"
                        | "x-api-key"
                        | "cookie"
                        | "set-cookie"
                )
                || key.contains("token")
                || key.contains("secret");
            (
                key,
                if secret {
                    "[REDACTED]".into()
                } else {
                    value.to_str().unwrap_or("[binary]").into()
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_are_redacted_including_custom_headers() {
        let mut headers = HeaderMap::new();
        for name in [
            "authorization",
            "x-refresh-token",
            "app-secret",
            "api-key",
            "set-cookie",
        ] {
            headers.insert(name, header::HeaderValue::from_static("private"));
        }
        headers.insert("x-request-id", header::HeaderValue::from_static("visible"));
        let safe = redact_headers(&headers);
        assert_eq!(safe["x-request-id"], "visible");
        assert!(!format!("{safe:?}").contains("private"));
    }
}
