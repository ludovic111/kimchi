//! Runway: Gen-4 video and image references.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "runway";

pub struct Runway;

#[async_trait]
impl Provider for Runway {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Runway".into(),
            kind: ProviderKind::Cloud,
            tagline: "Gen-4 video and image references".into(),
            website: "https://runwayml.com".into(),
            needs_key: true,
            key_env: vec!["RUNWAYML_API_SECRET".into(), "RUNWAY_API_KEY".into()],
            key_url: Some("https://dev.runwayml.com".into()),
            key_hint: Some("key_…".into()),
            default_base_url: "https://api.dev.runwayml.com/v1".into(),
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
