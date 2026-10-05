//! Microphone takes stay on the window thread; the recorder writes the WAV in the background.
use std::time::{Duration, Instant};

use gpui::{App, Context, Task};
use kimchi_audio::record::{self, Options, Take};
use kimchi_control::{CmdResult, Source};
use kimchi_core::{Asset, AssetOrigin, Edit, Id, MediaKind, MediaMeta, TrackKind, new_id};
use serde_json::{Value, json};

use crate::store::{Store, StoreExt};

#[derive(Default)]
pub struct Recorder {
    active: Option<record::Recording>,
    countdown: Option<Task<()>>,
    project: Option<Id>,
    track: Option<Id>,
}

pub fn command(params: &Value, cx: &mut App) -> CmdResult {
    cx.store().update(cx, |s, cx| match params["action"].as_str().unwrap_or("status") {
        "status" => Ok(json!({
            "recording": s.audio.to_json(s.project.as_deref())["recording"],
            "seconds": s.recorder.active.as_ref().map(record::Recording::seconds),
            "input": s.recorder.active.as_ref().map(record::Recording::level),
        })),
        "start" => start(s, params, cx),
        "stop" => stop(s, false, cx),
        "cancel" => stop(s, true, cx),
        _ => Err("audio.record action must be start, stop, cancel or status".into()),
    })
}

fn start(s: &mut Store, params: &Value, cx: &mut Context<Store>) -> CmdResult {
    if s.audio.recording.is_some() {
        return Err("A voice-over is already recording. Stop or cancel it first.".into());
    }
    let p = s.session.project()?;
    let track = match params["trackId"].as_str() {
        Some(key) => Some(kimchi_control::resolve::track(&p, key)?),
        None => p.tracks.iter().find(|t| t.kind == TrackKind::Audio && t.mix.armed).map(|t| t.id),
    };
    if let Some(t) = track.and_then(|id| p.tracks.iter().find(|t| t.id == id))
        && (t.kind != TrackKind::Audio || t.locked)
    {
        return Err("Choose an unlocked audio track to record onto.".into());
    }
    let count_in = params["countIn"].as_f64().unwrap_or(s.settings.audio.count_in);
    if !count_in.is_finite() || !(0.0..=30.0).contains(&count_in) {
        return Err("countIn must be between 0 and 30 seconds.".into());
    }
    let input = params["input"].as_str().unwrap_or(&s.settings.audio.input_device);
    let options = Options {
        device: (!input.is_empty()).then(|| input.to_string()),
        rate: p.settings.sample_rate,
        channels: 1,
        path: s.session.data_dir.join("recordings").join(format!("Voice-over-{}.wav", new_id())),
        start: s.playback.read(cx).playhead,
    };
    s.playback.update(cx, |pb, cx| pb.pause(cx));
    s.recorder.project = Some(p.id);
    s.recorder.track = track;
    s.set_audio(|a| a.recording = Some(super::Recording {
        track: track.unwrap_or_default(), start: options.start, count_in, since: Instant::now(),
    }), cx);
    if count_in == 0.0 {
        if let Err(e) = capture(s, options, cx) {
            s.set_audio(|a| a.recording = None, cx);
            return Err(e);
        }
    } else {
        s.flash(format!("Recording starts in {} seconds", count_in.ceil()), cx);
        s.recorder.countdown = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs_f64(count_in)).await;
            let _ = this.update(cx, |s, cx| {
                if s.recorder.project != s.session.current_id() || s.audio.recording.is_none() {
                    return;
                }
                if let Err(e) = capture(s, options, cx) {
                    s.set_audio(|a| a.recording = None, cx);
                    s.error(e, cx);
                }
            });
        }));
    }
    Ok(json!({ "recording": s.audio.to_json(s.project.as_deref())["recording"] }))
}

fn capture(s: &mut Store, options: Options, cx: &mut Context<Store>) -> Result<(), String> {
    s.recorder.active = Some(record::start(options)?);
    s.set_audio(|a| {
        if let Some(r) = &mut a.recording {
            r.count_in = 0.0;
            r.since = Instant::now();
        }
    }, cx);
    s.playback.update(cx, |pb, cx| pb.play(cx));
    s.flash("Recording voice-over · press Record again to finish", cx);
    Ok(())
}

fn stop(s: &mut Store, cancel: bool, cx: &mut Context<Store>) -> CmdResult {
    s.recorder.countdown.take();
    let active = s.recorder.active.take();
    s.set_audio(|a| a.recording = None, cx);
    s.playback.update(cx, |pb, cx| pb.pause(cx));
    let Some(active) = active else { return Ok(json!({ "recording": null })) };
    let take = active.stop()?;
    if cancel || take.seconds <= 0.0 {
        let _ = std::fs::remove_file(&take.path);
        return Ok(json!({ "recording": null, "cancelled": true }));
    }
    let result = place_take(s, &take);
    if let Err(e) = result {
        return Err(format!("The take is saved at {} but couldn't be placed: {e}", take.path.display()));
    }
    if take.dropped > 0 {
        s.error(format!("Voice-over saved, but the audio input dropped {} samples", take.dropped), cx);
    } else {
        s.flash("Voice-over added to the timeline", cx);
    }
    Ok(json!({ "recording": null, "take": take, "clips": result? }))
}

/// Add the asset, a new track if needed, and the clip as one atomic undo step.
fn place_take(s: &Store, take: &Take) -> Result<Vec<Id>, String> {
    let project = s.recorder.project.ok_or("The recording's project is closed.")?;
    if let Some(id) = s.recorder.track {
        let p = s.session.project()?;
        let t = p.tracks.iter().find(|t| t.id == id).ok_or("The recording track was removed.")?;
        if t.locked || t.kind != TrackKind::Audio {
            return Err("The recording track must be unlocked and accept audio.".into());
        }
    }
    let asset = Asset {
        id: new_id(), name: "Voice-over".into(), path: take.path.to_string_lossy().into_owned(),
        kind: MediaKind::Audio, origin: AssetOrigin::Imported,
        meta: MediaMeta { duration: Some(take.seconds), width: None, height: None, fps: None,
            has_video: false, has_audio: true, video_codec: None, audio_codec: Some("pcm_f32le".into()),
            size_bytes: std::fs::metadata(&take.path).map_err(|e| e.to_string())?.len() },
        created_at: chrono::Utc::now(),
        thumbnail: None, filmstrip: None, waveform: None, proxy: None, beats: None,
    };
    let clips = s.session.edit("Record voice-over", Source::Window, |ed| {
        if ed.project().id != project {
            return Err("The project changed while recording.".into());
        }
        ed.begin_batch("Record voice-over", "window");
        let result = (|| {
            ed.apply(&Edit::AddAsset { asset: asset.clone() }, None)?;
            let track = match s.recorder.track {
                Some(id) => id,
                None => ed.apply(&Edit::AddTrack { kind: TrackKind::Audio, index: None }, None)?.created_tracks[0],
            };
            ed.apply(&Edit::InsertAsset { asset_id: asset.id, track_id: Some(track), start: take.start }, None)
        })();
        match result {
            Ok(out) => { ed.end_batch(); Ok(out.created_clips) }
            Err(e) => { ed.rollback_batch(); Err(e.to_string()) }
        }
    })?;
    kimchi_control::commands::media::spawn_previews(&s.session, project, asset);
    Ok(clips)
}

pub fn project_switched(s: &mut Store, cx: &mut Context<Store>) {
    s.recorder.countdown.take();
    if let Some(active) = s.recorder.active.take() {
        let path = active.path().to_path_buf();
        match active.stop() {
            Ok(_) => s.flash(format!("Voice-over saved at {}", path.display()), cx),
            Err(e) => s.error(e, cx),
        }
    }
    s.recorder.project = None;
    s.recorder.track = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn a_count_in_can_be_cancelled_without_changing_the_project(cx: &mut TestAppContext) {
        let (f, _, cx) = crate::tests::setup(cx);
        let before = f.project();
        cx.update(|_, cx| {
            command(&json!({"action": "start", "countIn": 10}), cx).unwrap();
            assert!(command(&json!({"action": "status"}), cx).unwrap()["recording"]["countingIn"].as_bool().unwrap());
            assert!(command(&json!({"action": "start", "countIn": 10}), cx).is_err());
            command(&json!({"action": "cancel"}), cx).unwrap();
            assert!(command(&json!({"action": "status"}), cx).unwrap()["recording"].is_null());
        });
        cx.executor().advance_clock(Duration::from_secs(11));
        cx.run_until_parked();
        assert_eq!(f.project(), before);
    }

    #[gpui::test]
    fn a_finished_take_is_one_undo_step_and_failed_placement_rolls_back(cx: &mut TestAppContext) {
        let (f, _, cx) = crate::tests::setup(cx);
        let before = f.project();
        let path = f.session.data_dir.join("test-take.wav");
        // Placement already has the recorder's metadata; preview generation is asynchronous.
        std::fs::write(&path, []).unwrap();
        let take = Take { path, start: 1.0, seconds: 2.0, rate: 48_000, channels: 1,
            peak_db: -12.0, dropped: 0, latency: 0.0, device: "test".into() };
        let steps = f.session.read(|ed| ed.undo_steps().len()).unwrap();
        let clips = cx.update(|_, cx| cx.store().update(cx, |s, _| {
            s.recorder.project = Some(before.id);
            place_take(s, &take).unwrap()
        }));
        assert_eq!(clips.len(), 1);
        let placed = f.project();
        let clip = placed.clip(clips[0]).unwrap();
        assert_eq!((clip.start, clip.duration), (1.0, 2.0));
        assert_eq!(placed.tracks.len(), before.tracks.len() + 1);
        assert_eq!(f.session.read(|ed| ed.undo_steps().len()).unwrap(), steps + 1);
        f.call("history.undo", json!({}));
        assert_eq!(f.project().tracks, before.tracks);
        assert_eq!(f.project().assets, placed.assets);
        cx.update(|_, cx| cx.store().update(cx, |s, _| {
            s.recorder.track = Some(new_id());
            assert!(place_take(s, &take).is_err());
        }));
        assert_eq!(f.project().assets, placed.assets);
    }
}
