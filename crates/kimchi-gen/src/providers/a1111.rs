//! Stable Diffusion WebUI: AUTOMATIC1111, Forge, SD.Next or Draw Things.
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "a1111";

pub struct A1111;

#[async_trait]
impl Provider for A1111 {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Stable Diffusion WebUI".into(),
            kind: ProviderKind::Local,
            tagline: "AUTOMATIC1111, Forge, SD.Next or Draw Things".into(),
            website: "https://github.com/AUTOMATIC1111/stable-diffusion-webui".into(),
            needs_key: false,
            key_env: vec![],
            key_url: None,
            key_hint: None,
            default_base_url: "http://127.0.0.1:7860".into(),
            base_url_editable: true,
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
