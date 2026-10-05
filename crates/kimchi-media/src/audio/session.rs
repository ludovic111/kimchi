//! The audio timeline as a ryolune session: each clip's sound decoded and played through what
//! ryolune can't do itself (speed, reverse, pitch, channels, volume keyframes, clip effects,
//! pan) by kimchi's mixer, then laid out by `kimchi_audio::song::write_session`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kimchi_audio::mixer::{Mixer, Mode, SourceOpener, heard, sound_of};
use kimchi_audio::song::{ClipSound, SessionSounds};
use kimchi_core::{Project, Track, TrackKind};

use super::{FfmpegOpener, failed, rate};
use crate::{MediaError, MediaResult, Tools};

/// Writes the project's audio timeline as a ryolune session at `path`: one ryolune audio track
/// per kimchi track with sound (its clips, fades, gains, effects, fader and pan), the markers,
/// and the cut's length. Returns the file written.
pub async fn write_ryolune_session(tools: &Tools, project: &Project, path: &Path) -> MediaResult<PathBuf> {
    let opener: Arc<dyn SourceOpener> = FfmpegOpener::new(tools.clone());
    let (project, path) = (project.clone(), path.to_path_buf());
    tokio::task::spawn_blocking(move || {
        let rate = rate(&project);
        let mut sounds = SessionSounds { rate, ..Default::default() };
        let total: f64 = project.tracks.iter().flat_map(|t| heard(&project, t)).map(|h| h.clip.duration).sum();
        if total <= 0.0 {
            return Err(MediaError::Unsupported("the timeline has no sound to send".into()));
        }
        kimchi_audio::song::fits(total, rate).map_err(MediaError::Unsupported)?;
        for track in &project.tracks {
            for h in heard(&project, track) {
                let Some(asset) = sound_of(&project, &h.clip) else { continue };
                // The clip alone, its fades and gain left for ryolune.
                let (gain_db, baked) = kimchi_audio::song::clip_gain(&h.clip);
                let mut clip = h.clip.clone();
                clip.transition = None;
                clip.fade_in = 0.0;
                clip.fade_out = 0.0;
                if !baked {
                    clip.volume = 1.0;
                }
                let mut alone = Project::new(&project.name, project.settings.clone());
                alone.tracks = vec![Track { clips: vec![clip.clone()], ..Track::new(TrackKind::Audio, "clip") }];
                alone.assets = vec![asset.clone()];
                alone.mixer.master.limiter = false;
                let mut m = Mixer::new(Arc::new(alone), opener.clone(), rate, Mode::Offline).map_err(MediaError::Unsupported)?;
                m.seek(clip.start);
                let mut frames = vec![[0.0f32; 2]; (clip.duration * rate as f64).round() as usize];
                m.render(&mut frames);
                if let Some(p) = m.problems().into_iter().find(|p| p.contains("missing media file") || p.contains("couldn't decode") || p.contains("ffmpeg wasn't found")) {
                    return Err(MediaError::Unsupported(p));
                }
                let curve = if h.fade_in > 0.0 { h.fade_in_curve } else { h.fade_out_curve };
                sounds.clips.insert(h.clip.id, ClipSound { frames, start: clip.start, gain_db, fade_in: h.fade_in, fade_out: h.fade_out, curve });
            }
        }
        // Ducking moves with the other tracks: the mixer's readings of it become fader automation.
        if project.tracks.iter().any(|t| t.mix.duck.is_some()) {
            let mut m = Mixer::new(Arc::new(project.clone()), opener.clone(), rate, Mode::Offline).map_err(MediaError::Unsupported)?;
            let mut block = vec![[0.0f32; 2]; (rate / 30) as usize];
            let frames = (project.duration() * rate as f64) as usize;
            let mut done = 0;
            while done < frames {
                m.render(&mut block);
                done += block.len();
                if let Some(s) = m.meters().latest() {
                    for (id, db) in s.ducking_db {
                        sounds.ducking.entry(id).or_default().push((s.time, db));
                    }
                }
            }
        }
        kimchi_audio::song::write_session(&project, &sounds, &path).map_err(MediaError::Unsupported)
    })
    .await
    .map_err(failed)?
}
