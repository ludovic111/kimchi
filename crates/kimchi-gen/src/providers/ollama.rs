//! Ollama: Local image models with one command.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "ollama";

pub struct Ollama;

#[async_trait]
impl Provider for Ollama {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Ollama".into(),
            kind: ProviderKind::Local,
            tagline: "Local image models with one command".into(),
            website: "https://ollama.com".into(),
            needs_key: false,
            key_env: vec![],
            key_url: None,
            key_hint: None,
            default_base_url: "http://127.0.0.1:11434".into(),
            base_url_editable: true,
            tasks: vec![Task::TextToImage],
        }
    }

    async fn models(&self, _cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        Ok(vec![])
    }

    async fn check(&self, _cx: &Ctx) -> GenResult<String> {
        Err(GenError::Unsupported("not implemented yet".into()))
    }

    async fn generate(&self, _cx: &Ctx, _req: &GenRequest) -> GenResult<GenOutput> {
        Err(GenError::Unsupported("not implemented yet".into()))
    }
}
