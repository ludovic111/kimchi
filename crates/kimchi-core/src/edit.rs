//! Every change to a project goes through an [`Edit`]. Edits are plain data so
//! the UI, the generation harness and scripts all speak the same language.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::*;

#[derive(Debug, Error, PartialEq)]
pub enum EditError {
    #[error("clip {0} not found")]
    ClipNotFound(Id),
    #[error("track {0} not found")]
    TrackNotFound(Id),
    #[error("asset {0} not found")]
    AssetNotFound(Id),
    #[error("track is locked")]
    Locked,
    /// A batch from another client (`agent`, `cli`…) is open; its step would swallow this change.
    #[error("a batch of changes from the {0} is running; try again when it ends")]
    Busy(String),
    #[error("{0}")]
    Invalid(String),
}

pub type EditResult<T = ()> = Result<T, EditError>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Start,
    End,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipMove {
    pub clip_id: Id,
    pub track_id: Id,
    pub start: f64,
}

/// A clip and the track it goes on (paste).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackClip {
    pub track_id: Id,
    pub clip: Clip,
}

/// Partial update for a clip. `None` fields are left untouched.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ClipPatch {
    pub name: Option<String>,
    pub transform: Option<Transform>,
    pub volume: Option<f64>,
    pub fade_in: Option<f64>,
    pub fade_out: Option<f64>,
    pub speed: Option<f64>,
    #[serde(default)]
    pub reverse: Option<bool>,
    pub text: Option<TextStyle>,
    pub color: Option<String>,
    /// Replaces every keyframe of the clip.
    #[serde(default)]
    pub keyframes: Option<crate::anim::Keyframes>,
    /// Motion clips: the new scene (and the template that made it, if any).
    #[serde(default)]
    pub scene: Option<crate::motion::Scene>,
    #[serde(default)]
    pub template: Option<Option<crate::motion::TemplateRef>>,
    #[serde(default)]
    pub effects: Option<crate::effects::Effects>,
    /// `Some(None)` removes the transition.
    #[serde(default)]
    pub transition: Option<Option<crate::transition::Transition>>,
    /// Motion clips: frames rendered ahead (`Some(None)` goes back to drawing it live).
    #[serde(default)]
    pub rendered: Option<Option<crate::model::Rendered>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TrackPatch {
    pub name: Option<String>,
    pub muted: Option<bool>,
    pub hidden: Option<bool>,
    pub locked: Option<bool>,
    /// Makes it (or stops it being) a captions track (video tracks only).
    #[serde(default)]
    pub captions: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Edit {
    RenameProject { name: String },
    SetSettings { settings: ProjectSettings },

    /// Adds media to the library. Not recorded in history: undo never un-imports.
    AddAsset { asset: Asset },
    /// Replaces an asset's metadata (thumbnails, proxies…). Not recorded in history.
    UpdateAsset { asset: Asset },
    /// Removes an asset and every clip that uses it.
    RemoveAsset { asset_id: Id },

    AddTrack { kind: TrackKind, index: Option<usize> },
    RemoveTrack { track_id: Id },
    UpdateTrack { track_id: Id, patch: TrackPatch },
    MoveTrack { track_id: Id, index: usize },

    /// Puts an asset on the timeline. Without a track, the first compatible
    /// track that is free at `start` is used (or a new one is created).
    InsertAsset { asset_id: Id, track_id: Option<Id>, start: f64 },
    AddClip { track_id: Option<Id>, clip: Clip },
    MoveClips { moves: Vec<ClipMove> },
    /// Moves one edge of a clip to the timeline time `time`.
    TrimClip { clip_id: Id, edge: Edge, time: f64 },
    /// Splits clips at `time`. Without ids, every clip under the playhead on an unlocked track.
    Split { time: f64, clip_ids: Option<Vec<Id>> },
    DeleteClips { clip_ids: Vec<Id>, ripple: bool },
    DuplicateClips { clip_ids: Vec<Id> },
    /// Places copies of clips (new ids) where given; what they land on is overwritten.
    PasteClips { clips: Vec<TrackClip> },
    UpdateClip { clip_id: Id, patch: ClipPatch },
    /// Closes the empty space at `time` on a track by pulling later clips left.
    CloseGap { track_id: Id, time: f64 },

    /// Swaps every placeholder for `job_id` with the finished asset. Not recorded in history.
    ResolvePending { job_id: String, asset_id: Id },
    /// Drops every placeholder for `job_id`. Not recorded in history.
    DropPending { job_id: String },

    AddMarker { time: f64, label: String },
    RemoveMarker { marker_id: Id },
}

impl Edit {
    /// Edits that keep the project in sync with the outside world instead of
    /// expressing a user decision. They apply to every snapshot in the undo
    /// history so undo never brings back stale state.
    pub fn is_background(&self) -> bool {
        matches!(self, Edit::AddAsset { .. } | Edit::UpdateAsset { .. } | Edit::ResolvePending { .. } | Edit::DropPending { .. })
    }
}

/// What an edit produced, useful for the UI to select new things.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EditOutcome {
    pub created_clips: Vec<Id>,
    pub created_tracks: Vec<Id>,
}

/// Latest time (and longest duration) an edit accepts, in seconds (about 115 days): beyond it
/// sums overflow to infinity, which a project file can't hold.
pub const MAX_TIME: f64 = 1e7;

/// Refuses a time or duration that isn't a number or is absurdly far.
fn check_time(what: &str, v: f64) -> EditResult {
    if v.is_finite() && v.abs() <= MAX_TIME {
        Ok(())
    } else {
        Err(EditError::Invalid(format!("{what} {v} is out of range (at most {MAX_TIME} s)")))
    }
}

fn check_clip_times(c: &Clip) -> EditResult {
    check_time("start", c.start)?;
    check_time("duration", c.duration)?;
    check_time("in point", c.in_point)
}

impl Project {
    pub fn apply(&mut self, edit: &Edit) -> EditResult<EditOutcome> {
        self.check_times(edit)?;
        let mut out = EditOutcome::default();
        match edit {
            Edit::RenameProject { name } => {
                let name = name.trim();
                if name.is_empty() {
                    return Err(EditError::Invalid("project name can't be empty".into()));
                }
                self.name = name.to_string();
            }
            Edit::SetSettings { settings } => {
                if settings.width < 16 || settings.height < 16 || settings.fps <= 0.0 {
                    return Err(EditError::Invalid("invalid project settings".into()));
                }
                self.settings = settings.clone();
            }
            Edit::AddAsset { asset } => {
                if self.asset(asset.id).is_none() {
                    self.assets.push(asset.clone());
                }
            }
            Edit::UpdateAsset { asset } => {
                if let Some(a) = self.asset_mut(asset.id) {
                    *a = asset.clone();
                }
            }
            Edit::RemoveAsset { asset_id } => {
                self.assets.retain(|a| a.id != *asset_id);
                for t in &mut self.tracks {
                    t.clips.retain(|c| c.asset_id() != Some(*asset_id));
                }
            }
            Edit::AddTrack { kind, index } => {
                let track = Track::new(*kind, self.next_track_name(*kind));
                out.created_tracks.push(track.id);
                let index = index.unwrap_or_else(|| self.default_track_index(*kind)).min(self.tracks.len());
                self.tracks.insert(index, track);
            }
            Edit::RemoveTrack { track_id } => {
                let before = self.tracks.len();
                self.tracks.retain(|t| t.id != *track_id);
                if self.tracks.len() == before {
                    return Err(EditError::TrackNotFound(*track_id));
                }
            }
            Edit::UpdateTrack { track_id, patch } => {
                let t = self.track_mut(*track_id).ok_or(EditError::TrackNotFound(*track_id))?;
                if let Some(n) = &patch.name {
                    t.name = n.clone();
                }
                if let Some(v) = patch.muted {
                    t.muted = v;
                }
                if let Some(v) = patch.hidden {
                    t.hidden = v;
                }
                if let Some(v) = patch.locked {
                    t.locked = v;
                }
                if let Some(v) = patch.captions {
                    if v && t.kind != TrackKind::Video {
                        return Err(EditError::Invalid("captions go on a video track".into()));
                    }
                    t.captions = v;
                }
            }
            Edit::MoveTrack { track_id, index } => {
                let from = self.tracks.iter().position(|t| t.id == *track_id).ok_or(EditError::TrackNotFound(*track_id))?;
                let t = self.tracks.remove(from);
                let index = (*index).min(self.tracks.len());
                self.tracks.insert(index, t);
            }
            Edit::InsertAsset { asset_id, track_id, start } => {
                let asset = self.asset(*asset_id).ok_or(EditError::AssetNotFound(*asset_id))?;
                let duration = asset.duration().unwrap_or(DEFAULT_STILL_DURATION).max(MIN_CLIP);
                let clip = Clip::new(&asset.name, start.max(0.0), duration, ClipContent::Media { asset_id: *asset_id });
                out.created_clips.push(clip.id);
                let track = self.pick_track(*track_id, &clip, &mut out)?;
                place(&mut self.tracks[track], clip, &self.assets);
            }
            Edit::AddClip { track_id, clip } => {
                let mut clip = clip.clone();
                clip.start = clip.start.max(0.0);
                clip.duration = clip.duration.max(MIN_CLIP);
                out.created_clips.push(clip.id);
                let track = self.pick_track(*track_id, &clip, &mut out)?;
                place(&mut self.tracks[track], clip, &self.assets);
            }
            Edit::MoveClips { moves } => self.move_clips(moves)?,
            Edit::TrimClip { clip_id, edge, time } => self.trim(*clip_id, *edge, *time)?,
            Edit::Split { time, clip_ids } => {
                let ids: Vec<Id> = match clip_ids {
                    Some(ids) => ids.clone(),
                    None => self
                        .tracks
                        .iter()
                        .filter(|t| !t.locked)
                        .flat_map(|t| t.clips.iter().filter(|c| c.contains(*time)).map(|c| c.id))
                        .collect(),
                };
                for id in ids {
                    if let Some(new_id) = self.split(id, *time)? {
                        out.created_clips.push(new_id);
                    }
                }
            }
            Edit::DeleteClips { clip_ids, ripple } => self.delete_clips(clip_ids, *ripple)?,
            Edit::DuplicateClips { clip_ids } => {
                for id in clip_ids {
                    let (ti, ci) = self.locate_clip(*id).ok_or(EditError::ClipNotFound(*id))?;
                    let mut copy = self.tracks[ti].clips[ci].clone();
                    copy.id = new_id();
                    copy.start = self.tracks[ti].end();
                    out.created_clips.push(copy.id);
                    place(&mut self.tracks[ti], copy, &self.assets);
                }
            }
            Edit::PasteClips { clips } => {
                // Check everything first so a paste lands whole or not at all.
                for c in clips {
                    let t = self.track(c.track_id).ok_or(EditError::TrackNotFound(c.track_id))?;
                    if t.locked {
                        return Err(EditError::Locked);
                    }
                    if let Some(a) = c.clip.asset_id()
                        && self.asset(a).is_none()
                    {
                        return Err(EditError::AssetNotFound(a));
                    }
                    if !t.accepts(&c.clip.content, &self.assets) {
                        return Err(EditError::Invalid("that clip can't go on this track".into()));
                    }
                }
                for c in clips {
                    let mut copy = c.clip.clone();
                    copy.id = new_id();
                    copy.start = copy.start.max(0.0);
                    copy.duration = copy.duration.max(MIN_CLIP);
                    out.created_clips.push(copy.id);
                    let i = self.tracks.iter().position(|t| t.id == c.track_id).expect("checked above");
                    place(&mut self.tracks[i], copy, &self.assets);
                }
            }
            Edit::UpdateClip { clip_id, patch } => self.update_clip(*clip_id, patch)?,
            Edit::CloseGap { track_id, time } => {
                let t = self.track_mut(*track_id).ok_or(EditError::TrackNotFound(*track_id))?;
                if t.locked {
                    return Err(EditError::Locked);
                }
                // Pulling clips left onto a clip under `time` would overlap them.
                if t.clips.iter().any(|c| c.start < *time - 1e-9 && c.end() > *time + 1e-9) {
                    return Err(EditError::Invalid("no gap at that time: a clip is there".into()));
                }
                let prev_end = t.clips.iter().filter(|c| c.end() <= *time + 1e-9).map(Clip::end).fold(0.0, f64::max);
                let next_start = t.clips.iter().filter(|c| c.start >= *time - 1e-9).map(|c| c.start).fold(f64::INFINITY, f64::min);
                if next_start.is_finite() && next_start > prev_end {
                    let shift = next_start - prev_end;
                    for c in t.clips.iter_mut().filter(|c| c.start >= next_start - 1e-9) {
                        c.start -= shift;
                    }
                }
            }
            Edit::ResolvePending { job_id, asset_id } => {
                let asset = self.asset(*asset_id).ok_or(EditError::AssetNotFound(*asset_id))?.clone();
                for t in &mut self.tracks {
                    for c in &mut t.clips {
                        if matches!(&c.content, ClipContent::Pending { job_id: j, .. } if j == job_id) {
                            c.content = ClipContent::Media { asset_id: asset.id };
                            c.name = asset.name.clone();
                            if let Some(d) = asset.duration() {
                                c.duration = d.max(MIN_CLIP);
                            }
                        }
                    }
                    // A longer result than the placeholder may now overlap the next clip.
                    resolve_overlaps(t, &self.assets);
                }
            }
            Edit::DropPending { job_id } => {
                for t in &mut self.tracks {
                    t.clips.retain(|c| !matches!(&c.content, ClipContent::Pending { job_id: j, .. } if j == job_id));
                }
            }
            Edit::AddMarker { time, label } => {
                self.markers.push(Marker { id: new_id(), time: time.max(0.0), label: label.clone(), color: "#ff5a36".into() });
                self.markers.sort_by(|a, b| a.time.total_cmp(&b.time));
            }
            Edit::RemoveMarker { marker_id } => self.markers.retain(|m| m.id != *marker_id),
        }
        self.updated_at = chrono::Utc::now();
        Ok(out)
    }

    /// Times and durations an edit brings in must be finite and within [`MAX_TIME`].
    fn check_times(&self, edit: &Edit) -> EditResult {
        match edit {
            Edit::InsertAsset { start, .. } => check_time("start", *start),
            Edit::AddClip { clip, .. } => check_clip_times(clip),
            Edit::MoveClips { moves } => moves.iter().try_for_each(|m| check_time("start", m.start)),
            Edit::TrimClip { time, .. } | Edit::Split { time, .. } | Edit::CloseGap { time, .. } | Edit::AddMarker { time, .. } => check_time("time", *time),
            Edit::PasteClips { clips } => clips.iter().try_for_each(|c| check_clip_times(&c.clip)),
            // Copies go after the track's end.
            Edit::DuplicateClips { clip_ids } => clip_ids.iter().try_for_each(|id| {
                let (ti, ci) = self.locate_clip(*id).ok_or(EditError::ClipNotFound(*id))?;
                check_time("end", self.tracks[ti].end() + self.tracks[ti].clips[ci].duration)
            }),
            Edit::UpdateClip { patch, .. } => {
                for (what, v) in [("volume", patch.volume), ("fade in", patch.fade_in), ("fade out", patch.fade_out), ("speed", patch.speed)] {
                    if v.is_some_and(|v| !v.is_finite()) {
                        return Err(EditError::Invalid(format!("{what} must be a number")));
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// New video tracks go on top of the stack, new audio tracks at the bottom.
    fn default_track_index(&self, kind: TrackKind) -> usize {
        match kind {
            TrackKind::Video => 0,
            TrackKind::Audio => self.tracks.len(),
        }
    }

    /// Finds (or creates) the track a new clip should land on.
    fn pick_track(&mut self, track_id: Option<Id>, clip: &Clip, out: &mut EditOutcome) -> EditResult<usize> {
        let kind = clip.content.track_kind(&self.assets).unwrap_or(TrackKind::Video);
        if let Some(id) = track_id {
            let i = self.tracks.iter().position(|t| t.id == id).ok_or(EditError::TrackNotFound(id))?;
            let t = &self.tracks[i];
            if t.locked {
                return Err(EditError::Locked);
            }
            if t.kind == kind {
                return Ok(i);
            }
        }
        let free = |t: &Track| !t.locked && t.kind == kind && t.clips.iter().all(|c| c.end() <= clip.start + 1e-9 || c.start >= clip.end() - 1e-9);
        // Footage goes on the lowest free video track (closest to the base layer);
        // titles go on the highest so nothing above can cover them.
        let overlay = matches!(clip.content, ClipContent::Text { .. } | ClipContent::Motion { .. });
        let found = match kind {
            TrackKind::Video if overlay => self.tracks.iter().position(free),
            TrackKind::Video => self.tracks.iter().rposition(free),
            TrackKind::Audio => self.tracks.iter().position(free),
        };
        if let Some(i) = found {
            return Ok(i);
        }
        let track = Track::new(kind, self.next_track_name(kind));
        out.created_tracks.push(track.id);
        let index = match kind {
            // Just above the existing video tracks' top, i.e. at the very top.
            TrackKind::Video => 0,
            TrackKind::Audio => self.tracks.len(),
        };
        self.tracks.insert(index, track);
        Ok(index)
    }

    fn move_clips(&mut self, moves: &[ClipMove]) -> EditResult {
        // Lift every moving clip out first so they can't carve each other.
        let mut lifted = Vec::with_capacity(moves.len());
        for m in moves {
            let (ti, ci) = self.locate_clip(m.clip_id).ok_or(EditError::ClipNotFound(m.clip_id))?;
            if self.tracks[ti].locked {
                return Err(EditError::Locked);
            }
            let dest = self.track(m.track_id).ok_or(EditError::TrackNotFound(m.track_id))?;
            if dest.locked {
                return Err(EditError::Locked);
            }
            let clip = &self.tracks[ti].clips[ci];
            if !dest.accepts(&clip.content, &self.assets) {
                return Err(EditError::Invalid("that clip can't go on this track".into()));
            }
            lifted.push((self.tracks[ti].clips.remove(ci), m));
        }
        for (mut clip, m) in lifted {
            clip.start = m.start.max(0.0);
            let i = self.tracks.iter().position(|t| t.id == m.track_id).expect("checked above");
            place(&mut self.tracks[i], clip, &self.assets);
        }
        Ok(())
    }

    fn trim(&mut self, clip_id: Id, edge: Edge, time: f64) -> EditResult {
        let (ti, ci) = self.locate_clip(clip_id).ok_or(EditError::ClipNotFound(clip_id))?;
        if self.tracks[ti].locked {
            return Err(EditError::Locked);
        }
        let source_len = {
            let c = &self.tracks[ti].clips[ci];
            c.asset_id().and_then(|id| self.asset(id)).and_then(Asset::duration)
        };
        let track = &mut self.tracks[ti];
        // Neighbours bound the trim so it never eats another clip.
        let prev_end = if ci > 0 { track.clips[ci - 1].end() } else { 0.0 };
        let next_start = track.clips.get(ci + 1).map(|c| c.start).unwrap_or(f64::INFINITY);
        let c = &mut track.clips[ci];
        // Source seconds the edges can be dragged out by (unbounded for stills, titles…).
        let (before, after) = source_len.map_or((f64::INFINITY, f64::INFINITY), |len| c.room(len));
        let (forward, speed) = (!c.reverse, c.speed);
        match edge {
            Edge::Start => {
                let (lo, hi) = (prev_end.max(c.start - before), c.end() - MIN_CLIP);
                // A sliver hard against its neighbour has nowhere to go.
                if lo > hi {
                    return Ok(());
                }
                let new_start = time.clamp(lo, hi);
                let delta = new_start - c.start;
                // Forward clips lose (or gain) source at the head; reversed ones at the tail.
                if forward {
                    c.in_point = (c.in_point + delta * speed).max(0.0);
                }
                c.start = new_start;
                c.duration -= delta;
                // Keyframes stay where they were on the timeline.
                crate::anim::shift(&mut c.keyframes, -delta);
            }
            Edge::End => {
                let hi = next_start.min(c.end() + after);
                let new_end = time.clamp(c.start + MIN_CLIP, hi.max(c.start + MIN_CLIP));
                let delta = new_end - c.end();
                if !forward {
                    c.in_point = (c.in_point - delta * speed).max(0.0);
                }
                c.duration = new_end - c.start;
            }
        }
        clamp_fades(c);
        Ok(())
    }

    /// Splits a clip in two at `time`; returns the id of the right half.
    fn split(&mut self, clip_id: Id, time: f64) -> EditResult<Option<Id>> {
        let (ti, ci) = self.locate_clip(clip_id).ok_or(EditError::ClipNotFound(clip_id))?;
        if self.tracks[ti].locked {
            return Err(EditError::Locked);
        }
        let c = &self.tracks[ti].clips[ci];
        if time - c.start < MIN_CLIP || c.end() - time < MIN_CLIP {
            return Ok(None);
        }
        let right = split_right(c, time);
        let left = &mut self.tracks[ti].clips[ci];
        *left = left_part(left, time);
        left.fade_out = 0.0;
        clamp_fades(left);
        let id = right.id;
        self.tracks[ti].clips.insert(ci + 1, right);
        Ok(Some(id))
    }

    fn delete_clips(&mut self, ids: &[Id], ripple: bool) -> EditResult {
        let ids: HashSet<Id> = ids.iter().copied().collect();
        for t in &mut self.tracks {
            if t.locked && t.clips.iter().any(|c| ids.contains(&c.id)) {
                return Err(EditError::Locked);
            }
            if !ripple {
                t.clips.retain(|c| !ids.contains(&c.id));
                continue;
            }
            // Ripple: every later clip on the same track moves left by the removed length.
            let mut removed: Vec<(f64, f64)> = t.clips.iter().filter(|c| ids.contains(&c.id)).map(|c| (c.start, c.duration)).collect();
            removed.sort_by(|a, b| b.0.total_cmp(&a.0));
            t.clips.retain(|c| !ids.contains(&c.id));
            for (start, len) in removed {
                for c in t.clips.iter_mut().filter(|c| c.start >= start - 1e-9) {
                    c.start -= len;
                }
            }
        }
        Ok(())
    }

    fn update_clip(&mut self, clip_id: Id, p: &ClipPatch) -> EditResult {
        let (ti, ci) = self.locate_clip(clip_id).ok_or(EditError::ClipNotFound(clip_id))?;
        if self.tracks[ti].locked {
            return Err(EditError::Locked);
        }
        let source_len = {
            let c = &self.tracks[ti].clips[ci];
            c.asset_id().and_then(|id| self.asset(id)).and_then(Asset::duration)
        };
        let c = &mut self.tracks[ti].clips[ci];
        if let Some(v) = &p.name {
            c.name = v.clone();
        }
        if let Some(v) = &p.transform {
            let mut t = v.clone();
            t.opacity = t.opacity.clamp(0.0, 1.0);
            t.scale = t.scale.max(0.01);
            c.transform = t;
        }
        if let Some(v) = p.volume {
            c.volume = v.clamp(0.0, 4.0);
        }
        if let Some(v) = p.fade_in {
            c.fade_in = v.max(0.0);
        }
        if let Some(v) = p.fade_out {
            c.fade_out = v.max(0.0);
        }
        if let Some(speed) = p.speed {
            let speed = speed.clamp(0.1, 16.0);
            // Keep the same source range: the clip gets shorter or longer on the timeline.
            let source_span = c.duration * c.speed;
            let mut duration = (source_span / speed).max(MIN_CLIP);
            if let Some(len) = source_len {
                duration = duration.min((len - c.in_point) / speed);
            }
            if duration < MIN_CLIP - 1e-9 {
                return Err(EditError::Invalid(format!("at {speed}x there isn't a frame of media left after the clip's in point")));
            }
            c.speed = speed;
            c.duration = duration;
        }
        if let Some(reverse) = p.reverse {
            if reverse && source_len.is_none() {
                return Err(EditError::Invalid("only video and sound clips can play backwards".into()));
            }
            // Same source range, played the other way.
            c.reverse = reverse;
        }
        if let Some(effects) = &p.effects {
            c.effects = effects.clone().clamped();
        }
        if let Some(tr) = &p.transition {
            c.transition = tr.clone().map(|mut tr| {
                tr.duration = tr.duration.clamp(MIN_CLIP, 30.0);
                tr
            });
        }
        if let Some(r) = &p.rendered {
            if r.is_some() && !matches!(c.content, ClipContent::Motion { .. }) {
                return Err(EditError::Invalid("only motion clips are rendered ahead".into()));
            }
            c.rendered = r.clone();
        }
        if let Some(style) = &p.text {
            match &mut c.content {
                ClipContent::Text { style: s } => *s = style.clone(),
                _ => return Err(EditError::Invalid("not a text clip".into())),
            }
        }
        if let Some(color) = &p.color {
            match &mut c.content {
                ClipContent::Solid { color: s } => *s = color.clone(),
                _ => return Err(EditError::Invalid("not a solid clip".into())),
            }
        }
        if let Some(keys) = &p.keyframes {
            let mut keys = keys.clone();
            crate::anim::normalize(&mut keys);
            c.keyframes = keys;
        }
        if let Some(new_scene) = &p.scene {
            match &mut c.content {
                ClipContent::Motion { scene, .. } => *scene = new_scene.clone(),
                _ => return Err(EditError::Invalid("not a motion clip".into())),
            }
        }
        if let Some(new_template) = &p.template {
            match &mut c.content {
                ClipContent::Motion { template, .. } => *template = new_template.clone(),
                _ => return Err(EditError::Invalid("not a motion clip".into())),
            }
        }
        clamp_fades(c);
        let track = &mut self.tracks[ti];
        resolve_overlaps(track, &self.assets);
        Ok(())
    }
}

fn clamp_fades(c: &mut Clip) {
    c.fade_in = c.fade_in.clamp(0.0, c.duration);
    c.fade_out = c.fade_out.clamp(0.0, c.duration - c.fade_in);
}

/// The part of `c` after `time`, as a new clip (no fade in, no transition: it starts on a cut
/// inside the old clip).
fn split_right(c: &Clip, time: f64) -> Clip {
    let mut right = c.cut(time, c.end());
    right.id = new_id();
    right.fade_in = 0.0;
    right.transition = None;
    clamp_fades(&mut right);
    right
}

/// The part of `c` before `time` (same id).
fn left_part(c: &Clip, time: f64) -> Clip {
    let mut left = c.cut(c.start, time);
    // Keyframes are relative to the start, which hasn't moved.
    left.keyframes = c.keyframes.clone();
    clamp_fades(&mut left);
    left
}

/// Inserts `clip` into `track`, overwriting whatever was underneath it. What is left of a
/// covered clip shorter than [`MIN_CLIP`] goes too (a sliver nobody can see or grab).
pub fn place(track: &mut Track, clip: Clip, assets: &[Asset]) {
    let _ = assets;
    let (s, e) = (clip.start, clip.end());
    let mut keep = Vec::with_capacity(track.clips.len() + 2);
    for c in track.clips.drain(..) {
        if c.end() <= s + 1e-9 || c.start >= e - 1e-9 {
            keep.push(c);
            continue;
        }
        if c.start < s && s - c.start >= MIN_CLIP {
            keep.push(left_part(&c, s));
        }
        if c.end() > e && c.end() - e >= MIN_CLIP {
            keep.push(split_right(&c, e));
        }
    }
    keep.push(clip);
    keep.sort_by(|a, b| a.start.total_cmp(&b.start));
    track.clips = keep;
}

/// After a clip grew in place, push later clips right so nothing overlaps.
fn resolve_overlaps(track: &mut Track, _assets: &[Asset]) {
    track.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
    for i in 1..track.clips.len() {
        let prev_end = track.clips[i - 1].end();
        if track.clips[i].start < prev_end - 1e-9 {
            track.clips[i].start = prev_end;
        }
    }
}
