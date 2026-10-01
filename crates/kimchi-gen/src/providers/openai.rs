//! OpenAI: GPT Image and Sora.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "openai";

pub struct OpenAi;

#[async_trait]
impl Provider for OpenAi {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "OpenAI".into(),
            kind: ProviderKind::Cloud,
            tagline: "GPT Image and Sora".into(),
            website: "https://platform.openai.com".into(),
            needs_key: true,
            key_env: vec!["OPENAI_API_KEY".into()],
            key_url: Some("https://platform.openai.com/api-keys".into()),
            key_hint: Some("sk-…".into()),
            default_base_url: "https://api.openai.com/v1".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo],
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
