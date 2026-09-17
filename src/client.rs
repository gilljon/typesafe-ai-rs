use crate::{
    config::{config_methods, Config, ConfigBuilder},
    transport, Error, ListModelsResponse, RawResponse, RequestOptions, SystemOneRequest,
    SystemOneResponse,
};
use reqwest::Method;
use serde_json::Value;
use std::time::Instant;

/// Asynchronous TypeSafe client. Clone it to share a pooled HTTP connection manager.
#[derive(Clone)]
pub struct Client {
    config: Config,
    http: reqwest::Client,
}

/// Construct an asynchronous client with environment fallbacks.
#[derive(Default)]
pub struct ClientBuilder {
    config: ConfigBuilder,
    http: Option<reqwest::Client>,
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
    /// Use an existing HTTP client for custom proxy, TLS, and connection settings.
    /// SDK request timeouts still apply. Configure its redirect policy appropriately.
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http = Some(client);
        self
    }
    /// Resolve configuration and build the client without making a network request.
    pub fn build(self) -> Result<Client, Error> {
        let config = self.config.resolve()?;
        let http = match self.http {
            Some(http) => http,
            None => reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(Error::Connection)?,
        };
        Ok(Client { config, http })
    }
}

impl Client {
    /// Create a client using `TYPESAFE_*` environment variables.
    pub fn new() -> Result<Self, Error> {
        Self::builder().build()
    }
    /// Configure a client explicitly.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }
    /// The resolved default model.
    pub fn default_model(&self) -> &str {
        &self.config.model
    }
    /// The resolved API root.
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }
    /// Evaluate named questions about text or structured JSON state.
    pub async fn system_one(&self, request: SystemOneRequest) -> Result<SystemOneResponse, Error> {
        self.system_one_with_options(request, RequestOptions::default())
            .await
    }
    /// Evaluate questions with per-call timeout, retry, headers, or cancellation overrides.
    pub async fn system_one_with_options(
        &self,
        request: SystemOneRequest,
        options: RequestOptions,
    ) -> Result<SystemOneResponse, Error> {
        let body = request.prepare(&self.config.model)?;
        self.send(Method::POST, "/v1/systemone", Some(body), options, |raw| {
            SystemOneResponse::from_raw_with_log_level(raw, self.config.log_level)
        })
        .await
    }
    /// Access model discovery.
    pub fn models(&self) -> Models<'_> {
        Models(self)
    }

    async fn send<T>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        options: RequestOptions,
        decode: impl Fn(RawResponse) -> Result<T, Error>,
    ) -> Result<T, Error> {
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
            let operation = async {
                let response = builder.send().await.map_err(|e| request.map_error(e))?;
                let status = response.status();
                let headers = response.headers().clone();
                let body = response.bytes().await.map_err(|e| request.map_error(e))?;
                let raw = RawResponse {
                    status,
                    headers,
                    body,
                };
                request.log_response(&self.config, &raw, attempt_started);
                request.finish(raw, &decode)
            };
            let result = tokio::select! {
                biased;
                _ = cancelled(&options) => return Err(Error::Cancelled),
                result = operation => result,
            };
            match result {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let Some(delay) = request.retry_delay(&error, attempt, started) else {
                        return Err(error);
                    };
                    if self.config.log_level >= log::LevelFilter::Info {
                        log::info!(target: "typesafe_ai_rs", "retry={} delay_ms={}", attempt + 1, delay.as_millis());
                    }
                    tokio::select! {
                        biased;
                        _ = cancelled(&options) => return Err(Error::Cancelled),
                        _ = tokio::time::sleep(delay) => {},
                    }
                    attempt += 1;
                }
            }
        }
    }
}

async fn cancelled(options: &RequestOptions) {
    match &options.cancellation_token {
        Some(token) => token.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

/// Models resource associated with an asynchronous client.
#[derive(Clone, Copy, Debug)]
pub struct Models<'a>(&'a Client);
impl Models<'_> {
    /// List available model cards with response metadata.
    pub async fn list(&self) -> Result<ListModelsResponse, Error> {
        self.list_with_options(RequestOptions::default()).await
    }
    /// List models with per-call request overrides.
    pub async fn list_with_options(
        &self,
        options: RequestOptions,
    ) -> Result<ListModelsResponse, Error> {
        self.0
            .send(
                Method::GET,
                "/v1/models",
                None,
                options,
                ListModelsResponse::from_raw,
            )
            .await
    }
}
