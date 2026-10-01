//! Together AI: Fast open models: FLUX, Qwen Image and more.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "together";

pub struct Together;

#[async_trait]
impl Provider for Together {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Together AI".into(),
            kind: ProviderKind::Cloud,
            tagline: "Fast open models: FLUX, Qwen Image and more".into(),
            website: "https://together.ai".into(),
            needs_key: true,
            key_env: vec!["TOGETHER_API_KEY".into()],
            key_url: Some("https://api.together.ai/settings/api-keys".into()),
            key_hint: None,
            default_base_url: "https://api.together.xyz/v1".into(),
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
