use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{oneshot, Notify},
};
use typesafe_ai_rs::{
    ApiErrorKind, CancellationToken, Choice, Client, Error, HeaderMap, LogLevel, Noul, Question,
    RequestOptions, RetryPolicy, Score, SystemOneRequest,
};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn client(base_url: &str) -> Client {
    Client::builder()
        .api_key("test-api-key-never-log")
        .base_url(base_url)
        .log_level(LogLevel::Off)
        .retry(RetryPolicy::no_retries())
        .build()
        .unwrap()
}

fn models_body() -> Value {
    json!({"models": [{"name": "jev-latest", "description": "A model", "release_date": "2026-01-01", "future_field": 1}]})
}

fn fast_retry(max_retries: u32) -> RetryPolicy {
    RetryPolicy {
        max_retries,
        backoff_initial: Duration::ZERO,
        backoff_jitter: 0.0,
        ..RetryPolicy::default()
    }
}

fn system_one_body() -> Value {
    json!({
        "model": "jev-latest",
        "answers": {
            "billing": {"type": "noul", "noul": 0.9, "future_field": true},
            "tone": {"type": "choice", "choice": "calm", "confidence": 0.8, "probabilities": {"calm": 0.8, "angry": 0.2}},
            "severity": {"type": "score", "score": 0.25, "confidence": 0.75, "legend": {"0": {"description": "low"}, "1": ["high"]}, "probabilities": {"0": 0.75, "1": 0.25}},
            "future": {"type": "future-answer", "value": 42}
        },
        "usage": {"input_tokens": 12, "output_tokens": null},
        "future_field": {"preserved": true}
    })
}

fn system_one_request() -> SystemOneRequest {
    SystemOneRequest::new("A message", [("billing", Noul::new("Is it billing?"))])
}

#[tokio::test]
async fn system_one_preserves_structured_questions_and_decodes_every_answer_type() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(system_one_body())
                .insert_header("x-typesafe-request-id", "req-system"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut defaults = HeaderMap::new();
    defaults.insert("X-Custom", "default".parse().unwrap());
    defaults.insert("Authorization", "default-auth".parse().unwrap());
    let client = Client::builder()
        .api_key("wire-key")
        .base_url(server.uri())
        .model("custom-default")
        .default_headers(defaults)
        .build()
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("x-custom", "per-call".parse().unwrap());
    headers.insert("CONTENT-TYPE", "text/plain".parse().unwrap());
    let questions: [(&str, Question); 3] = [
        (
            "billing",
            Noul::new(json!(["Is it billing?", {"language": "en"}]))
                .null_criteria()
                .into(),
        ),
        (
            "tone",
            Choice::new([("calm", Value::Null), ("angry", json!({"tone": "angry"}))]).into(),
        ),
        (
            "severity",
            Score::new([json!({"description": "low"}), json!(["high"])])
                .instructions("Severity?")
                .into(),
        ),
    ];
    let request = SystemOneRequest::new(json!({"messages": ["Charged twice", null]}), questions)
        .extra_body([("trace", Value::Null)]);
    let response = client
        .system_one_with_options(
            request,
            RequestOptions {
                headers,
                ..RequestOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(response.model, "jev-latest");
    assert_eq!(response.usage.input_tokens, Some(12));
    assert_eq!(response.usage.output_tokens, None);
    assert_eq!(response.nouls()["billing"].noul, 0.9);
    assert_eq!(response.choices()["tone"].choice, "calm");
    assert_eq!(response.scores()["severity"].score, 0.25);
    assert_eq!(
        response.scores()["severity"].legend[&0],
        json!({"description": "low"})
    );
    assert_eq!(response.scores()["severity"].probabilities[&1], 0.25);
    assert!(!response.answers.contains_key("future"));
    assert_eq!(
        response.raw_http_response.json().unwrap()["answers"]["future"]["value"],
        42
    );
    assert_eq!(
        response.raw_http_response.json().unwrap()["future_field"]["preserved"],
        true
    );
    assert_eq!(response.request_id(), Some("req-system"));
    let requests = server.received_requests().await.unwrap();
    let request = &requests[0];
    assert_eq!(request.headers["content-type"], "application/json");
    assert_eq!(request.headers.get_all("content-type").iter().count(), 1);
    assert_eq!(request.headers["x-custom"], "per-call");
    assert_eq!(request.headers.get_all("x-custom").iter().count(), 1);
    assert_eq!(request.headers["authorization"], "Bearer wire-key");
    assert_eq!(
        request.body_json::<Value>().unwrap(),
        json!({
            "state": {"messages": ["Charged twice", null]}, "model": "custom-default", "trace": null,
            "questions": {
                "billing": {"type": "noul", "instructions": ["Is it billing?", {"language": "en"}], "criteria": null},
                "tone": {"type": "choice", "criteria": {"calm": null, "angry": {"tone": "angry"}}},
                "severity": {"type": "score", "instructions": "Severity?", "criteria": [{"description": "low"}, ["high"]]}
            }
        })
    );
}

#[tokio::test]
async fn extra_body_can_shallow_override_standard_fields_and_preserve_null() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(system_one_body()))
        .mount(&server)
        .await;
    let request = system_one_request().model("per-call").extra_body([
        ("state", Value::Null),
        ("model", json!("body-model")),
        (
            "questions",
            json!({"new": {"type": "future-question", "nested": [null, {"extra": true}]}}),
        ),
    ]);
    client(&server.uri()).system_one(request).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0].body_json::<Value>().unwrap(),
        json!({
            "state": null, "model": "body-model", "questions": {"new": {"type": "future-question", "nested": [null, {"extra": true}]}}
        })
    );
}

#[tokio::test]
async fn malformed_success_retains_raw_response_and_reports_the_bad_field() {
    let server = MockServer::start().await;
    let mut malformed = system_one_body();
    malformed["answers"]["tone"]
        .as_object_mut()
        .unwrap()
        .remove("confidence");
    Mock::given(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(malformed.clone())
                .insert_header("x-typesafe-request-id", "req-malformed"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let error = client(&server.uri())
        .system_one_with_options(
            system_one_request(),
            RequestOptions {
                retry: Some(fast_retry(3)),
                ..RequestOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.status().unwrap().as_u16(), 200);
    assert_eq!(error.request_id(), Some("req-malformed"));
    match error {
        Error::ResponseValidation {
            field_path,
            response,
        } => {
            assert_eq!(field_path, "answers.tone.confidence");
            assert_eq!(response.json().unwrap(), malformed);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn models_wrapper_is_validated_even_on_success() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let error = client(&server.uri()).models().list().await.unwrap_err();
    assert!(
        matches!(error, Error::ResponseValidation { field_path, .. } if field_path == "models")
    );
}

#[tokio::test]
async fn invalid_question_sets_fail_before_a_network_request() {
    let server = MockServer::start().await;
    let client = client(&server.uri());
    for request in [
        SystemOneRequest::new("text", std::iter::empty::<(&str, Question)>()),
        SystemOneRequest::new("text", [("score", Score::new(Vec::<Value>::new()))]),
        SystemOneRequest::new("text", [("raw", json!({"instructions": "Missing type"}))]),
    ] {
        assert!(matches!(
            client.system_one(request).await,
            Err(Error::InvalidRequest(_))
        ));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn models_unwraps_cards_preserves_raw_metadata_and_protects_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(models_body())
                .insert_header("x-typesafe-request-id", "req-models"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let mut headers = HeaderMap::new();
    headers.insert("Authorization", "Bearer attacker".parse().unwrap());
    headers.insert("Accept", "text/html".parse().unwrap());
    headers.insert("User-Agent", "attacker".parse().unwrap());
    headers.insert("X-TypeSafe-SDK", "attacker".parse().unwrap());
    headers.insert("X-TypeSafe-Runtime", "attacker".parse().unwrap());
    headers.insert("X-TypeSafe-Retry-Count", "91".parse().unwrap());
    let result = client(&format!("{}///", server.uri()))
        .models()
        .list_with_options(RequestOptions {
            headers,
            ..RequestOptions::default()
        })
        .await
        .unwrap();

    assert_eq!(result.models[0].name, "jev-latest");
    assert_eq!(result.request_id(), Some("req-models"));
    assert_eq!(
        result.raw_http_response.json().unwrap()["models"][0]["future_field"],
        1
    );
    let requests = server.received_requests().await.unwrap();
    let request = &requests[0];
    assert_eq!(
        request.headers["authorization"],
        "Bearer test-api-key-never-log"
    );
    assert_eq!(request.headers["accept"], "application/json");
    assert_eq!(request.headers.get_all("authorization").iter().count(), 1);
    assert_ne!(request.headers["user-agent"], "attacker");
    assert_ne!(request.headers["x-typesafe-sdk"], "attacker");
    assert_ne!(request.headers["x-typesafe-runtime"], "attacker");
    assert!(!request.headers.contains_key("x-typesafe-retry-count"));
    assert!(request.body.is_empty());
}

#[tokio::test]
async fn retries_server_delay_and_sends_incrementing_attempt_headers() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    let attempts = count.clone();
    Mock::given(path("/v1/models"))
        .respond_with(move |_: &wiremock::Request| {
            if attempts.fetch_add(1, Ordering::SeqCst) < 2 {
                ResponseTemplate::new(429)
                    .insert_header("retry-after-ms", "25")
                    .insert_header("retry-after", "5")
                    .set_body_json(json!({"error": "slow down"}))
            } else {
                ResponseTemplate::new(200).set_body_json(models_body())
            }
        })
        .expect(3)
        .mount(&server)
        .await;
    let start = Instant::now();
    client(&server.uri())
        .models()
        .list_with_options(RequestOptions {
            retry: Some(RetryPolicy {
                backoff_initial: Duration::from_secs(3),
                ..RetryPolicy::default()
            }),
            ..RequestOptions::default()
        })
        .await
        .unwrap();
    assert!(start.elapsed() >= Duration::from_millis(45));
    let requests = server.received_requests().await.unwrap();
    assert!(!requests[0].headers.contains_key("x-typesafe-retry-count"));
    assert_eq!(requests[1].headers["x-typesafe-retry-count"], "1");
    assert_eq!(requests[2].headers["x-typesafe-retry-count"], "2");
}

#[tokio::test]
async fn authentication_failure_is_not_retried_and_retains_error_context() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": {"message": "Invalid API key"}}))
                .insert_header("x-typesafe-request-id", "req-denied"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let error = client(&server.uri())
        .models()
        .list_with_options(RequestOptions {
            retry: Some(fast_retry(5)),
            ..RequestOptions::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.request_id(), Some("req-denied"));
    let api = error.as_api_error().unwrap();
    assert_eq!(api.kind(), ApiErrorKind::Authentication);
    assert_eq!(api.status.as_u16(), 401);
    assert_eq!(api.message(), "Invalid API key");
    assert_eq!(api.body["error"]["message"], "Invalid API key");
    assert_eq!(
        api.endpoint,
        Some(format!("GET {}/v1/models", server.uri()))
    );
}

#[tokio::test]
async fn exhausted_retries_keep_the_last_response_and_overrides_stay_local() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    let attempts = count.clone();
    Mock::given(path("/v1/models"))
        .respond_with(move |_: &wiremock::Request| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(503)
                .set_body_string(format!("unavailable {attempt}"))
                .insert_header("x-typesafe-request-id", format!("req-{attempt}"))
        })
        .expect(4)
        .mount(&server)
        .await;
    let client = client(&server.uri());
    let error = client
        .models()
        .list_with_options(RequestOptions {
            retry: Some(fast_retry(2)),
            ..RequestOptions::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.request_id(), Some("req-2"));
    assert_eq!(error.as_api_error().unwrap().body, "unavailable 2");
    let error = client.models().list().await.unwrap_err();
    assert_eq!(error.request_id(), Some("req-3"));
    let requests = server.received_requests().await.unwrap();
    assert!(!requests[3].headers.contains_key("x-typesafe-retry-count"));
}

#[tokio::test]
async fn concurrent_calls_have_independent_retry_counters_and_header_overrides() {
    let server = MockServer::start().await;
    let retried = Arc::new(AtomicUsize::new(0));
    let attempts = retried.clone();
    Mock::given(path("/v1/models"))
        .respond_with(move |request: &wiremock::Request| {
            if request.headers["x-call"] == "retry" && attempts.fetch_add(1, Ordering::SeqCst) == 0
            {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_json(models_body())
            }
        })
        .expect(3)
        .mount(&server)
        .await;
    let client = client(&server.uri());
    let models = client.models();
    let mut retry_headers = HeaderMap::new();
    retry_headers.insert("x-call", "retry".parse().unwrap());
    let mut normal_headers = HeaderMap::new();
    normal_headers.insert("x-call", "normal".parse().unwrap());
    let (retried, normal) = tokio::join!(
        models.list_with_options(RequestOptions {
            headers: retry_headers,
            retry: Some(fast_retry(1)),
            ..RequestOptions::default()
        }),
        models.list_with_options(RequestOptions {
            headers: normal_headers,
            ..RequestOptions::default()
        })
    );
    retried.unwrap();
    normal.unwrap();
    let requests = server.received_requests().await.unwrap();
    let retried: Vec<_> = requests
        .iter()
        .filter(|request| request.headers["x-call"] == "retry")
        .collect();
    assert_eq!(retried.len(), 2);
    assert!(!retried[0].headers.contains_key("x-typesafe-retry-count"));
    assert_eq!(retried[1].headers["x-typesafe-retry-count"], "1");
    let normal = requests
        .iter()
        .find(|request| request.headers["x-call"] == "normal")
        .unwrap();
    assert!(!normal.headers.contains_key("x-typesafe-retry-count"));
}

#[tokio::test]
async fn retry_budget_stops_before_an_excessive_server_wait() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "10"))
        .expect(1)
        .mount(&server)
        .await;
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        client(&server.uri())
            .models()
            .list_with_options(RequestOptions {
                retry: Some(RetryPolicy {
                    timeout: Some(Duration::from_millis(100)),
                    ..RetryPolicy::default()
                }),
                ..RequestOptions::default()
            }),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(
        error.as_api_error().unwrap().kind(),
        ApiErrorKind::RateLimit
    );
    assert_eq!(
        error.as_api_error().unwrap().retry_after(),
        Some(Duration::from_secs(10))
    );
}

#[tokio::test]
async fn cancellation_interrupts_retry_wait_without_another_attempt() {
    let server = MockServer::start().await;
    let attempted = Arc::new(Notify::new());
    let seen = attempted.clone();
    Mock::given(path("/v1/models"))
        .respond_with(move |_: &wiremock::Request| {
            seen.notify_one();
            ResponseTemplate::new(429).insert_header("retry-after", "10")
        })
        .expect(1)
        .mount(&server)
        .await;
    let token = CancellationToken::new();
    let cancel = token.clone();
    let task = tokio::spawn(async move {
        attempted.notified().await;
        tokio::time::sleep(Duration::from_millis(25)).await;
        cancel.cancel();
    });
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        client(&server.uri())
            .models()
            .list_with_options(RequestOptions {
                cancellation_token: Some(token),
                retry: Some(RetryPolicy::default()),
                ..RequestOptions::default()
            }),
    )
    .await
    .unwrap()
    .unwrap_err();
    task.await.unwrap();
    assert!(matches!(error, Error::Cancelled));
}

#[tokio::test]
async fn already_cancelled_request_does_not_reach_the_network() {
    let server = MockServer::start().await;
    let token = CancellationToken::new();
    token.cancel();
    let error = client(&server.uri())
        .models()
        .list_with_options(RequestOptions {
            cancellation_token: Some(token),
            ..RequestOptions::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Cancelled));
    assert!(server.received_requests().await.unwrap().is_empty());
}

// Send valid response headers and only the first body byte. This distinguishes
// whole-response deadlines/cancellation from merely timing out response headers.
async fn stalled_body_server() -> (String, oneshot::Receiver<()>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sent, received) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let length = socket.read(&mut chunk).await.unwrap();
            assert_ne!(length, 0);
            request.extend_from_slice(&chunk[..length]);
        }
        let body = models_body().to_string();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{{", body.len()).as_bytes()).await.unwrap();
        sent.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    (url, received, task)
}

#[tokio::test]
async fn timeout_covers_slow_response_body_delivery() {
    let (url, headers_sent, server_task) = stalled_body_server().await;
    let task = tokio::spawn(async move {
        client(&url)
            .models()
            .list_with_options(RequestOptions {
                timeout: Some(Duration::from_millis(100)),
                ..RequestOptions::default()
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), headers_sent)
        .await
        .unwrap()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    server_task.abort();
    assert!(
        matches!(result, Err(Error::Timeout { timeout, .. }) if timeout == Duration::from_millis(100))
    );
}

#[tokio::test]
async fn interrupted_response_body_is_a_retryable_connection_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server_task = tokio::spawn(async move {
        let mut received = Vec::new();
        for attempt in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let length = socket.read(&mut chunk).await.unwrap();
                assert_ne!(length, 0);
                request.extend_from_slice(&chunk[..length]);
            }
            received.push(String::from_utf8(request).unwrap().to_ascii_lowercase());
            let body = models_body().to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
            if attempt == 0 {
                socket.write_all(b"{").await.unwrap();
            } else {
                socket.write_all(body.as_bytes()).await.unwrap();
            }
            socket.shutdown().await.unwrap();
        }
        received
    });
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        client(&url).models().list_with_options(RequestOptions {
            retry: Some(fast_retry(1)),
            ..RequestOptions::default()
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.models[0].name, "jev-latest");
    let received = server_task.await.unwrap();
    assert!(!received[0].contains("x-typesafe-retry-count:"));
    assert!(received[1].contains("x-typesafe-retry-count: 1\r\n"));
}

#[tokio::test]
async fn cancellation_interrupts_response_body_delivery() {
    let (url, headers_sent, server_task) = stalled_body_server().await;
    let token = CancellationToken::new();
    let cancel = token.clone();
    let task = tokio::spawn(async move {
        client(&url)
            .models()
            .list_with_options(RequestOptions {
                cancellation_token: Some(token),
                ..RequestOptions::default()
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), headers_sent)
        .await
        .unwrap()
        .unwrap();
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    server_task.abort();
    assert!(matches!(result, Err(Error::Cancelled)));
}

#[tokio::test]
async fn injected_http_client_settings_are_used_without_overriding_sdk_auth() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(models_body()))
        .mount(&server)
        .await;
    let mut headers = HeaderMap::new();
    headers.insert("x-custom-transport", "configured".parse().unwrap());
    headers.insert("authorization", "transport-secret".parse().unwrap());
    let http = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    let client = Client::builder()
        .api_key("sdk-secret")
        .base_url(server.uri())
        .http_client(http)
        .build()
        .unwrap();
    client.models().list().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers["x-custom-transport"], "configured");
    assert_eq!(requests[0].headers["authorization"], "Bearer sdk-secret");
    assert!(!format!("{client:?}").contains("sdk-secret"));
    assert!(!format!("{client:?}").contains("transport-secret"));
}

#[cfg(feature = "blocking")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_models_uses_the_same_wire_contract_and_retries() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    let attempts = count.clone();
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(move |_: &wiremock::Request| {
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200)
                    .set_body_json(models_body())
                    .insert_header("x-typesafe-request-id", "req-blocking")
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        typesafe_ai_rs::blocking::Client::builder()
            .api_key("blocking-key")
            .base_url(url)
            .retry(fast_retry(1))
            .build()
            .unwrap()
            .models()
            .list()
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.models[0].name, "jev-latest");
    assert_eq!(result.request_id(), Some("req-blocking"));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers["authorization"], "Bearer blocking-key");
    assert_eq!(requests[1].headers["x-typesafe-retry-count"], "1");
}

#[cfg(feature = "blocking")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_system_one_has_typed_answers_raw_metadata_and_call_overrides() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(system_one_body())
                .insert_header("x-typesafe-request-id", "req-blocking-system"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let url = server.uri();
    let result = tokio::task::spawn_blocking(move || {
        let client = typesafe_ai_rs::blocking::Client::builder()
            .api_key("blocking-key")
            .base_url(url)
            .model("client-model")
            .build()
            .unwrap();
        client.system_one_with_options(
            system_one_request().model("call-model"),
            RequestOptions {
                timeout: Some(Duration::from_secs(1)),
                ..RequestOptions::default()
            },
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.nouls()["billing"].noul, 0.9);
    assert_eq!(result.request_id(), Some("req-blocking-system"));
    assert_eq!(result.raw_http_response.status.as_u16(), 200);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0].body_json::<Value>().unwrap()["model"],
        "call-model"
    );
}
