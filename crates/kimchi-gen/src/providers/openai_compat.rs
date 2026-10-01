//! OpenAI-compatible server: Any server speaking the OpenAI Images API (LocalAI, vLLM-Omni, sd-server…).
//!
//! TODO: not implemented yet.

use async_trait::async_trait;

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;

pub const ID: &str = "openai_compat";

pub struct OpenAiCompat;

#[async_trait]
impl Provider for OpenAiCompat {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "OpenAI-compatible server".into(),
            kind: ProviderKind::Local,
            tagline: "Any server speaking the OpenAI Images API (LocalAI, vLLM-Omni, sd-server…)".into(),
            website: "https://platform.openai.com/docs/api-reference/images".into(),
            needs_key: false,
            key_env: vec![],
            key_url: None,
            key_hint: Some("optional".into()),
            default_base_url: "http://127.0.0.1:8080/v1".into(),
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
