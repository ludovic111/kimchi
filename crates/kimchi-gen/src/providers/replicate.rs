//! Replicate: Run open and proprietary models by the second.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "replicate";

pub struct Replicate;

#[async_trait]
impl Provider for Replicate {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Replicate".into(),
            kind: ProviderKind::Cloud,
            tagline: "Run open and proprietary models by the second".into(),
            website: "https://replicate.com".into(),
            needs_key: true,
            key_env: vec!["REPLICATE_API_TOKEN".into()],
            key_url: Some("https://replicate.com/account/api-tokens".into()),
            key_hint: Some("r8_…".into()),
            default_base_url: "https://api.replicate.com/v1".into(),
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
