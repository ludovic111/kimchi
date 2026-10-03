//! The sound of a project as speech recognition hears it: the export's mix, 16 kHz mono.

use kimchi_core::Project;
use tokio::io::AsyncReadExt;

use crate::export::{self, ExportFormat, ExportSettings, Quality, SPEECH_SAMPLE_RATE, Sink};
use crate::{Caps, MediaError, MediaResult, Tools, process};

/// The project's sound between `range` (the whole timeline by default), mixed as it exports
/// (volumes, fades, mutes, transitions), as mono f32 samples at [`SPEECH_SAMPLE_RATE`].
/// Silence when nothing makes a sound.
pub async fn speech_samples(tools: &Tools, project: &Project, range: Option<(f64, f64)>) -> MediaResult<Vec<f32>> {
    let caps = Caps::detect(tools).await?;
    let st = ExportSettings { path: String::new(), format: ExportFormat::Wav, quality: Quality::Draft, width: None, height: None, fps: None, range, encoder: Default::default() };
    let (plan, audible) = export::compile(project, &st, &caps, Sink::Speech)?;
    if audible == 0 {
        return Ok(vec![0.0; (plan.duration * SPEECH_SAMPLE_RATE as f64) as usize]);
    }
    let script = (plan.graph.len() > export::INLINE_GRAPH_MAX).then(|| std::env::temp_dir().join(format!("kimchi-speech-{}.txt", kimchi_core::new_id())));
    if let Some(path) = &script {
        tokio::fs::write(path, &plan.graph).await?;
    }
    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-nostats"].map(String::from).to_vec();
    args.extend(plan.body(script.as_deref(), &caps));
    args.push("-".into());
    let result = async {
        let mut child = process::spawn(&tools.ffmpeg, &args, true)?;
        let stderr = process::collect_stderr(&mut child);
        let mut bytes = vec![];
        child.stdout.take().expect("piped stdout").read_to_end(&mut bytes).await?;
        let status = child.wait().await?;
        if !status.success() {
            return Err(MediaError::Ffmpeg(process::summarize(&stderr.await.unwrap_or_default(), status)));
        }
        Ok(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
    }
    .await;
    if let Some(path) = &script {
        let _ = tokio::fs::remove_file(path).await;
    }
    result
}
