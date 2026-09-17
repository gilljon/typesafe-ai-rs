use std::{process::Command, time::Duration};
use typesafe_ai_rs::{Client, Error, HeaderMap, RequestOptions, RetryPolicy};

#[test]
fn rejects_invalid_config_without_network() {
    for url in [
        "ftp://example.com",
        "relative",
        "https://u:p@example.com",
        "https://example.com?key=secret",
        "https://example.com#fragment",
    ] {
        assert!(matches!(
            Client::builder().api_key("key").base_url(url).build(),
            Err(Error::Configuration(_))
        ));
    }
    assert!(Client::builder().api_key("bad\nkey").build().is_err());
    assert!(Client::builder().api_key("").build().is_err());
    assert!(Client::builder()
        .api_key("key")
        .timeout(Duration::ZERO)
        .build()
        .is_err());
    assert!(Client::builder()
        .api_key("key")
        .timeout(Duration::MAX)
        .build()
        .is_err());
    assert!(Client::builder()
        .api_key("key")
        .retry(RetryPolicy {
            backoff_jitter: f64::NAN,
            ..Default::default()
        })
        .build()
        .is_err());
}

#[test]
fn debug_omits_credentials() {
    let mut headers = HeaderMap::new();
    headers.insert("x-secret", "hidden-options-secret".parse().unwrap());
    let options = RequestOptions {
        headers: headers.clone(),
        ..Default::default()
    };
    let builder = Client::builder()
        .api_key("hidden-api-key")
        .default_headers(headers);
    assert!(!format!("{options:?} {builder:?}").contains("hidden-"));
    let client = builder.build().unwrap();
    assert!(!format!("{client:?}").contains("hidden-"));
}

// Environment behavior is tested in isolated processes: no process-global env
// mutation races against parallel tests or application code.
#[test]
fn environment_resolution_isolated() {
    for mode in ["configured", "blank", "missing", "invalid-log"] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "environment_probe", "--nocapture"])
            .env("TYPESAFE_TEST_ENV_MODE", mode)
            .env(
                "TYPESAFE_API_KEY",
                if mode == "missing" {
                    " "
                } else {
                    "  env-key  "
                },
            )
            .env(
                "TYPESAFE_BASE_URL",
                if mode == "blank" {
                    " "
                } else {
                    " https://env.example/api/// "
                },
            )
            .env(
                "TYPESAFE_DEFAULT_MODEL",
                if mode == "blank" { " " } else { " env-model " },
            )
            .env(
                "TYPESAFE_LOG_LEVEL",
                if mode == "invalid-log" {
                    "verbose"
                } else {
                    " off "
                },
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn environment_probe() {
    let Ok(mode) = std::env::var("TYPESAFE_TEST_ENV_MODE") else {
        return;
    };
    if matches!(mode.as_str(), "missing" | "invalid-log") {
        assert!(matches!(Client::new(), Err(Error::Configuration(_))));
        return;
    }
    let client = Client::new().unwrap();
    if mode == "blank" {
        assert_eq!(client.base_url(), typesafe_ai_rs::DEFAULT_BASE_URL);
        assert_eq!(client.default_model(), typesafe_ai_rs::DEFAULT_MODEL);
    } else {
        assert_eq!(client.base_url(), "https://env.example/api");
        assert_eq!(client.default_model(), "env-model");
    }
    let explicit = Client::builder()
        .api_key("explicit")
        .base_url("https://explicit.example/")
        .model("explicit-model")
        .build()
        .unwrap();
    assert_eq!(explicit.base_url(), "https://explicit.example");
    assert_eq!(explicit.default_model(), "explicit-model");
}
