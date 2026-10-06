//! The one trait every model backend implements.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::types::*;

#[derive(Debug, Error)]
pub enum GenError {
    #[error("No API key set for {0}. Add one in Settings → Models.")]
    MissingKey(String),
    #[error("{provider} rejected the API key ({status}): {message}")]
    Unauthorized { provider: String, status: u16, message: String },
    #[error("{provider} returned {status}: {message}")]
    Http { provider: String, status: u16, message: String },
    #[error("Couldn't reach {provider}: {message}")]
    Network { provider: String, message: String },
    #[error("{0}")]
    Provider(String),
    #[error("This model can't do that: {0}")]
    Unsupported(String),
    #[error("The provider's content filter blocked this request: {0}")]
    Moderated(String),
    #[error("Timed out after {0}s")]
    Timeout(u64),
    #[error("Cancelled")]
    Cancelled,
    #[error("Unexpected response from {provider}: {message}")]
    Decode { provider: String, message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type GenResult<T> = Result<T, GenError>;

/// Runtime context for one provider call.
#[derive(Clone)]
pub struct Ctx {
    pub provider: String,
    pub http: reqwest::Client,
    pub api_key: Option<String>,
    /// Resolved base URL, without trailing slash.
    pub base_url: String,
    /// Free-form provider options from settings (e.g. ComfyUI workflow overrides).
    pub options: Map<String, Value>,
    pub cancel: CancellationToken,
    progress: Arc<dyn Fn(Progress) + Send + Sync>,
}

impl Ctx {
    pub fn new(provider: impl Into<String>, http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            http,
            api_key: None,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            options: Map::new(),
            cancel: CancellationToken::new(),
            progress: Arc::new(|_| {}),
        }
    }

    pub fn with_key(mut self, key: Option<String>) -> Self {
        self.api_key = key.filter(|k| !k.trim().is_empty());
        self
    }

    pub fn with_options(mut self, options: Map<String, Value>) -> Self {
        self.options = options;
        self
    }

    pub fn with_progress(mut self, f: impl Fn(Progress) + Send + Sync + 'static) -> Self {
        self.progress = Arc::new(f);
        self
    }

    pub fn with_cancel(mut self, token: CancellationToken) -> Self {
        self.cancel = token;
        self
    }

    /// The API key, or [`GenError::MissingKey`].
    pub fn key(&self) -> GenResult<&str> {
        self.api_key.as_deref().ok_or_else(|| GenError::MissingKey(self.provider.clone()))
    }

    /// `base_url` + `path` (path should start with `/`).
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    pub fn report(&self, p: Progress) {
        (self.progress)(p)
    }

    pub fn option_str(&self, key: &str) -> Option<&str> {
        self.options.get(key).and_then(Value::as_str)
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn info(&self) -> ProviderInfo;

    /// Models this provider offers. May hit the network (e.g. to list installed
    /// local checkpoints); should fall back to a built-in list when that fails.
    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>>;

    /// Cheap call that proves the key / server works. Returns a short status line.
    async fn check(&self, cx: &Ctx) -> GenResult<String>;

    /// Runs one generation to completion. Long-running jobs should report
    /// progress via [`Ctx::report`]. Cancellation is handled by dropping the
    /// future, so implementations don't need to watch `cx.cancel` unless they
    /// want to tell the remote service to stop.
    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput>;

    /// The voices a speech model reads with (models with [`ModelInfo::voices`]). May hit the
    /// network; providers without speech have none.
    async fn voices(&self, _cx: &Ctx, _model: &str) -> GenResult<Vec<Voice>> {
        Ok(vec![])
    }
}
