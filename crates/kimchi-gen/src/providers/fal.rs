//! fal: Hundreds of image and video models, fast queues.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "fal";

pub struct Fal;

#[async_trait]
impl Provider for Fal {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "fal".into(),
            kind: ProviderKind::Cloud,
            tagline: "Hundreds of image and video models, fast queues".into(),
            website: "https://fal.ai".into(),
            needs_key: true,
            key_env: vec!["FAL_KEY".into(), "FAL_API_KEY".into()],
            key_url: Some("https://fal.ai/dashboard/keys".into()),
            key_hint: Some("key-id:key-secret".into()),
            default_base_url: "https://queue.fal.run".into(),
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
