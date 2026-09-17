#![doc = include_str!("../README.md")]

mod client;
mod config;
mod error;
mod retry;
mod transport;
mod types;

#[cfg(feature = "blocking")]
pub mod blocking;

pub use client::{Client, ClientBuilder, Models};
pub use config::{LogLevel, RequestOptions};
pub use error::{ApiError, ApiErrorKind, Error};
pub use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
pub use reqwest::StatusCode;
pub use retry::{parse_retry_after, RetryPolicy, RetryPredicate};
pub use tokio_util::sync::CancellationToken;
pub use types::*;

/// Python-compatible name for the asynchronous client.
pub type AsyncTypeSafeClient = Client;
/// Python-compatible name for the synchronous client.
#[cfg(feature = "blocking")]
pub type TypeSafeClient = blocking::Client;

/// Default API root (without the versioned endpoint path).
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
/// Default model alias.
pub const DEFAULT_MODEL: &str = "jev-latest";
/// Timeout for each complete HTTP attempt, including its response body.
pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// This crate's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
