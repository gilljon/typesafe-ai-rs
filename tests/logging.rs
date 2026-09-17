use std::sync::Mutex;

use serde_json::json;
use typesafe_ai_rs::{Client, HeaderMap, HeaderValue, LogLevel, Noul, SystemOneRequest};
use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};

struct CaptureLogger(Mutex<Vec<String>>);

impl log::Log for CaptureLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == "typesafe_ai_rs"
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.0.lock().unwrap().push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

static LOGGER: CaptureLogger = CaptureLogger(Mutex::new(Vec::new()));

// A separate test binary keeps the process-global logger isolated from other
// integration suites and application logger configuration.
#[tokio::test]
async fn client_log_filter_silences_unknown_answers_and_debug_redacts_wire_secrets() {
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
    let server = MockServer::start().await;
    Mock::given(path("/off/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-latest", "usage": {},
            "answers": {"future": {"type": "unrecognized-answer"}}
        })))
        .mount(&server)
        .await;
    Client::builder()
        .api_key("off-secret")
        .base_url(format!("{}/off", server.uri()))
        .log_level(LogLevel::Off)
        .build()
        .unwrap()
        .system_one(SystemOneRequest::new(
            "text",
            [("q", Noul::new("Question?"))],
        ))
        .await
        .unwrap();
    assert!(LOGGER.0.lock().unwrap().is_empty());

    Mock::given(path("/debug/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "model": "debug-response-marker", "usage": {},
                    "answers": {"q": {"type": "noul", "noul": 0.9}}
                }))
                .insert_header("x-typesafe-request-id", "req-debug")
                .insert_header("set-cookie", "response-cookie-secret")
                .insert_header("x-server-secret", "response-header-secret"),
        )
        .mount(&server)
        .await;
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", "custom-key-secret".parse().unwrap());
    headers.insert("cookie", "request-cookie-secret".parse().unwrap());
    headers.insert("x-vendor-token", "vendor-token-secret".parse().unwrap());
    headers.insert("x-debug-visible", "visible-header".parse().unwrap());
    let mut marked = HeaderValue::from_static("marked-header-secret");
    marked.set_sensitive(true);
    headers.insert("x-sensitive", marked);
    Client::builder()
        .api_key("sdk-log-secret")
        .base_url(format!("{}/debug", server.uri()))
        .log_level(LogLevel::Debug)
        .default_headers(headers)
        .build()
        .unwrap()
        .system_one(SystemOneRequest::new(
            "debug-body-marker",
            [("q", Noul::new("Question?"))],
        ))
        .await
        .unwrap();

    let logs = LOGGER.0.lock().unwrap().join("\n");
    for expected in [
        "debug-body-marker",
        "debug-response-marker",
        "visible-header",
        "req-debug",
        "[REDACTED]",
    ] {
        assert!(
            logs.contains(expected),
            "expected {expected} in debug logging"
        );
    }
    for secret in [
        "sdk-log-secret",
        "custom-key-secret",
        "request-cookie-secret",
        "vendor-token-secret",
        "marked-header-secret",
        "response-cookie-secret",
        "response-header-secret",
    ] {
        assert!(!logs.contains(secret), "debug logging exposed {secret}");
    }
}
