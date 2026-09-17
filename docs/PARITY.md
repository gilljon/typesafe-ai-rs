# SDK parity reference

This document records the upstream behavior used to design the Rust client. It is
a source audit, not a claim that every upstream implementation detail is identical
or that a live API test has passed. The Python and JavaScript SDKs differ in several
observable behaviors; those differences are listed explicitly below.

## Audited versions

| SDK | Package version | Source revision |
| --- | --- | --- |
| Python `typesafe-sdk` | `0.6.0` | [`420ef4ffb612d5a539a1e0f0fe883ff6770340af`](https://github.com/typesafe-ai/typesafe-sdk-python/tree/420ef4ffb612d5a539a1e0f0fe883ff6770340af) |
| JavaScript `@typesafe-ai/sdk` | `0.6.0` | [`66880ccded6cb642dc1809620c2b108c33730214`](https://github.com/typesafe-ai/typesafe-sdk-js/tree/66880ccded6cb642dc1809620c2b108c33730214) |

The source code and upstream test suites, rather than inferred API behavior, are
the reference. Relevant public documentation:
[Python](https://docs.typesafe.ai/sdk/python),
[JavaScript](https://docs.typesafe.ai/sdk/javascript).

## Shared API surface

| Capability | Upstream contract |
| --- | --- |
| System One | `POST /v1/systemone` with `state`, `questions`, and resolved `model`. |
| Models | `GET /v1/models`, whose wire body is `{ "models": [...] }`. There are no pagination, streaming, file upload, or model-retrieve resources in the audited SDKs. |
| Noul questions | `type: "noul"`, optional instructions, optional `true`/`false` criterion descriptions. The answer contains a numeric `noul`. |
| Choice questions | `type: "choice"`, optional instructions, labels mapped to criterion descriptions. The answer contains `choice`, `confidence`, and label-keyed `probabilities`. |
| Score questions | `type: "score"`, optional instructions, an ordered rubric. The answer contains a possibly fractional `score`, `confidence`, `legend`, and score-keyed `probabilities`. |
| Structured input | Text, JSON objects, and JSON arrays are accepted as state and descriptions. Explicit nullable fields and nested JSON must survive serialization. |
| Response metadata | Model name, named answers, token usage, response status/headers/body, and the `x-typesafe-request-id` header. |
| Model metadata | `name`, `description`, and `release_date` strings. Unknown server fields must not break parsing. |
| Overrides | Client and per-request timeout, retry policy, custom headers, model selection, and extra request-body fields. |
| Transport configuration | Python accepts a custom HTTP client or transport; JavaScript accepts a custom `fetch`. |

Python additionally supplies synchronous and asynchronous clients and grouped
`nouls`, `choices`, and `scores` response views. JavaScript supplies promise methods
for raw responses and parsed data plus response metadata. Equivalent Rust
capabilities need not reproduce Python context managers or JavaScript promise
inheritance.

## Configuration and HTTP behavior

- Explicit configuration takes precedence over environment variables and then SDK
  defaults. Environment values are trimmed; blank environment values are unset.
- Environment variables are `TYPESAFE_API_KEY`, `TYPESAFE_BASE_URL`,
  `TYPESAFE_DEFAULT_MODEL`, and `TYPESAFE_LOG_LEVEL`.
- The default API root is `https://api.typesafe.ai`; the default model is
  `jev-latest`. Trailing root slashes are removed before endpoint paths are appended.
- An API key is required. Timeout values must be positive and finite. The default
  timeout is ten seconds, with the timeout interpretation difference noted below.
- Additional headers merge case-insensitively, with request headers overriding
  client defaults. Authentication and SDK identification headers remain protected.
- Both endpoints send `Authorization: Bearer ...`, `Accept: application/json`,
  `User-Agent: typesafe-sdk/<version>`, `X-TypeSafe-SDK`, and `X-TypeSafe-Runtime`.
  Requests with JSON bodies send `Content-Type: application/json`.
- `X-TypeSafe-Retry-Count` is absent on the initial attempt and is `1`, `2`, etc. on
  retries. Caller-supplied values must not impersonate the SDK attempt count.
- JSON errors are decoded regardless of content type. Invalid JSON errors remain
  text; empty errors retain an empty-body representation.
- All non-2xx statuses are errors, including through raw-response access.
- Requests and retry state must be independent under concurrent calls. Per-call
  overrides must not mutate client defaults or caller-owned input.

The JavaScript client removes caller `Content-Type` on a bodyless GET. Python
retains a supplied GET content type. Rust currently follows Python here.

## Retry and cancellation

Shared defaults are two retries after the initial attempt; retryable HTTP statuses
are 408, 429, and 500–599. Connection failures and timeouts are retryable and can be
disabled independently. The default backoff starts at 500 ms, doubles to a 5 s cap,
and subtracts up to 25% random jitter. Zero backoff is supported.

`retry-after-ms` takes precedence over `Retry-After`. Both accept nonnegative
numeric delays; `Retry-After` also accepts an HTTP date. Past dates yield zero
delay, invalid headers fall back to backoff, and server delays are not jittered.
The final exhausted error must retain the last response and its request ID.

Caller cancellation must interrupt both network/body delivery and retry waits
without starting another attempt. JavaScript exposes an abort signal and distinct
abort error; Python propagates task cancellation. Dropping a Rust async future is
an idiomatic cancellation mechanism, with an explicit cancellation token useful
when the caller needs a cancellation result.

| Behavior | Python 0.6.0 | JavaScript 0.6.0 |
| --- | --- | --- |
| Timeout scope | HTTP operation timeout; custom HTTP timeout configuration supported. | Whole attempt, including response-body delivery; fresh timeout on each retry. |
| Total retry budget | 30 s by default, optional. Stops before a retry whose next delay would reach/exceed the budget; does not forcibly interrupt the initial attempt. | No total budget. |
| Maximum server delay | No independent maximum. A configured total budget can prevent the retry. | 60 s by default; a longer server delay falls back to exponential backoff. Configurable. |
| Retry extensions | Extra exception classes and a predicate add retryable failures. | Configurable statuses and connection/timeout flags. |
| Per-request retry override | Replaces the complete policy. | Partially overrides individual client policy fields. |

No single set of defaults can duplicate both SDKs. A Rust implementation should
state its selected defaults and expose the other behavior through configuration.
Rust error predicates can represent Python's additional exception matching without
introducing a foreign-language exception hierarchy.

## Errors and response parsing

HTTP errors distinguish 400 bad request, 401 authentication, 403 permission denied,
404 not found, 422 unprocessable entity, 429 rate limit, and server failures.
Other statuses retain a generic API error. Every HTTP error exposes status,
headers, decoded/text body, and optional request ID. Rate-limit errors also expose
the parsed server delay. Connection, timeout, configuration, caller cancellation,
and invalid-success-response errors need distinguishable Rust representations.

Error messages prefer, in order, a text body, `error` string, `error.message`,
`message`, `detail` string, `detail.message`, or validation entries in `detail`.
Validation entries combine `loc` (excluding `body`) and `msg`. Fallback JSON/text
is truncated to 200 characters in the displayed message, while the full body
remains accessible. Python includes a sanitized endpoint and request ID in its
displayed error; endpoint diagnostics must omit credentials, query, and fragment.

Python validates required success fields and reports an invalid field path.
`model`, `answers`, and the `usage` object are required; individual token counts
may be absent or null. Python ignores unknown response fields and skips unknown
answer types with a warning, keeping the original raw body available. JavaScript
trusts System One's declared response type and only explicitly validates the
models wrapper. Rust should validate known answer variants and tolerate future
fields; preserving a future answer variant as raw JSON is a useful, documented
idiomatic alternative to discarding it.

## Other upstream differences

| Capability | Python 0.6.0 | JavaScript 0.6.0 |
| --- | --- | --- |
| Score rubric validation | Requires at least one entry. Raw-question schema checks are intentionally minimal. | Requires an array with at least two entries before sending. |
| Raw questions | Allows future nonempty question type strings and extra JSON fields. | Runtime forwarding works, but the public TypeScript union describes the three known types. |
| Null state | Excluded by Python's public input type, though the encoder can represent it. | Explicitly supported by the public input type. |
| Extra body fields | Shallow-merged after `state`, `model`, and `questions`; collisions replace those fields, including with null. | Extra request properties are forwarded, then the default model is filled when absent/null. |
| Score map keys | Decoded as integers. | JSON string keys, with TypeScript inference from the rubric. |
| Missing request ID | Accessing the response property raises an SDK error. | An absent optional value. |
| Resource lifetime | Explicit close/context-manager closes supplied HTTP clients too. | Managed by the runtime/fetch implementation. |
| Browser behavior | Not a browser SDK. | Browser use rejected unless explicitly allowed. |

Rust ownership and `Drop`, `Result`/error enums, `Duration`, serde input/response
types, optional request IDs, and maps/iterators are appropriate language-specific
adaptations. TypeScript's compile-time inference of arbitrary question names and
rubric keys is not a runtime API requirement; callers should still be able to
deserialize known answer structures safely.

## Logging

Both upstream clients provide request summaries at info and bodies/headers at
debug. Bodies are not redacted, so debug logging can include application data.
API keys must not appear in client debug representations or unredacted header
logs. JavaScript redacts authorization, proxy authorization, API-key, and cookie
headers, preserving a suffix on long keys. Python redacts those credentials in
full and additionally protects `api-key` and header names containing `token` or
`secret`. Full redaction is the preferable Rust behavior.

Python integrates with standard logging and accepts `warning` as well as `warn`;
unrecognized environment levels are ignored. JavaScript has a configurable logger,
defaults to `warn`, and rejects unrecognized levels. A Rust logging facade can
provide equivalent integration without configuring a process-global logger.

## Rust choices

The Rust client combines the public capabilities with these explicit choices
where the upstream implementations differ:

| Area | Rust behavior |
| --- | --- |
| Client APIs | Async `Client` and optional `blocking::Client`; resources close through ownership and `Drop`. |
| Retry defaults | Two retries, a 30 s optional retry budget, and a configurable 60 s maximum server delay. Set `timeout: None` for JavaScript's lack of a total budget; set `max_retry_after: None` for Python's uncapped server hints. |
| Per-call retry | A complete policy replacement, as in Python. Rust struct update syntax makes inherited fields explicit. |
| Timeout | Ten seconds for the complete request and response-body delivery, as in JavaScript. Supplied reqwest client settings remain available; SDK request timeout settings are authoritative. |
| Cancellation | Explicit `CancellationToken` for async calls and native future cancellation. Blocking calls reject cancellation tokens. |
| Input validation | Nonempty score arrays, as in Python. Raw choice criteria must be a map and raw score criteria an array; malformed shapes are rejected locally rather than forwarded. |
| JSON extension | `Question::Raw` supports future question types and fields. `extra_body` shallow-merges after standard fields, including null and collisions, as in Python. |
| Response parsing | Required known fields are validated; optional usage counts accept missing/null; unknown answer variants are skipped and remain available in the original raw response. |
| Score keys | Integer-keyed score maps, as in Python. |
| HTTP metadata | Response structs retain status, headers, raw bytes, JSON/text access, and an optional request ID. Validation errors also retain the raw response. |
| SDK identification | `typesafe-ai-rs/<crate version>` identifies the Rust client; runtime headers identify Rust and the target OS/architecture. |
| Errors | `Error` and `ApiErrorKind` provide Rust error matching. Formatting includes status, sanitized endpoint, and request ID; `ApiError::message()` explicitly retrieves server details. Bodies and arbitrary headers are omitted from automatic error formatting. |
| Logging | The standard `log` facade with client-level filtering; the library does not install a process-global logger. Credential headers are fully redacted. |
| Configuration | Invalid API roots and blank explicit API keys are rejected early; roots cannot contain credentials, query strings, or fragments. |
| Browser support | This crate targets native Rust with reqwest/Tokio. JavaScript browser opt-in and JavaScript runtime detection are not Rust APIs. |

These choices preserve access to the API's capabilities without claiming identical
language-specific constructors, exceptions, promises, or compile-time inference.

When supplying a custom reqwest client, leave `X-TypeSafe-Retry-Count` out of that
client's own `default_headers`: reqwest inserts absent default headers during
execution, after the SDK has prepared the initial request. The SDK protects
headers supplied through its own client/request options; a caller-controlled
transport can still alter the final request.

## Verification scope

The test suite checks parity with local HTTP-server tests covering request payloads,
protected headers, both endpoints, all three answer types, raw metadata, errors,
retry controls and server delays, cancellation, body-read timeouts, and concurrent
requests. Response fixtures should include optional usage, unknown fields/types,
malformed known fields, and extra-body null values. Blocking and async examples,
documentation builds, and packaged-crate builds are separate release checks.

The initial aggregate verification passed 22 unit tests, 4 configuration tests,
19 HTTP integration tests, and 5 documentation tests with all features enabled.
An additional isolated logging test passed, verifying client filtering and request/response
credential redaction. Clippy, warning-free documentation, a packaged-crate build,
no-default-feature tests, and Rust 1.88 compatibility checks also passed.
The HTTP suite includes both blocking endpoints, concurrent retries, interrupted
response-body recovery, full-body timeouts, and cancellation before dispatch,
during backoff, and during body delivery. These tests use local fixtures and TCP
servers; no API key was available for a live service test.

Live API compatibility and registry publication are separate checks. Neither is
implied by this source audit or by local mock-server tests.
