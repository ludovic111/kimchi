//! Built-in providers. Order here is the order shown in Settings.

use std::sync::Arc;

use crate::provider::Provider;

pub mod openrouter;
pub mod fal;
pub mod replicate;
pub mod openai;
pub mod google;
pub mod xai;
pub mod runway;
pub mod luma;
pub mod bfl;
pub mod stability;
pub mod together;
pub mod comfyui;
pub mod a1111;
pub mod ollama;
pub mod openai_compat;

pub fn all() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(openrouter::OpenRouter),
        Arc::new(fal::Fal),
        Arc::new(replicate::Replicate),
        Arc::new(openai::OpenAi),
        Arc::new(google::Google),
        Arc::new(xai::Xai),
        Arc::new(runway::Runway),
        Arc::new(luma::Luma),
        Arc::new(bfl::Bfl),
        Arc::new(stability::Stability),
        Arc::new(together::Together),
        Arc::new(comfyui::ComfyUi),
        Arc::new(a1111::A1111),
        Arc::new(ollama::Ollama),
        Arc::new(openai_compat::OpenAiCompat),
    ]
}
