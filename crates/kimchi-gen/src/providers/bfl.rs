//! Black Forest Labs: FLUX, straight from the source.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "bfl";

pub struct Bfl;

#[async_trait]
impl Provider for Bfl {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Black Forest Labs".into(),
            kind: ProviderKind::Cloud,
            tagline: "FLUX, straight from the source".into(),
            website: "https://bfl.ai".into(),
            needs_key: true,
            key_env: vec!["BFL_API_KEY".into()],
            key_url: Some("https://dashboard.bfl.ai".into()),
            key_hint: None,
            default_base_url: "https://api.bfl.ai/v1".into(),
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
