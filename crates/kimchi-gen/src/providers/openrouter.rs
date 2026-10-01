//! OpenRouter: One key for image models from every lab.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "openrouter";

pub struct OpenRouter;

#[async_trait]
impl Provider for OpenRouter {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "OpenRouter".into(),
            kind: ProviderKind::Cloud,
            tagline: "One key for image models from every lab".into(),
            website: "https://openrouter.ai".into(),
            needs_key: true,
            key_env: vec!["OPENROUTER_API_KEY".into()],
            key_url: Some("https://openrouter.ai/settings/keys".into()),
            key_hint: Some("sk-or-v1-…".into()),
            default_base_url: "https://openrouter.ai/api/v1".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage],
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
