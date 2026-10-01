//! Google Gemini: Imagen, Gemini image and Veo video.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "google";

pub struct Google;

#[async_trait]
impl Provider for Google {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Google Gemini".into(),
            kind: ProviderKind::Cloud,
            tagline: "Imagen, Gemini image and Veo video".into(),
            website: "https://ai.google.dev".into(),
            needs_key: true,
            key_env: vec!["GEMINI_API_KEY".into(), "GOOGLE_API_KEY".into()],
            key_url: Some("https://aistudio.google.com/apikey".into()),
            key_hint: Some("AIza…".into()),
            default_base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
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
