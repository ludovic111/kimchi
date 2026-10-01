//! Stability AI: Stable Diffusion 3.5 and Stable Image.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "stability";

pub struct Stability;

#[async_trait]
impl Provider for Stability {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Stability AI".into(),
            kind: ProviderKind::Cloud,
            tagline: "Stable Diffusion 3.5 and Stable Image".into(),
            website: "https://platform.stability.ai".into(),
            needs_key: true,
            key_env: vec!["STABILITY_API_KEY".into()],
            key_url: Some("https://platform.stability.ai/account/keys".into()),
            key_hint: Some("sk-…".into()),
            default_base_url: "https://api.stability.ai".into(),
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
