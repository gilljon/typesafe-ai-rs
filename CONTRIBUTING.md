# Contributing

Bug reports and focused pull requests are welcome. This is an independently
maintained Rust client for TypeSafe AI; service and account issues belong with
TypeSafe support.

## Reporting a bug

[Open an issue](https://github.com/gilljon/typesafe-ai-rs/issues/new/choose) with
the crate version, Rust version, enabled features, and a minimal reproduction.
Include the expected behavior and the error or HTTP status you received. Remove
API keys, authorization headers, and private request or response data.

Report security vulnerabilities privately using the [security policy](SECURITY.md).

## Working on the SDK

Use Rust 1.88 or newer. Clone the repository and run:

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo test --locked --no-default-features
cargo doc --locked --all-features --no-deps
```

Tests use local HTTP servers and do not need an API key. Examples require
`TYPESAFE_API_KEY` and make real requests that may consume API credits.
CI also checks Linux, macOS, Windows, Rust 1.88, and the packaged crate.

Keep changes focused. Add a regression test for a behavior change, update the
relevant documentation, and describe what changed and how you verified it in the
pull request. Run `cargo package --locked` from a clean checkout before a release.

The [parity notes](docs/PARITY.md) record the upstream Python and JavaScript
versions and their differences. Check those sources before changing wire formats,
validation, retries, or error behavior. If a deliberate Rust difference changes,
update the notes alongside the code.

See [Releasing](docs/RELEASING.md) for the publication process.
