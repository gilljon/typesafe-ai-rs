# TypeSafe Rust SDK

An independent Rust client for the [TypeSafe AI System One API](https://docs.typesafe.ai/), maintained at [gilljon/typesafe-ai-rs](https://github.com/gilljon/typesafe-ai-rs).

Async and blocking clients, typed Noul/Choice/Score questions and answers, model discovery, configurable retries, cancellation, and complete HTTP response metadata. The implementation targets the official [Python](https://github.com/typesafe-ai/typesafe-sdk-python) and [JavaScript](https://github.com/typesafe-ai/typesafe-sdk-js) SDKs at version **0.6.0**. See [the parity notes](docs/PARITY.md) for exact source revisions and deliberate Rust differences.

## Install

```sh
cargo add typesafe-ai-rs
cargo add tokio --features macros,rt-multi-thread
cargo add serde_json
```

The package is named `typesafe-ai-rs`, imported as `typesafe_ai_rs`.

Alternatively, install directly from the GitHub release:

```sh
cargo add typesafe-ai-rs --git https://github.com/gilljon/typesafe-ai-rs --tag v0.1.0
```

Rust 1.88 or newer is required. The default TLS backend is Rustls. To use native TLS, disable default features and enable `native-tls`. Enable `blocking` for the synchronous client.

## Quick start

Set `TYPESAFE_API_KEY` to an API key from the [TypeSafe console](https://console.typesafe.ai/).

```rust,no_run
use serde_json::json;
use typesafe_ai_rs::{Client, Choice, Noul, Question, Score, SystemOneRequest};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new()?;
    let response = client.system_one(SystemOneRequest::new(
        json!({"document": "I was charged twice. Please fix this ASAP."}),
        [
            ("billing", Question::from(Noul::new("Is this about billing?"))),
            ("tone", Choice::new([("calm", json!(null)), ("frustrated", json!(null)), ("angry", json!(null))])
                .instructions("What is the customer's tone?").into()),
            ("urgency", Score::new(["can wait", "this week", "today"])
                .instructions("How urgent is this?").into()),
        ],
    )).await?;

    println!("billing: {}", response.nouls()["billing"].noul);
    println!("tone: {}", response.choices()["tone"].choice);
    println!("urgency: {}", response.scores()["urgency"].score);
    println!("request ID: {:?}", response.request_id());
    Ok(())
}
```

Questions accept JSON instructions and descriptions. Omitted instructions and explicit JSON `null` remain distinct. Score criteria are an ordered, nonempty sequence, sent as an array on the wire. Score answers expose integer keys for their probabilities and legend. A one-level score rubric follows Python's validation; JavaScript requires two levels.

## Blocking client

```sh
cargo add typesafe-ai-rs --features blocking
```

```rust,no_run
# #[cfg(feature = "blocking")]
use typesafe_ai_rs::{blocking::Client, Noul, SystemOneRequest};

# #[cfg(feature = "blocking")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let response = Client::new()?.system_one(SystemOneRequest::new(
        "I was charged twice.",
        [("billing", Noul::new("Is this about billing?"))],
    ))?;
    println!("{}", response.nouls()["billing"].noul);
    Ok(())
}
# #[cfg(not(feature = "blocking"))]
# fn main() {}
```

Use the blocking client outside async runtimes, or in `tokio::task::spawn_blocking`. Clones share connection pools; dropping the last client releases its resources. `AsyncTypeSafeClient` and, with the feature enabled, `TypeSafeClient` are aliases matching Python's naming.

## Configuration and per-call options

Explicit configuration takes precedence over environment variables. Empty environment values are ignored.

| Environment variable | Default |
| --- | --- |
| `TYPESAFE_API_KEY` | Required |
| `TYPESAFE_BASE_URL` | `https://api.typesafe.ai` |
| `TYPESAFE_DEFAULT_MODEL` | `jev-latest` |
| `TYPESAFE_LOG_LEVEL` | `warn` |

```rust,no_run
# async fn example() -> Result<(), typesafe_ai_rs::Error> {
use std::time::Duration;
use typesafe_ai_rs::{Client, Noul, RequestOptions, RetryPolicy, SystemOneRequest};

let client = Client::builder()
    .api_key("your-api-key")
    .model("jev-latest")
    .timeout(Duration::from_secs(10))
    .retry(RetryPolicy { max_retries: 3, ..Default::default() })
    .build()?;

let result = client.system_one_with_options(
    SystemOneRequest::new("A support ticket", [("billing", Noul::new("About billing?"))])
        .model("jev-latest"),
    RequestOptions {
        timeout: Some(Duration::from_secs(5)),
        retry: Some(RetryPolicy::no_retries()),
        ..Default::default()
    },
).await?;
# Ok(()) }
```

`default_headers` and per-call `headers` use `HeaderMap`. Per-call headers override defaults case-insensitively, except protected authentication and SDK protocol headers. A custom `reqwest::Client` (or `reqwest::blocking::Client`) can be supplied through `.http_client(...)` for proxy, TLS, and pool configuration. SDK request timeouts still apply. The default HTTP client does not follow redirects; custom clients control their own redirect behavior.

The default policy makes up to two retries for HTTP 408, 429, 5xx, connection errors, and timeouts. Exponential backoff starts at 500ms, caps at 5s, and subtracts up to 25% jitter. Server retry headers accept milliseconds, seconds, and HTTP dates. A 30s retry budget follows Python; the 60s retry-header cap follows JavaScript. The budget prevents scheduling another retry but does not interrupt an already-running attempt. Set `timeout: None` in `RetryPolicy` for JavaScript's unlimited total budget, or `max_retry_after: None` for Python's uncapped server delays. A per-call retry policy replaces the client's policy. Use struct update syntax to inherit selected settings. Custom HTTP statuses and an additional retry predicate are supported.

## Models and metadata

```rust,no_run
# async fn example(client: &typesafe_ai_rs::Client) -> Result<(), typesafe_ai_rs::Error> {
let response = client.models().list().await?;
for model in &response.models {
    println!("{}: {}", model.name, model.description);
}
println!("request ID: {:?}", response.request_id());
println!("HTTP status: {}", response.raw_http_response.status);
# Ok(()) }
```

Responses retain status, headers, and body bytes in `raw_http_response`; `.json()` and `.text()` inspect the full body. Required response fields are validated, unknown fields are ignored, and unknown answer kinds are skipped from typed answers while remaining available in the raw body.

## Errors and cancellation

Errors distinguish invalid configuration/requests, transport failures, timeout, cancellation, server errors, and malformed successful responses. API errors expose an `ApiErrorKind`, status, request ID, headers, body, and extracted server message. `Display` and `Debug` omit response bodies and secret headers; call `.message()` explicitly for server details.

```rust,no_run
# async fn example(client: &typesafe_ai_rs::Client) {
use typesafe_ai_rs::{ApiErrorKind, Error};
match client.models().list().await {
    Ok(response) => println!("{} models", response.models.len()),
    Err(Error::Api(error)) if error.kind() == ApiErrorKind::RateLimit => {
        eprintln!("retry after {:?}; request {:?}", error.retry_after(), error.request_id());
    }
    Err(error) => eprintln!("{error}"),
}
# }
```

Pass a cloned `CancellationToken` in `RequestOptions::cancellation_token` and call `token.cancel()` from another task to cancel an async request or retry wait. Dropping an in-progress request future also cancels it. Cancellation is never retried. Blocking requests use timeouts and reject cancellation tokens.

## Forward compatibility and logging

Use `Question::Raw(json!(...))` for future question types/fields, and `.extra_body([("beam_width", json!(4))])` on a request for additional top-level fields. Extra body fields merge last and can override `state`, `model`, or `questions`, matching Python.

The SDK uses the `log` facade with target `typesafe_ai_rs`; install an application logger such as `env_logger` to receive output. `TYPESAFE_LOG_LEVEL` or `.log_level(...)` filters transport messages. `info` emits request summaries; `debug` adds bodies and redacted headers. Authorization, cookies, API keys, and headers containing `token` or `secret` are redacted. Bodies can contain application data and are deliberately not redacted at debug level.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo test --no-default-features
cargo doc --all-features --no-deps
cargo package
```

Tests use local HTTP servers and need no TypeSafe API key. Runnable examples live in `examples/`. Live examples require your key and may consume API credits. See [release instructions](docs/RELEASING.md) for registry publication.

MIT licensed. This project is independently maintained and is not an official TypeSafe SDK.
