//! xAI: Grok Imagine images and video.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "xai";

pub struct Xai;

#[async_trait]
impl Provider for Xai {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "xAI".into(),
            kind: ProviderKind::Cloud,
            tagline: "Grok Imagine images and video".into(),
            website: "https://x.ai/api".into(),
            needs_key: true,
            key_env: vec!["XAI_API_KEY".into()],
            key_url: Some("https://console.x.ai".into()),
            key_hint: Some("xai-…".into()),
            default_base_url: "https://api.x.ai/v1".into(),
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
