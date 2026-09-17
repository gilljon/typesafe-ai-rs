//! Errors and response metadata returned by the SDK.

use std::{fmt, time::Duration};

use reqwest::{header::HeaderMap, StatusCode, Url};
use serde_json::Value;

/// An error returned by the TypeSafe SDK.
pub enum Error {
    /// Invalid client configuration.
    Configuration(String),
    /// A request could not be constructed from the supplied arguments.
    InvalidRequest(String),
    /// The server returned an unsuccessful HTTP status.
    Api(Box<ApiError>),
    /// A successful response did not contain the required data.
    ResponseValidation {
        /// Dotted path to the first invalid or missing field.
        field_path: String,
        /// The original response, available for inspection.
        response: Box<crate::RawResponse>,
    },
    /// The request or response body could not be delivered.
    Connection(reqwest::Error),
    /// The full response did not arrive within the request timeout.
    Timeout {
        /// The configured timeout for this attempt.
        timeout: Duration,
        /// The underlying transport error.
        source: reqwest::Error,
    },
    /// The caller cancelled the request.
    Cancelled,
}

impl Error {
    /// HTTP status for an API or response-validation error.
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            Self::Api(error) => Some(error.status),
            Self::ResponseValidation { response, .. } => Some(response.status),
            _ => None,
        }
    }

    /// Response request ID, if the server provided one.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Api(error) => error.request_id(),
            Self::ResponseValidation { response, .. } => response.request_id(),
            _ => None,
        }
    }

    /// The structured server error, when this is an HTTP failure.
    pub fn as_api_error(&self) -> Option<&ApiError> {
        match self {
            Self::Api(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(message) => write!(f, "Invalid configuration: {message}"),
            Self::InvalidRequest(message) => write!(f, "Invalid request: {message}"),
            Self::Api(error) => error.fmt(f),
            Self::ResponseValidation {
                field_path,
                response,
            } => {
                write!(
                    f,
                    "{} Invalid response data at {field_path:?}",
                    response.status
                )?;
                if let Some(request_id) = response.request_id() {
                    write!(f, " (request_id={request_id})")?;
                }
                Ok(())
            }
            Self::Connection(_) => f.write_str("Request connection failed"),
            Self::Timeout { timeout, .. } => {
                write!(f, "Request timed out (timeout={timeout:?})")
            }
            Self::Cancelled => f.write_str("Request was cancelled"),
        }
    }
}

// Transport errors and raw responses can contain credentials or document data.
// Keep them accessible through explicit fields without including them in logs.
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(error) => f.debug_tuple("Api").field(error).finish(),
            Self::ResponseValidation {
                field_path,
                response,
            } => f
                .debug_struct("ResponseValidation")
                .field("field_path", field_path)
                .field("status", &response.status)
                .field("request_id", &response.request_id())
                .finish_non_exhaustive(),
            _ => fmt::Display::fmt(self, f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Api(error) => Some(error.as_ref()),
            Self::Connection(error) | Self::Timeout { source: error, .. } => Some(error),
            _ => None,
        }
    }
}

impl From<ApiError> for Error {
    fn from(error: ApiError) -> Self {
        Self::Api(Box::new(error))
    }
}

/// Classification of an unsuccessful HTTP response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ApiErrorKind {
    /// HTTP 400.
    BadRequest,
    /// HTTP 401.
    Authentication,
    /// HTTP 403.
    PermissionDenied,
    /// HTTP 404.
    NotFound,
    /// HTTP 422.
    UnprocessableEntity,
    /// HTTP 429.
    RateLimit,
    /// HTTP 5xx.
    InternalServer,
    /// Another unsuccessful status.
    Other,
}

/// An unsuccessful HTTP response with its body and request metadata.
///
/// `Display` and `Debug` omit the body and arbitrary headers. Inspect [`Self::message`]
/// or `body` explicitly when server details are appropriate to expose.
#[derive(Clone)]
pub struct ApiError {
    /// HTTP response status.
    pub status: StatusCode,
    /// JSON error data, response text as a string, or null for an empty body.
    pub body: Value,
    /// The original response headers.
    pub headers: HeaderMap,
    /// Request method and URL, without URL credentials, query, or fragment.
    pub endpoint: Option<String>,
}

impl ApiError {
    /// Construct an API error and sanitize the optional endpoint.
    pub fn new(
        status: StatusCode,
        body: Value,
        headers: HeaderMap,
        endpoint: Option<String>,
    ) -> Self {
        Self {
            status,
            body,
            headers,
            endpoint: endpoint.map(|endpoint| sanitize_endpoint(&endpoint)),
        }
    }

    /// Classify the error by its HTTP status.
    pub fn kind(&self) -> ApiErrorKind {
        match self.status.as_u16() {
            400 => ApiErrorKind::BadRequest,
            401 => ApiErrorKind::Authentication,
            403 => ApiErrorKind::PermissionDenied,
            404 => ApiErrorKind::NotFound,
            422 => ApiErrorKind::UnprocessableEntity,
            429 => ApiErrorKind::RateLimit,
            500..=599 => ApiErrorKind::InternalServer,
            _ => ApiErrorKind::Other,
        }
    }

    /// The `x-typesafe-request-id` response header, if valid UTF-8.
    pub fn request_id(&self) -> Option<&str> {
        self.headers
            .get("x-typesafe-request-id")
            .and_then(|value| value.to_str().ok())
    }

    /// Server-requested retry delay, preferring `retry-after-ms`.
    pub fn retry_after(&self) -> Option<Duration> {
        crate::retry::parse_retry_after(&self.headers)
    }

    /// Extract the server's error message, including FastAPI validation details.
    ///
    /// This can contain response data; it is deliberately omitted from automatic
    /// error formatting. Unstructured fallback bodies are limited to 200 characters.
    pub fn message(&self) -> String {
        if let Some(message) = extract_message(&self.body).filter(|message| !message.is_empty()) {
            return message;
        }
        if self.body.is_null() {
            return "status code (no body)".into();
        }
        let raw = self
            .body
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| self.body.to_string());
        let mut chars = raw.chars();
        let mut message: String = chars.by_ref().take(200).collect();
        if chars.next().is_some() {
            message.push('…');
        }
        message
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(endpoint) = &self.endpoint {
            write!(f, "{}: ", sanitize_endpoint(endpoint))?;
        }
        write!(f, "{}", self.status)?;
        if let Some(request_id) = self.request_id() {
            write!(f, " (request_id={request_id})")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiError")
            .field("kind", &self.kind())
            .field("status", &self.status)
            .field("endpoint", &self.endpoint.as_deref().map(sanitize_endpoint))
            .field("request_id", &self.request_id())
            .finish_non_exhaustive()
    }
}

impl std::error::Error for ApiError {}

fn sanitize_endpoint(endpoint: &str) -> String {
    let (method, raw_url) = match endpoint.split_once(' ') {
        Some((method, url)) if method.bytes().all(|byte| byte.is_ascii_uppercase()) => {
            (Some(method), url)
        }
        _ => (None, endpoint),
    };
    let sanitized = match Url::parse(raw_url) {
        Ok(mut url) if matches!(url.scheme(), "http" | "https") => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        _ => "<invalid URL>".into(),
    };
    match method {
        Some(method) => format!("{method} {sanitized}"),
        None => sanitized,
    }
}

fn extract_message(body: &Value) -> Option<String> {
    if let Some(message) = body.as_str() {
        return Some(message.to_owned());
    }
    let body = body.as_object()?;
    let error = body.get("error");
    let detail = body.get("detail");
    let message = error
        .and_then(Value::as_str)
        .or_else(|| error?.get("message")?.as_str())
        .or_else(|| body.get("message")?.as_str())
        .or_else(|| detail?.as_str())
        .or_else(|| detail?.get("message")?.as_str());
    if let Some(message) = message {
        return Some(message.to_owned());
    }
    let parts: Vec<_> = detail?
        .as_array()?
        .iter()
        .filter_map(|entry| {
            let message = entry.get("msg")?.as_str()?;
            let path = entry
                .get("loc")
                .and_then(Value::as_array)
                .map(|location| {
                    location
                        .iter()
                        .filter(|part| part.as_str() != Some("body"))
                        .map(|part| {
                            part.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| part.to_string())
                        })
                        .collect::<Vec<_>>()
                        .join(".")
                })
                .unwrap_or_default();
            Some(if path.is_empty() {
                message.to_owned()
            } else {
                format!("{path}: {message}")
            })
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_all_specialized_statuses() {
        for (status, kind) in [
            (400, ApiErrorKind::BadRequest),
            (401, ApiErrorKind::Authentication),
            (403, ApiErrorKind::PermissionDenied),
            (404, ApiErrorKind::NotFound),
            (422, ApiErrorKind::UnprocessableEntity),
            (429, ApiErrorKind::RateLimit),
            (500, ApiErrorKind::InternalServer),
            (599, ApiErrorKind::InternalServer),
            (409, ApiErrorKind::Other),
        ] {
            assert_eq!(
                ApiError::new(
                    StatusCode::from_u16(status).unwrap(),
                    Value::Null,
                    HeaderMap::new(),
                    None,
                )
                .kind(),
                kind,
            );
        }
    }

    #[test]
    fn extracts_message_shapes_and_validation_paths() {
        for (body, expected) in [
            (json!("plain"), "plain"),
            (json!({"error": "error", "message": "message"}), "error"),
            (json!({"error": {"message": "nested"}}), "nested"),
            (json!({"message": "message"}), "message"),
            (json!({"detail": "detail"}), "detail"),
            (
                json!({"detail": {"message": "nested detail"}}),
                "nested detail",
            ),
            (
                json!({"detail": [
                    {"loc": ["body", "questions", 0, "name"], "msg": "required"},
                    {"msg": "invalid input"},
                    {"loc": ["ignored"]}
                ]}),
                "questions.0.name: required; invalid input",
            ),
        ] {
            let error = ApiError::new(StatusCode::BAD_REQUEST, body, HeaderMap::new(), None);
            assert_eq!(error.message(), expected);
        }
    }

    #[test]
    fn logging_omits_response_data_and_sanitizes_urls() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer secret-header".parse().unwrap());
        headers.insert("x-typesafe-request-id", "req-123".parse().unwrap());
        let error = ApiError::new(
            StatusCode::UNAUTHORIZED,
            json!({"message": "secret-body"}),
            headers,
            Some("POST https://user:secret-password@example.com/v1/extract?key=secret-query#secret-fragment".into()),
        );
        assert_eq!(
            error.endpoint.as_deref(),
            Some("POST https://example.com/v1/extract")
        );
        for formatted in [
            format!("{error}"),
            format!("{error:?}"),
            format!("{:?}", Error::from(error)),
        ] {
            assert!(formatted.contains("req-123"));
            assert!(!formatted.contains("secret"));
            assert!(!formatted.contains("user:"));
        }
    }

    #[test]
    fn raw_fallback_truncates_on_unicode_character_boundaries() {
        let error = ApiError::new(
            StatusCode::BAD_REQUEST,
            json!(["é".repeat(300)]),
            HeaderMap::new(),
            None,
        );
        assert_eq!(error.message().chars().count(), 201);
        assert!(error.message().ends_with('…'));
    }

    #[test]
    fn validation_errors_retain_metadata_without_logging_raw_response() {
        let mut headers = HeaderMap::new();
        headers.insert("x-typesafe-request-id", "req-456".parse().unwrap());
        headers.insert("set-cookie", "secret-session".parse().unwrap());
        let error = Error::ResponseValidation {
            field_path: "answers.tone.confidence".into(),
            response: Box::new(crate::RawResponse {
                status: StatusCode::OK,
                headers,
                body: b"secret-document".to_vec().into(),
            }),
        };
        assert_eq!(error.status(), Some(StatusCode::OK));
        assert_eq!(error.request_id(), Some("req-456"));
        for formatted in [format!("{error}"), format!("{error:?}")] {
            assert!(formatted.contains("answers.tone.confidence"));
            assert!(formatted.contains("req-456"));
            assert!(!formatted.contains("secret"));
        }
    }
}
