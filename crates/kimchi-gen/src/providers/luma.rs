//! Luma AI: Ray video and Photon images.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "luma";

pub struct Luma;

#[async_trait]
impl Provider for Luma {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Luma AI".into(),
            kind: ProviderKind::Cloud,
            tagline: "Ray video and Photon images".into(),
            website: "https://lumalabs.ai/api".into(),
            needs_key: true,
            key_env: vec!["LUMAAI_API_KEY".into(), "LUMA_API_KEY".into()],
            key_url: Some("https://lumalabs.ai/api/keys".into()),
            key_hint: Some("luma-…".into()),
            default_base_url: "https://api.lumalabs.ai/dream-machine/v1".into(),
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
