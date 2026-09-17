//! Synchronous client, enabled by the `blocking` Cargo feature.
//!
//! Use outside async runtimes, or inside `tokio::task::spawn_blocking`.
use crate::{
    config::{config_methods, Config, ConfigBuilder},
    transport, Error, ListModelsResponse, RawResponse, RequestOptions, SystemOneRequest,
    SystemOneResponse,
};
use reqwest::Method;
use serde_json::Value;
use std::time::Instant;

/// Synchronous TypeSafe client with pooled HTTP connections.
#[derive(Clone)]
pub struct Client {
    config: Config,
    http: reqwest::blocking::Client,
}

/// Configure a blocking client using the same settings as the async client.
#[derive(Default)]
pub struct ClientBuilder {
    config: ConfigBuilder,
    http: Option<reqwest::blocking::Client>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientBuilder").finish_non_exhaustive()
    }
}

impl ClientBuilder {
    config_methods!();
    /// Supply an HTTP client configured with custom proxy, TLS, or pool settings.
    pub fn http_client(mut self, value: reqwest::blocking::Client) -> Self {
        self.http = Some(value);
        self
    }
    /// Build a client. Call outside an async runtime.
    pub fn build(self) -> Result<Client, Error> {
        let config = self.config.resolve()?;
        let http = match self.http {
            Some(http) => http,
            None => reqwest::blocking::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(Error::Connection)?,
        };
        Ok(Client { config, http })
    }
}
impl Client {
    /// Construct from `TYPESAFE_*` environment variables.
    pub fn new() -> Result<Self, Error> {
        Self::builder().build()
    }
    /// Configure a blocking client.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }
    /// Resolved default model.
    pub fn default_model(&self) -> &str {
        &self.config.model
    }
    /// Resolved API root.
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }
    /// Evaluate named questions about text or structured state.
    pub fn system_one(&self, request: SystemOneRequest) -> Result<SystemOneResponse, Error> {
        self.system_one_with_options(request, RequestOptions::default())
    }
    /// Evaluate questions with per-call overrides.
    pub fn system_one_with_options(
        &self,
        request: SystemOneRequest,
        options: RequestOptions,
    ) -> Result<SystemOneResponse, Error> {
        let body = request.prepare(&self.config.model)?;
        self.send(Method::POST, "/v1/systemone", Some(body), options, |raw| {
            SystemOneResponse::from_raw_with_log_level(raw, self.config.log_level)
        })
    }
    /// Access model discovery.
    pub fn models(&self) -> Models<'_> {
        Models(self)
    }

    fn send<T>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        options: RequestOptions,
        decode: impl Fn(RawResponse) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if options.cancellation_token.is_some() {
            return Err(Error::Configuration(
                "Cancellation requires the async client".into(),
            ));
        }
        let request = transport::prepare(&self.config, method, path, body, &options)?;
        let started = Instant::now();
        let mut attempt = 0;
        loop {
            let headers = request.headers(attempt);
            let attempt_started = Instant::now();
            request.log_request(&self.config, &headers, attempt);
            let mut builder = self
                .http
                .request(request.method.clone(), &request.url)
                .headers(headers)
                .timeout(request.timeout);
            if let Some(body) = &request.body {
                builder = builder.body(body.clone());
            }
            let result = (|| {
                let response = builder.send().map_err(|e| request.map_error(e))?;
                let status = response.status();
                let headers = response.headers().clone();
                let body = response.bytes().map_err(|e| request.map_error(e))?;
                let raw = RawResponse {
                    status,
                    headers,
                    body,
                };
                request.log_response(&self.config, &raw, attempt_started);
                request.finish(raw, &decode)
            })();
            match result {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let Some(delay) = request.retry_delay(&error, attempt, started) else {
                        return Err(error);
                    };
                    if self.config.log_level >= log::LevelFilter::Info {
                        log::info!(target: "typesafe_ai_rs", "retry={} delay_ms={}", attempt + 1, delay.as_millis());
                    }
                    std::thread::sleep(delay);
                    attempt += 1;
                }
            }
        }
    }
}

/// Synchronous model discovery resource.
#[derive(Clone, Copy, Debug)]
pub struct Models<'a>(&'a Client);
impl Models<'_> {
    /// List model cards and response metadata.
    pub fn list(&self) -> Result<ListModelsResponse, Error> {
        self.list_with_options(RequestOptions::default())
    }
    /// List models with per-call overrides.
    pub fn list_with_options(&self, options: RequestOptions) -> Result<ListModelsResponse, Error> {
        self.0.send(
            Method::GET,
            "/v1/models",
            None,
            options,
            ListModelsResponse::from_raw,
        )
    }
}
