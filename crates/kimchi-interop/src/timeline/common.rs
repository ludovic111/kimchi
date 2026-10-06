//! What every format shares: media paths and `file://` URLs, guessing a file's kind, building
//! the imported project ([`Builder`]), keeping a project's tracks in kimchi's order, colours,
//! and what to say in the report.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kimchi_core::{Asset, AssetOrigin, Clip, ClipContent, Id, MediaKind, MediaMeta, Project, ProjectSettings, Track, TrackKind, TransitionKind, new_id};

use super::Imported;
use crate::Report;

// ---------------------------------------------------------------------------------------------
// Paths and URLs

const VIDEO_EXT: &[&str] = &["mp4", "mov", "m4v", "webm", "mkv", "avi", "mxf", "mts", "m2ts", "mpg", "mpeg", "wmv", "flv", "3gp", "r3d", "braw", "dv", "ts", "ogv", "gif"];
const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "heic", "avif", "tif", "tiff", "bmp", "psd", "exr", "dpx", "tga", "svg"];
const AUDIO_EXT: &[&str] = &["mp3", "wav", "m4a", "aac", "flac", "ogg", "opus", "aif", "aiff", "wma", "caf"];

/// The kind of media a file name says it is (video when it doesn't say).
pub fn kind_of(path: &str) -> MediaKind {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if IMAGE_EXT.contains(&ext.as_str()) {
        MediaKind::Image
    } else if AUDIO_EXT.contains(&ext.as_str()) {
        MediaKind::Audio
    } else {
        let _ = VIDEO_EXT;
        MediaKind::Video
    }
}

pub fn file_name(path: &str) -> String {
    let p = path.trim_end_matches(['/', '\\']);
    p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()
}

/// Decodes `%20`-style escapes (bytes that aren't valid UTF-8 are kept as they were).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Escapes what a URL path can't hold as it is (RFC 3986: everything but letters, digits,
/// `-._~` and the path's own `/`, `:`, `@`, `!`, `$`, `&`, `'`, `(`, `)`, `*`, `+`, `,`, `;`, `=`).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for &b in s.as_bytes() {
        let keep = b.is_ascii_alphanumeric() || b"-._~/:@!$&'()*+,;=".contains(&b);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A `file://` URL for a local path (`/a b.mov` → `file:///a%20b.mov`, `C:\x.mov` →
/// `file:///C:/x.mov`).
pub fn path_to_url(path: &str) -> String {
    let p = path.replace('\\', "/");
    if let Some(unc) = p.strip_prefix("//") {
        return format!("file://{}", percent_encode(unc));
    }
    let p = if p.starts_with('/') { p } else { format!("/{p}") };
    format!("file://{}", percent_encode(&p))
}

/// A local path from a `file://` URL (any of `file:///x`, `file://localhost/x`, `file:/x`,
/// `file://host/share` as a UNC path), or from a plain path, which is resolved against `base`
/// when it is relative. Other URLs (`http://…`) are returned as they are.
pub fn url_to_path(url: &str, base: &Path) -> String {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    let path = if lower.starts_with("file:") {
        let rest = &u[5..];
        let rest = if let Some(r) = rest.strip_prefix("//") {
            if let Some(r) = r.strip_prefix("localhost") {
                r.to_string()
            } else if r.starts_with('/') {
                r.to_string()
            } else {
                // file://server/share/x → \\server\share\x (Windows network paths).
                format!("//{r}")
            }
        } else {
            rest.to_string()
        };
        let mut p = percent_decode(&rest);
        // file:///C:/x on Windows-made files.
        let b = p.as_bytes();
        if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
            p.remove(0);
        }
        p
    } else if lower.starts_with("http://") || lower.starts_with("https://") {
        return u.to_string();
    } else {
        u.to_string()
    };
    let is_abs = path.starts_with('/') || path.starts_with('\\') || path.as_bytes().get(1) == Some(&b':');
    if is_abs || path.is_empty() {
        path
    } else {
        base.join(&path).to_string_lossy().into_owned()
    }
}

// ---------------------------------------------------------------------------------------------
// Building the imported project

/// What a file says about a media file.
#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub name: Option<String>,
    pub duration: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub has_video: Option<bool>,
    pub has_audio: Option<bool>,
}

/// Collects the imported project: assets (one per file, plus a sound-only one when a video
/// file's sound is used on its own), video tracks from the bottom up as most formats list them,
/// audio tracks from the top down, and the report.
pub struct Builder {
    pub project: Project,
    pub report: Report,
    pub base: PathBuf,
    /// Video tracks, the bottom one first.
    pub video: Vec<Track>,
    /// Audio tracks, the first (A1) first.
    pub audio: Vec<Track>,
    /// Video clips whose sound settings are known as they are (kimchi's own metadata):
    /// [`merge_sound`] leaves them alone.
    pub keep_sound: std::collections::HashSet<Id>,
    assets: HashMap<(String, bool), Id>,
}

impl Builder {
    /// `file` is the project file read (relative media paths are resolved beside it).
    pub fn new(format: &str, name: &str, file: &Path) -> Builder {
        let mut project = Project::new(if name.trim().is_empty() { "Imported" } else { name.trim() }, ProjectSettings::default());
        project.tracks.clear();
        let base = if file.is_dir() { file.to_path_buf() } else { file.parent().map(Path::to_path_buf).unwrap_or_default() };
        Builder { project, report: Report::new(format), base, video: vec![], audio: vec![], keep_sound: Default::default(), assets: HashMap::new() }
    }

    pub fn settings(&mut self) -> &mut ProjectSettings {
        &mut self.project.settings
    }

    pub fn path(&self, url_or_path: &str) -> String {
        url_to_path(url_or_path, &self.base)
    }

    /// The asset for a media file (made the first time), with what the file says about it.
    pub fn media(&mut self, url_or_path: &str, info: &MediaInfo) -> Id {
        let path = self.path(url_or_path);
        self.asset_for(path, false, info)
    }

    /// The asset for the sound of a file alone (a video file's sound on an audio track): kind
    /// audio, so it goes on audio tracks.
    pub fn sound(&mut self, url_or_path: &str, info: &MediaInfo) -> Id {
        let path = self.path(url_or_path);
        if kind_of(&path) == MediaKind::Audio {
            return self.asset_for(path, false, info);
        }
        self.asset_for(path, true, info)
    }

    fn asset_for(&mut self, path: String, sound_only: bool, info: &MediaInfo) -> Id {
        if let Some(id) = self.assets.get(&(path.clone(), sound_only)).copied() {
            if let Some(a) = self.project.asset_mut(id) {
                merge_meta(&mut a.meta, info);
            }
            return id;
        }
        let kind = if sound_only { MediaKind::Audio } else { kind_of(&path) };
        let mut name = info.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| file_name(&path));
        if sound_only && !name.ends_with("(sound)") {
            name = format!("{name} (sound)");
        }
        let mut meta = MediaMeta { has_video: kind != MediaKind::Audio, has_audio: kind != MediaKind::Image, ..Default::default() };
        merge_meta(&mut meta, info);
        if sound_only {
            meta.has_video = false;
            meta.has_audio = true;
        }
        let asset = Asset {
            id: new_id(),
            name,
            kind,
            path: path.clone(),
            meta,
            origin: AssetOrigin::Imported,
            created_at: chrono::Utc::now(),
            thumbnail: None,
            filmstrip: None,
            waveform: None,
            proxy: None,
            beats: None,
        };
        let id = asset.id;
        self.project.assets.push(asset);
        self.assets.insert((path, sound_only), id);
        id
    }

    pub fn asset_kind(&self, id: Id) -> Option<MediaKind> {
        self.project.asset(id).map(|a| a.kind)
    }

    /// The video asset a sound-only asset was made from (the same file).
    pub fn picture_of(&self, sound: Id) -> Option<Id> {
        let a = self.project.asset(sound)?;
        self.assets.get(&(a.path.clone(), false)).copied().filter(|id| *id != sound)
    }

    /// A new video track above the others.
    pub fn video_track(&mut self, name: &str) -> usize {
        let name = if name.trim().is_empty() { format!("Video {}", self.video.len() + 1) } else { name.trim().to_string() };
        self.video.push(Track::new(TrackKind::Video, name));
        self.video.len() - 1
    }

    /// A new audio track below the others.
    pub fn audio_track(&mut self, name: &str) -> usize {
        let name = if name.trim().is_empty() { format!("Audio {}", self.audio.len() + 1) } else { name.trim().to_string() };
        self.audio.push(Track::new(TrackKind::Audio, name));
        self.audio.len() - 1
    }

    /// The project: tracks in kimchi's order (top video track first), clips sorted with
    /// overlaps moved to tracks of their own, empty tracks dropped (one of each kind kept), the
    /// missing files listed and what came through counted.
    pub fn finish(mut self) -> Imported {
        // Sound files on video tracks play from audio tracks in kimchi; pictures on audio
        // tracks can't play.
        let mut moved_sound = vec![];
        for t in &mut self.video {
            let (sound, pictures): (Vec<Clip>, Vec<Clip>) = std::mem::take(&mut t.clips).into_iter().partition(|c| {
                matches!(c.content, ClipContent::Media { asset_id } if self.project.asset(asset_id).is_some_and(|a| a.kind == MediaKind::Audio))
            });
            t.clips = pictures;
            if !sound.is_empty() {
                moved_sound.push(Track { clips: sound, ..Track::new(TrackKind::Audio, format!("{} sound", t.name)) });
            }
        }
        self.audio.extend(moved_sound);
        for t in &mut self.audio {
            let before = t.clips.len();
            t.clips.retain(|c| match c.content {
                ClipContent::Media { asset_id } => self.project.asset(asset_id).is_some_and(|a| a.kind == MediaKind::Audio),
                _ => false,
            });
            if t.clips.len() != before {
                self.report.dropped("pictures on audio tracks");
            }
        }
        let mut tracks: Vec<Track> = self.video.drain(..).rev().collect();
        tracks.extend(self.audio.drain(..));
        let mut out = vec![];
        let mut moved = 0;
        for t in tracks {
            let (kept, extra) = settle(t);
            moved += extra.iter().map(|t| t.clips.len()).sum::<usize>();
            // Overlapping clips go on a track of their own just above (video) or below (audio).
            if kept.kind == TrackKind::Video {
                out.extend(extra.into_iter().rev());
                out.push(kept);
            } else {
                out.push(kept);
                out.extend(extra);
            }
        }
        if moved > 0 {
            self.report.approximated(format!("{moved} clip{} that overlapped others went on tracks of their own", plural(moved)));
        }
        // Empty tracks are kept when they were named (the person's layout), but there is
        // always at least one video and one audio track, as in a new kimchi project.
        if !out.iter().any(|t| t.kind == TrackKind::Video) {
            out.insert(0, Track::new(TrackKind::Video, "Video 1"));
        }
        if !out.iter().any(|t| t.kind == TrackKind::Audio) {
            out.push(Track::new(TrackKind::Audio, "Audio 1"));
        }
        self.project.tracks = out;
        self.project.markers.sort_by(|a, b| a.time.total_cmp(&b.time));
        // Media no longer used by any clip (a file's sound-only twin left unused) goes.
        let used: Vec<Id> = self.project.clips().filter_map(|(_, c)| c.asset_id()).collect();
        self.project.assets.retain(|a| used.contains(&a.id));
        let mut missing = vec![];
        for a in &self.project.assets {
            if !Path::new(&a.path).exists() && !missing.contains(&a.path) {
                missing.push(a.path.clone());
            }
        }
        self.report.missing_media = missing;
        count_kept(&self.project, &mut self.report);
        Imported { project: self.project, report: self.report }
    }
}

/// Video clips and the sound clips that are their own sound (the same file, at the same time,
/// from the same part of the file, on an audio track: how Premiere, Resolve and Final Cut 7
/// keep a clip's sound): the sound clip goes and the video clip plays its sound, with the
/// sound clip's volume, pan and their keyframes. Sound clips that don't line up exactly (J and
/// L cuts) stay on their tracks. With `mute_unpaired`, video clips whose sound isn't on any
/// audio track are muted: the other app had taken their sound away.
pub fn merge_sound(b: &mut Builder, mute_unpaired: bool) {
    let tol = 0.5 / b.project.settings.fps.max(1.0);
    let mut merged = 0;
    // (video lane, clip index) → its partner, found on the audio lanes.
    for vi in 0..b.video.len() {
        for ci in 0..b.video[vi].clips.len() {
            let v = b.video[vi].clips[ci].clone();
            let Some(asset) = v.asset_id() else { continue };
            if b.asset_kind(asset) != Some(MediaKind::Video) {
                continue;
            }
            let same = |a: &Clip| {
                a.asset_id().is_some_and(|s| s == asset || b.picture_of(s) == Some(asset))
                    && (a.start - v.start).abs() <= tol
                    && (a.duration - v.duration).abs() <= tol
                    && (a.in_point - v.in_point).abs() <= tol * v.speed.max(1.0)
                    && (a.speed - v.speed).abs() < 1e-6
                    && a.reverse == v.reverse
            };
            let mut partners: Vec<(usize, usize)> = vec![];
            for (ai, t) in b.audio.iter().enumerate() {
                for (k, a) in t.clips.iter().enumerate() {
                    if same(a) {
                        partners.push((ai, k));
                    }
                }
            }
            if b.keep_sound.contains(&v.id) {
                continue;
            }
            let Some(&(ai, k)) = partners.first() else {
                if mute_unpaired {
                    b.video[vi].clips[ci].audio.muted = true;
                }
                continue;
            };
            let a = b.audio[ai].clips[k].clone();
            // A sound fade the picture doesn't have would fade the picture too in kimchi: then
            // the two stay apart.
            if (a.fade_in - v.fade_in).abs() > tol || (a.fade_out - v.fade_out).abs() > tol {
                b.video[vi].clips[ci].audio.muted = true;
                continue;
            }
            let muted_track = b.audio[ai].muted;
            let vc = &mut b.video[vi].clips[ci];
            vc.volume = a.volume;
            vc.audio = kimchi_core::ClipAudio { muted: muted_track, ..a.audio.clone() };
            for key in ["volume", "pan"] {
                if let Some(list) = a.keyframes.get(key) {
                    vc.keyframes.insert(key.into(), list.clone());
                }
            }
            merged += 1;
            // Remove every partner (both channels of a dual-mono pair), from the back.
            partners.sort_unstable_by(|x, y| y.cmp(x));
            for (ai, k) in partners {
                b.audio[ai].clips.remove(k);
            }
        }
    }
    let _ = merged;
}

fn merge_meta(meta: &mut MediaMeta, info: &MediaInfo) {
    if let Some(d) = info.duration.filter(|d| d.is_finite() && *d > 0.0) {
        meta.duration = Some(meta.duration.map_or(d, |old| old.max(d)));
    }
    meta.width = meta.width.or(info.width.filter(|w| *w > 0));
    meta.height = meta.height.or(info.height.filter(|h| *h > 0));
    meta.fps = meta.fps.or(info.fps.filter(|f| f.is_finite() && *f > 0.0));
    if let Some(v) = info.has_video {
        meta.has_video = v;
    }
    if let Some(a) = info.has_audio {
        meta.has_audio = a;
    }
}

/// A track's clips sorted, with clips that overlap the ones before them moved to extra tracks.
/// Overlaps shorter than a millisecond are trimmed away instead.
fn settle(mut t: Track) -> (Track, Vec<Track>) {
    t.clips.retain(|c| c.duration.is_finite() && c.start.is_finite());
    for c in &mut t.clips {
        c.start = c.start.max(0.0);
        c.duration = c.duration.max(kimchi_core::MIN_CLIP);
    }
    t.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut lanes: Vec<Track> = vec![];
    let mut kept: Vec<Clip> = vec![];
    for mut c in std::mem::take(&mut t.clips) {
        let end = kept.last().map_or(0.0, Clip::end);
        if c.start < end - 1e-3 {
            let lane = lanes.iter().position(|l| l.clips.last().is_none_or(|p| p.end() <= c.start + 1e-3));
            let i = lane.unwrap_or_else(|| {
                lanes.push(Track { clips: vec![], ..Track::new(t.kind, format!("{} ({})", t.name, lanes.len() + 2)) });
                lanes.len() - 1
            });
            lanes[i].clips.push(c);
            continue;
        }
        if c.start < end {
            let cut = end - c.start;
            c.start = end;
            c.duration = (c.duration - cut).max(kimchi_core::MIN_CLIP);
        }
        kept.push(c);
    }
    t.clips = kept;
    for l in &mut lanes {
        l.hidden = t.hidden;
        l.muted = t.muted;
        l.locked = t.locked;
    }
    (t, lanes)
}

pub fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// "12 clips on 3 tracks", "2 titles", "4 markers"… for what the project has.
pub fn count_kept(p: &Project, report: &mut Report) {
    let used = p.tracks.iter().filter(|t| !t.clips.is_empty()).count();
    let (mut media, mut titles, mut solids, mut motion, mut transitions, mut keyed, mut speed) = (0, 0, 0, 0, 0, 0, 0);
    for (_, c) in p.clips() {
        match c.content {
            ClipContent::Media { .. } => media += 1,
            ClipContent::Text { .. } => titles += 1,
            ClipContent::Solid { .. } => solids += 1,
            _ => motion += 1,
        }
        transitions += c.transition.is_some() as usize;
        keyed += !c.keyframes.is_empty() as usize;
        speed += (c.speed != 1.0 || c.reverse) as usize;
    }
    let all = media + titles + solids + motion;
    report.kept(format!("{all} clip{} on {used} track{}", plural(all), plural(used)));
    for (n, what) in [(media, "media clip"), (titles, "title"), (solids, "colour clip"), (motion, "motion clip"), (transitions, "transition"), (keyed, "animated clip"), (speed, "speed change")] {
        if n > 0 && (what != "media clip" || n != all) {
            report.kept(format!("{n} {what}{}", plural(n)));
        }
    }
    if !p.markers.is_empty() {
        report.kept(format!("{} marker{}", p.markers.len(), plural(p.markers.len())));
    }
    let caps = p.captions().len();
    if caps > 0 {
        report.kept(format!("{caps} caption{}", plural(caps)));
    }
}

// ---------------------------------------------------------------------------------------------
// Writing

/// The project's video tracks from the bottom up (V1 first), as most formats list them.
pub fn video_tracks(p: &Project) -> Vec<&Track> {
    p.tracks.iter().filter(|t| t.kind == TrackKind::Video).rev().collect()
}

/// The project's audio tracks from the top down (A1 first).
pub fn audio_tracks(p: &Project) -> Vec<&Track> {
    p.tracks.iter().filter(|t| t.kind == TrackKind::Audio).collect()
}

/// Does a video-track clip play its file's own sound (formats that keep sound on audio tracks
/// get a linked audio clip for it)?
pub fn plays_sound(p: &Project, c: &Clip) -> bool {
    matches!(c.content, ClipContent::Media { asset_id } if p.asset(asset_id).is_some_and(|a| a.kind == MediaKind::Video && a.meta.has_audio) && !c.audio.muted)
}

/// Every asset with its index, in project order: formats that give files ids number them
/// from this.
pub fn asset_index(p: &Project) -> HashMap<Id, usize> {
    p.assets.iter().enumerate().map(|(i, a)| (a.id, i)).collect()
}

/// Notes in the report for what a written project leaves out, the same for every format that
/// can't carry it.
pub fn note_unwritable(p: &Project, report: &mut Report, what_fits: &Fits) {
    let mut effects = 0;
    let mut plugins = 0;
    let mut motion = 0;
    let mut sound_fx = 0;
    let mut blur = 0;
    for (_, c) in p.clips() {
        if !what_fits.effects && (c.effects.brightness != 0.0 || c.effects.contrast != 0.0 || c.effects.saturation != 0.0 || c.effects.temperature != 0.0 || c.effects.tint != 0.0 || c.effects.vignette != 0.0 || c.effects.sharpen != 0.0 || c.effects.chroma_key.is_some() || c.effects.lut.is_some()) {
            effects += 1;
        }
        if !what_fits.plugins && !c.effects.plugins.is_empty() {
            plugins += 1;
        }
        if matches!(c.content, ClipContent::Motion { .. }) && !what_fits.motion {
            motion += 1;
        }
        if !c.audio.effects.is_empty() {
            sound_fx += 1;
        }
        if c.keyframes.contains_key("blur") {
            blur += 1;
        }
    }
    if effects > 0 {
        report.dropped(format!("colour corrections, keys and LUTs on {effects} clip{}", plural(effects)));
    }
    if plugins > 0 {
        report.dropped(format!("video plugins on {plugins} clip{}", plural(plugins)));
    }
    if motion > 0 {
        report.dropped(format!("{motion} motion clip{} (export with renderMotion to bring them as video files)", plural(motion)));
    }
    if sound_fx > 0 {
        report.dropped(format!("sound effects on {sound_fx} clip{}", plural(sound_fx)));
    }
    if blur > 0 {
        report.dropped(format!("blur on {blur} clip{}", plural(blur)));
    }
    let tracks_fx = p.tracks.iter().filter(|t| !t.mix.effects.is_empty() || !t.mix.sends.is_empty() || t.mix.duck.is_some()).count();
    if tracks_fx > 0 {
        report.dropped(format!("the mixer's effects, sends and ducking on {tracks_fx} track{}", plural(tracks_fx)));
    }
}

/// What a format can carry, for [`note_unwritable`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Fits {
    pub effects: bool,
    pub plugins: bool,
    pub motion: bool,
}

// ---------------------------------------------------------------------------------------------
// Transitions

/// The kimchi transition closest to another app's by its name ("Cross Dissolve", "Dip to
/// Color Dissolve", "Edge Wipe", "Push Slide", "luma"…), and whether it is the same effect
/// (false: the closest kimchi has, say so in the report).
pub fn transition_kind(name: &str) -> (TransitionKind, bool) {
    let n: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    // A kimchi id written back as it was.
    if let Some(k) = kimchi_core::transition::KINDS.iter().find(|k| k.id.eq_ignore_ascii_case(&n)) {
        return (k.kind, true);
    }
    let dir = |l: TransitionKind, r: TransitionKind, u: TransitionKind, d: TransitionKind| {
        if n.contains("fromtop") {
            d
        } else if n.contains("frombottom") {
            u
        } else if n.contains("right") || n.contains("fromleft") {
            r
        } else if n.contains("up") || n.contains("top") {
            u
        } else if n.contains("down") || n.contains("bottom") {
            d
        } else {
            l
        }
    };
    use TransitionKind::*;
    if n.contains("white") || n.contains("flash") {
        return (DipToWhite, n.contains("dip") || n.contains("fade") || n.contains("flash"));
    }
    if n.contains("dip") || n.contains("black") || n.contains("fadetocolor") {
        return (DipToBlack, !n.contains("color") || n.contains("black"));
    }
    if n.contains("push") {
        return (dir(PushLeft, PushRight, PushUp, PushDown), true);
    }
    if n.contains("slide") || n.contains("cover") {
        return (dir(SlideLeft, SlideRight, SlideUp, SlideDown), n.contains("slide"));
    }
    if n.contains("iris") || n.contains("circle") || n.contains("round") {
        return (Iris, n.contains("iris") || n.contains("circle"));
    }
    if n.contains("zoom") || n.contains("scale") {
        return (Zoom, n.contains("zoom"));
    }
    if n.contains("blur") {
        return (Blur, true);
    }
    if n.contains("wipe") || n.contains("barn") || n.contains("clock") || n.contains("luma") && n.contains("wipe") {
        let exact = n == "wipe" || n.contains("edge") || n.contains("linear") || n.contains("left") || n.contains("right") || n.contains("up") || n.contains("down");
        return (dir(WipeLeft, WipeRight, WipeUp, WipeDown), exact);
    }
    let exact = n.contains("dissolve") || n.contains("crossfade") || n == "mix" || n == "luma" || n == "fade" || n.contains("crossfade") || n == "smptedissolve";
    (Dissolve, exact)
}

// ---------------------------------------------------------------------------------------------
// Colours

/// `#rrggbb[aa]` → 0…1 channels.
pub fn rgba(hex: &str) -> [f64; 4] {
    kimchi_core::anim::Rgba::parse(hex).map_or([1.0, 1.0, 1.0, 1.0], |c| c.0.map(|v| v / 255.0))
}

/// 0…1 channels → `#rrggbb` (with `aa` when not opaque).
pub fn hex(c: [f64; 4]) -> String {
    kimchi_core::anim::Rgba(c.map(|v| (v.clamp(0.0, 1.0) * 255.0))).to_hex()
}

/// The colour in `palette` (`(name, "#rrggbb")`) nearest to `hex`.
pub fn nearest<'a>(color: &str, palette: &[(&'a str, &str)]) -> &'a str {
    let c = rgba(color);
    palette
        .iter()
        .min_by(|a, b| dist(c, rgba(a.1)).total_cmp(&dist(c, rgba(b.1))))
        .map_or("", |(n, _)| n)
}

fn dist(a: [f64; 4], b: [f64; 4]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum()
}

/// kimchi's marker colours as the usual named palette (OTIO's names, which Resolve and
/// Premiere colours map onto).
pub const MARKER_COLORS: &[(&str, &str)] = &[
    ("RED", "#ff3b30"),
    ("PINK", "#ff6fae"),
    ("ORANGE", "#ff5a36"),
    ("YELLOW", "#ffcc00"),
    ("GREEN", "#34c759"),
    ("CYAN", "#32d6e6"),
    ("BLUE", "#0a84ff"),
    ("PURPLE", "#8e5cf7"),
    ("MAGENTA", "#e040fb"),
    ("BLACK", "#000000"),
    ("WHITE", "#ffffff"),
];

/// A marker colour from its name in any app's palette (`Red`, `rose`, `Sky`…), as `#rrggbb`.
pub fn marker_color(name: &str) -> String {
    let n = name.trim().to_ascii_uppercase();
    let alias = match n.as_str() {
        "ROSE" | "FUCHSIA" => "PINK",
        "LAVENDER" | "VIOLET" | "IRIS" => "PURPLE",
        "SKY" | "TEAL" => "CYAN",
        "MINT" | "LIME" | "FOREST" => "GREEN",
        "SAND" | "COCOA" | "CREAM" | "MANGO" | "LEMON" => "YELLOW",
        other => other,
    };
    MARKER_COLORS.iter().find(|(k, _)| *k == alias).map_or("#ff5a36", |(_, v)| v).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_paths() {
        let base = Path::new("/projects/cut");
        assert_eq!(url_to_path("file://localhost/Users/a/My%20Clip.mov", base), "/Users/a/My Clip.mov");
        assert_eq!(url_to_path("file:///Users/a/caf%C3%A9.mov", base), "/Users/a/café.mov");
        assert_eq!(url_to_path("file:///C:/Media/x.mov", base), "C:/Media/x.mov");
        assert_eq!(url_to_path("file://server/share/x.mov", base), "//server/share/x.mov");
        assert_eq!(url_to_path("media/x.mov", base), "/projects/cut/media/x.mov");
        assert_eq!(url_to_path("/abs/x.mov", base), "/abs/x.mov");
        assert_eq!(url_to_path("https://example.com/x.mov", base), "https://example.com/x.mov");
        assert_eq!(path_to_url("/Users/a/My Clip #1.mov"), "file:///Users/a/My%20Clip%20%231.mov");
        assert_eq!(path_to_url("C:\\Media\\x.mov"), "file:///C:/Media/x.mov");
        for p in ["/a b/c%d/é.mov", "/x/[1].mp4"] {
            assert_eq!(url_to_path(&path_to_url(p), base), p);
        }
        assert_eq!(percent_decode("bad%zzescape%"), "bad%zzescape%");
    }

    #[test]
    fn kinds_from_names() {
        assert_eq!(kind_of("/a/b.MOV"), MediaKind::Video);
        assert_eq!(kind_of("b.wav"), MediaKind::Audio);
        assert_eq!(kind_of("c.JPG"), MediaKind::Image);
        assert_eq!(kind_of("noext"), MediaKind::Video);
    }

    #[test]
    fn overlaps_go_to_their_own_track() {
        let mut b = Builder::new("test", "x", Path::new("/tmp/x.otio"));
        let a = b.media("/nope/a.mov", &MediaInfo::default());
        let t = b.video_track("V1");
        for (s, d) in [(0.0, 2.0), (1.0, 2.0), (3.5, 1.0), (1.9995, 1.0)] {
            b.video[t].clips.push(Clip::new("c", s, d, ClipContent::Media { asset_id: a }));
        }
        let imp = b.finish();
        let v: Vec<&Track> = imp.project.tracks.iter().filter(|t| t.kind == TrackKind::Video).collect();
        assert_eq!(v.len(), 2, "{:?}", imp.project.tracks.iter().map(|t| (&t.name, t.clips.len())).collect::<Vec<_>>());
        assert_eq!(v[1].clips.len() + v[0].clips.len(), 4);
        assert_eq!(imp.report.missing_media, vec!["/nope/a.mov".to_string()]);
        assert!(imp.report.approximated.iter().any(|s| s.contains("overlapped")));
    }

    #[test]
    fn transition_names() {
        use TransitionKind::*;
        for (name, kind, exact) in [
            ("Cross Dissolve", Dissolve, true),
            ("SMPTE_Dissolve", Dissolve, true),
            ("Dip to Black", DipToBlack, true),
            ("Dip To Color Dissolve", DipToBlack, false),
            ("Dip to White", DipToWhite, true),
            ("Edge Wipe", WipeLeft, true),
            ("Clock Wipe", WipeLeft, false),
            ("Push Up", PushUp, true),
            ("Slide from left", SlideRight, true),
            ("Cross Zoom", Zoom, true),
            ("Iris Round", Iris, true),
            ("Page Peel", Dissolve, false),
            ("wipeDown", WipeDown, true),
        ] {
            assert_eq!(transition_kind(name), (kind, exact), "{name}");
        }
    }

    #[test]
    fn colours() {
        assert_eq!(nearest("#ff3c31", MARKER_COLORS), "RED");
        assert_eq!(marker_color("Sky"), "#32d6e6");
        assert_eq!(hex(rgba("#336699")), "#336699");
    }
}
