//! The compositor: every picture kimchi shows or exports is put together here, one frame at a
//! time, so the preview is exactly what renders.
//!
//! ffmpeg only decodes (each media clip into raw RGBA at the size it is drawn, see [`source`])
//! and, on export, encodes. Each frame starts from the project's background; then every visible
//! clip, bottom track first, is drawn at its keyframed placement (position, scale, rotation,
//! opacity, blur) times its fades: media pictures, solids, titles, 2D motion scenes
//! ([`flat`]) and 3D scenes ([`space`]).
//!
//! A [`Renderer`] is used in one of two ways. Playback and export ask for frames in order
//! ([`Renderer::frame`]): every playing video clip then has a decoder running ahead, started a
//! little before the clip comes up. Scrubbing asks for any time ([`Renderer::still`]): each
//! visible video is decoded at that time, in parallel. Pictures are kept between frames, so a
//! still image or a title that doesn't move is decoded or drawn once.
//!
//! Each clip's effects ([`grade`]) are applied to its picture (media) or its layer (titles,
//! solids, scenes). A transition ([`kimchi_core::transition`]) draws the outgoing and incoming
//! clips into layers of their own and mixes them ([`mix`]).
//!
//! Timing matches the old ffmpeg graph: a clip shows from half a frame before its start to half a
//! frame before its end, and fades are linear in opacity.

pub mod cache;
pub(crate) mod effects2d;
pub(crate) mod flat;
pub mod grade;
pub(crate) mod masks;
pub(crate) mod mix;
pub(crate) mod noise;
pub(crate) mod paint;
pub(crate) mod particles2d;
pub(crate) mod shapeops;
pub(crate) mod source;
pub mod space;
pub(crate) mod textfx;

pub use flat::{hit_test, layer_bounds, layer_transform};
pub use space::Quality;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_core::transition::{self, TransitionKind};
use kimchi_core::{Clip, ClipContent, Effects, Fit, Id, MediaKind, Placement, Project, Scene, TrackKind};
use tiny_skia::{FillRule, Pixmap, PixmapPaint, Transform};

use self::source::VideoStream;
use crate::{MediaResult, Tools};

/// Seconds ahead of a clip's start its decoder is started during playback.
const PREFETCH: f64 = 0.75;
/// Largest side stills and image layers are decoded at.
const MAX_STILL: f64 = 4096.0;

/// Puts frames of a project together at one output size.
pub struct Renderer {
    tools: Tools,
    project: Arc<Project>,
    width: u32,
    height: u32,
    fps: f64,
    strict: bool,
    decode_caps: Option<crate::Caps>,
    /// Output pixels per project pixel.
    sx: f32,
    sy: f32,
    streams: HashMap<StreamKey, VideoStream>,
    /// Frames grabbed for the still being drawn (scrubbing).
    grabbed: HashMap<StreamKey, Arc<Pixmap>>,
    /// Clips shown before their start by a transition: how early, in clip seconds (negative).
    early: HashMap<Id, f64>,
    /// Each still picture's graded copy, by clip, with what it was made from.
    graded: HashMap<Id, (usize, String, Arc<Pixmap>)>,
    /// Quick settings for the preview, the scenes' own render settings for exports.
    quality: Quality,
    /// Whether each rendered motion clip's file still matches its scene.
    current: HashMap<Id, bool>,
}

/// What one track shows at an instant.
#[allow(clippy::large_enum_variant)] // a few per frame; boxing would only add noise
enum Layer {
    Clip(Clip),
    /// A transition: the clip ending at the cut (if any), the incoming clip, the eased progress.
    Mix { from: Option<Clip>, to: Clip, kind: TransitionKind, p: f64 },
}

impl Layer {
    fn clips(&self) -> Vec<&Clip> {
        match self {
            Layer::Clip(c) => vec![c],
            Layer::Mix { from, to, .. } => from.iter().chain([to]).collect(),
        }
    }
}

/// Decoded pictures by file and size.
type Stills = Mutex<HashMap<(PathBuf, u32, u32), Arc<Pixmap>>>;
/// Each title's last picture, with the key of what it showed.
type Titles = Mutex<HashMap<Id, (String, Arc<Pixmap>)>>;

/// Decoded stills, shared by every renderer (the preview, playback and exports draw the same
/// pictures).
fn stills() -> &'static Stills {
    static S: OnceLock<Stills> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// The last picture of each title, by what it showed (titles rarely move).
fn titles() -> &'static Titles {
    static T: OnceLock<Titles> = OnceLock::new();
    T.get_or_init(Default::default)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A decoder's owner: a clip, or an image layer (by its asset reference) inside a motion clip.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StreamKey(Id, Option<String>);

impl Renderer {
    /// A renderer for `project` at `width`×`height` (rounded down to even numbers) and `fps`.
    /// Media are read through their proxies when those exist.
    pub fn new(tools: &Tools, project: &Project, width: u32, height: u32, fps: f64) -> Self {
        let (width, height) = (even(width), even(height));
        Self::with_project(tools, playable(tools, project), width, height, fps, false)
    }

    /// Render original media and report failures instead of omitting clips; motion clips at
    /// their final quality.
    pub fn for_export(tools: &Tools, project: &Project, width: u32, height: u32, fps: f64) -> Self {
        let mut r = Self::with_project(tools, project.clone(), width, height, fps, true);
        r.quality = Quality::Final;
        r
    }

    /// Draw motion clips at this quality (the preview's default is [`Quality::Preview`]).
    pub fn with_quality(mut self, quality: Quality) -> Self {
        self.quality = quality;
        self
    }

    fn with_project(tools: &Tools, project: Project, width: u32, height: u32, fps: f64, strict: bool) -> Self {
        let (width, height) = (even(width), even(height));
        let ps = &project.settings;
        let early = project
            .tracks
            .iter()
            .flat_map(|t| transition::spans(t).into_iter().map(move |s| (t.clips[s.to].id, s.start - t.clips[s.to].start)))
            .filter(|(_, e)| *e < 0.0)
            .collect();
        Self {
            tools: tools.clone(),
            sx: width as f32 / ps.width.max(1) as f32,
            sy: height as f32 / ps.height.max(1) as f32,
            project: Arc::new(project),
            width,
            height,
            fps: if fps.is_finite() && fps > 0.0 { crate::snap_fps(fps) } else { 30.0 },
            strict,
            decode_caps: None,
            streams: HashMap::new(),
            grabbed: HashMap::new(),
            early,
            graded: HashMap::new(),
            quality: Quality::Preview,
            current: HashMap::new(),
        }
    }

    /// Enable hardware decoding for heavy sources, with per-stream software retries.
    pub fn with_hardware_decoding(mut self, caps: crate::Caps) -> Self {
        self.decode_caps = Some(caps);
        self
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Which 3D renderer draws scenes: "gpu (<adapter>)" or "cpu".
    pub fn engine() -> String {
        lock(space::shared()).describe()
    }

    /// The frame at `t` when frames are asked for in order (playback, export).
    pub fn frame(&mut self, t: f64) -> MediaResult<Pixmap> {
        let canvas = self.compose(t, true)?;
        // Decoders for clips that are about to start, so they are ready on time.
        let soon: Vec<(Clip, f64)> = self
            .visible_clips(t + PREFETCH)
            .into_iter()
            .chain(self.visible_clips(t + PREFETCH / 2.0))
            .filter(|c| c.start > t)
            .map(|c| {
                let local = self.early.get(&c.id).copied().unwrap_or(0.0);
                (c, local)
            })
            .collect();
        for (clip, local) in soon {
            let key = StreamKey(clip.id, None);
            if !self.streams.contains_key(&key) {
                let _ = self.start_stream(&clip, key, local);
            }
        }
        Ok(canvas)
    }

    /// A motion clip's scene alone at scene time `t`, transparent around it, at this renderer's
    /// quality (what a render to the timeline stores).
    pub fn scene_frame(&mut self, clip_id: Id, t: f64) -> MediaResult<Pixmap> {
        let shading = if self.quality == Quality::Final { space::viewport::Shading::Rendered } else { space::viewport::Shading::Material };
        let opts = space::viewport::ViewOptions { through_camera: true, shading, ..Default::default() };
        self.scene_view(clip_id, t, None, &opts, None)
    }

    /// The frame a rendered motion clip's file has for timeline time `t` (scene time `st`), when
    /// the file is still right and covers it.
    fn rendered_frame(&mut self, clip: &Clip, t: f64, st: f64, streaming: bool, used: &mut HashSet<StreamKey>) -> Option<Arc<Pixmap>> {
        let r = clip.rendered.as_ref()?;
        let project = self.project.clone();
        let ok = *self.current.entry(clip.id).or_insert_with(|| cache::is_current(&project, clip));
        if !ok || !r.covers(st) {
            return None;
        }
        let path = PathBuf::from(&r.file);
        let file_t = (st - r.from).max(0.0);
        let key = StreamKey(clip.id, Some("@rendered".into()));
        used.insert(key.clone());
        if !streaming {
            return source::grab(&self.tools, &path, Some(file_t), self.width, self.height).ok().map(Arc::new);
        }
        let local = t - clip.start;
        if !self.streams.get(&key).is_some_and(|s| s.serves(local)) {
            let s = VideoStream::start(&self.tools, &path, file_t, clip.speed, self.fps, self.width, self.height, local).ok()?;
            self.streams.insert(key.clone(), s);
        }
        self.streams.get_mut(&key)?.at(local).ok()?
    }

    /// A motion clip's scene on its own at scene time `t`, for the Studio: a 3D scene from the
    /// editor's `view` (or through its camera, with `opts.through_camera`), with the view's
    /// overlays; a 2D scene on its canvas (or one of its compositions, `comp`).
    pub fn scene_view(&mut self, clip_id: Id, t: f64, view: Option<&space::viewport::ViewCamera>, opts: &space::viewport::ViewOptions, comp: Option<&str>) -> MediaResult<Pixmap> {
        let clip = self.project.clip(clip_id).cloned().ok_or_else(|| crate::MediaError::Unsupported(format!("no clip {clip_id}")))?;
        let ClipContent::Motion { scene, .. } = &clip.content else {
            return Err(crate::MediaError::Unsupported("not a motion clip".into()));
        };
        let (w, h) = (self.width as f32, self.height as f32);
        let mut canvas = Pixmap::new(self.width, self.height).expect("non-empty canvas");
        let mut used = HashSet::new();
        match scene {
            Scene::Flat(s) => {
                let base = Transform::from_translate(w / 2.0, h / 2.0).pre_scale(self.sx, self.sy);
                // One output frame lasts `speed` frames of scene time (motion blur spans it).
                let (sx, quality, frame) = (self.sx, self.quality, clip.speed.abs().max(1e-6) / self.fps);
                let eval = kimchi_core::motion::EvalOptions { fps: self.fps, duration: Some(scene_length(&clip)) };
                let shown = match comp.and_then(|c| s.composition(c)) {
                    Some(c) => kimchi_core::Scene2d { background: c.background.clone(), layers: c.layers.clone(), compositions: s.compositions.clone(), ..s.clone() },
                    None => s.clone(),
                };
                let mut pics = ScenePictures { r: self, clip: clip.id, streaming: false, used: &mut used };
                let mut fx = flat::Flat { pictures: &mut pics, scale: sx, quality, frame, eval };
                flat::draw(&mut canvas, &shown, t, base, &mut fx);
            }
            Scene::Space(s) => {
                let (width, height) = (self.width, self.height);
                let frame = clip.speed.abs().max(1e-6) / self.fps;
                let mut pics = ScenePictures { r: self, clip: clip.id, streaming: false, used: &mut used };
                let img = space::viewport::render_view(&mut lock(space::shared()), s, t, frame, width, height, &mut pics, view, opts)?;
                draw_picture(&mut canvas, &img, Transform::identity(), 1.0);
            }
        }
        Ok(canvas)
    }

    /// The Studio's "Rendered" view of a path-traced 3D scene: a picture that gets better with
    /// every [`space::trace::Progressive::add`], from the editor's `view` (or through the scene's
    /// camera). `None` for 2D scenes and scenes the standard engine draws.
    pub fn refining_view(&mut self, clip_id: Id, t: f64, view: Option<&space::viewport::ViewCamera>) -> MediaResult<Option<space::trace::Progressive>> {
        let clip = self.project.clip(clip_id).cloned().ok_or_else(|| crate::MediaError::Unsupported(format!("no clip {clip_id}")))?;
        let ClipContent::Motion { scene: Scene::Space(s), .. } = &clip.content else { return Ok(None) };
        if !s.render.path_traced() {
            return Ok(None);
        }
        let shown = match view {
            Some(v) => v.apply(s),
            None => s.clone(),
        };
        let (width, height) = (self.width, self.height);
        let mut used = HashSet::new();
        let mut pics = ScenePictures { r: self, clip: clip.id, streaming: false, used: &mut used };
        // Only building the frame holds the 3D renderer; the samples are traced without it.
        let p = lock(space::shared()).progressive(&shown, t, width, height, &mut pics);
        Ok(Some(p))
    }

    /// The frame at `t`, wherever the previous one was (scrubbing). Videos are decoded at `t`.
    pub fn still(&mut self, t: f64) -> MediaResult<Pixmap> {
        self.streams.clear();
        self.grab_all(t);
        let canvas = self.compose(t, false);
        self.grabbed.clear();
        canvas
    }

    /// What each visible track shows at `t`, bottom first.
    fn layers(&self, t: f64) -> Vec<Layer> {
        let half = 0.5 / self.fps;
        let shows = |a: f64, b: f64| t >= a - half - 1e-9 && t < b - half - 1e-9;
        let mut out = vec![];
        for track in self.project.tracks.iter().rev().filter(|tr| tr.kind == TrackKind::Video && !tr.hidden) {
            // At most one transition plays on a track at a time (see transition::effective_length).
            if let Some(s) = transition::spans(track).into_iter().find(|s| shows(s.start, s.end)) {
                let to = &track.clips[s.to];
                let tr = to.transition.as_ref().expect("a span has a transition");
                out.push(Layer::Mix { from: s.from.map(|i| track.clips[i].clone()), to: to.clone(), kind: tr.kind, p: s.progress(t, tr) });
                continue;
            }
            out.extend(track.clips.iter().filter(|c| shows(c.start, c.end())).map(|c| Layer::Clip(c.clone())));
        }
        out
    }

    /// Picture clips on screen at `t`, bottom first (both sides of a transition).
    fn visible_clips(&self, t: f64) -> Vec<Clip> {
        self.layers(t).iter().flat_map(|l| l.clips().into_iter().cloned()).collect()
    }

    fn compose(&mut self, t: f64, streaming: bool) -> MediaResult<Pixmap> {
        let mut canvas = Pixmap::new(self.width, self.height).expect("non-empty canvas");
        canvas.fill(paint::color(&self.project.settings.background));
        let mut used = HashSet::new();
        for layer in self.layers(t) {
            let drawn = match &layer {
                Layer::Clip(clip) => self.draw_clip(&mut canvas, clip, t, streaming, &mut used),
                Layer::Mix { from, to, kind, p } => self.draw_mix(&mut canvas, from.as_ref(), to, *kind, *p as f32, t, streaming, &mut used),
            };
            if let Err(e) = drawn {
                if self.strict {
                    return Err(e);
                }
                let names: Vec<&str> = layer.clips().iter().map(|c| c.name.as_str()).collect();
                tracing::warn!(clip = %names.join(" → "), "skipped in this frame: {e}");
            }
        }
        if streaming {
            self.streams.retain(|k, _| used.contains(k) || self.project.clip(k.0).is_some_and(|c| c.start > t));
        }
        Ok(canvas)
    }

    /// Draws both sides of a transition into layers of their own and mixes them onto `canvas`.
    #[allow(clippy::too_many_arguments)]
    fn draw_mix(&mut self, canvas: &mut Pixmap, from: Option<&Clip>, to: &Clip, kind: TransitionKind, p: f32, t: f64, streaming: bool, used: &mut HashSet<StreamKey>) -> MediaResult<()> {
        let (w, h) = (self.width, self.height);
        let blank = || Pixmap::new(w, h).expect("non-empty");
        let mut a = blank();
        if let Some(from) = from {
            self.draw_clip(&mut a, from, t, streaming, used)?;
        }
        let mut b = blank();
        self.draw_clip(&mut b, to, t, streaming, used)?;
        mix::draw(canvas, &a, &b, kind, p, self.sx);
        Ok(())
    }

    fn draw_clip(&mut self, canvas: &mut Pixmap, clip: &Clip, t: f64, streaming: bool, used: &mut HashSet<StreamKey>) -> MediaResult<()> {
        let pl = clip.placement_at(t);
        // A transition shows clips past their edges: they keep their first or last state.
        let alpha = (pl.opacity * fade(clip, t.clamp(clip.start, clip.end()))) as f32;
        let fx = clip.effects_at(t);
        if alpha <= 0.0 || pl.scale_x <= 0.0 || pl.scale_y <= 0.0 {
            return Ok(());
        }
        let (w, h) = (self.width as f32, self.height as f32);
        // Where the clip's centre goes and how it turns.
        let center = Transform::from_translate(w / 2.0 + pl.x as f32 * self.sx, h / 2.0 + pl.y as f32 * self.sy).pre_rotate(pl.rotation as f32);
        let blur = pl.blur as f32 * self.sx;
        match &clip.content {
            ClipContent::Pending { .. } => Ok(()),
            ClipContent::Solid { color } => {
                let path = paint::rect(w, h, 0.0).expect("non-empty");
                let ts = center.pre_scale(pl.scale_x as f32, pl.scale_y as f32);
                let spec = paint::FillSpec::Solid(paint::color(color));
                self.layer(canvas, alpha, blur, &fx, |own, a| own.fill_path(&path, &spec.paint(a), FillRule::Winding, ts, None));
                Ok(())
            }
            ClipContent::Media { asset_id } => {
                let asset = self.project.asset(*asset_id).cloned().ok_or_else(|| crate::MediaError::Unsupported(format!("missing asset {asset_id}")))?;
                if !Path::new(&asset.path).is_file() {
                    return Err(crate::MediaError::Unsupported(format!("missing media file {}", asset.path)));
                }
                let (mw, mh) = self.upright_size(&asset);
                let (dw, dh) = self.decode_size(clip, mw, mh);
                let pic = match asset.kind {
                    MediaKind::Image => self.still_image(Path::new(&asset.path), dw, dh)?,
                    MediaKind::Video if asset.meta.has_video => {
                        let key = StreamKey(clip.id, None);
                        used.insert(key.clone());
                        let local = t - clip.start;
                        let pic = if streaming { self.stream_frame(clip, key, local)? } else { self.grabbed.get(&key).cloned() };
                        match pic {
                            Some(p) => p,
                            // An export decodes the frame on its own: the picture, or ffmpeg's reason.
                            None if self.strict => {
                                let len = asset.duration().unwrap_or(f64::INFINITY);
                                let (dw, dh) = (dw.max(2), dh.max(2));
                                Arc::new(source::grab(&self.tools, Path::new(&asset.path), Some(clip.source_time(t).clamp(0.0, len)), dw, dh)?)
                            }
                            None => return Err(crate::MediaError::Unsupported("no frame".into())),
                        }
                    }
                    _ => return Ok(()),
                };
                let pic = self.grade(clip, pic, &fx, asset.kind == MediaKind::Image);
                let (fw, fh) = fitted(clip.transform.fit, mw, mh, w, h);
                let ts = center
                    .pre_scale((fw * pl.scale_x as f32) / pic.width() as f32, (fh * pl.scale_y as f32) / pic.height() as f32)
                    .pre_translate(-(pic.width() as f32) / 2.0, -(pic.height() as f32) / 2.0);
                self.layer(canvas, alpha, blur, &Effects::default(), |own, a| draw_picture(own, &pic, ts, a));
                Ok(())
            }
            ClipContent::Text { .. } => {
                let pic = self.title(clip, &pl, t);
                self.layer(canvas, alpha, blur, &fx, |own, a| {
                    own.draw_pixmap(0, 0, (*pic).as_ref(), &PixmapPaint { opacity: a, ..PixmapPaint::default() }, Transform::identity(), None)
                });
                Ok(())
            }
            ClipContent::Motion { scene, .. } => {
                let st = clip.scene_time(t);
                let scene = scene.clone();
                let rendered = self.rendered_frame(clip, t, st, streaming, used);
                // Scene pixels map to the canvas through the clip's placement, around the canvas centre.
                let ts = center.pre_scale(pl.scale_x as f32, pl.scale_y as f32).pre_translate(-w / 2.0, -h / 2.0);
                let plain = ts.is_identity() && alpha >= 1.0 && blur <= 0.0 && !fx.is_active();
                let mut own = if plain { None } else { Some(Pixmap::new(self.width, self.height).expect("non-empty")) };
                if let Some(pic) = rendered {
                    let target = own.as_mut().unwrap_or(canvas);
                    draw_picture(target, &pic, Transform::from_scale(w / pic.width() as f32, h / pic.height() as f32), 1.0);
                } else {
                    let target = own.as_mut().unwrap_or(canvas);
                    match &scene {
                        Scene::Flat(s) => {
                            let base = Transform::from_translate(w / 2.0, h / 2.0).pre_scale(self.sx, self.sy);
                            let (sx, quality, frame) = (self.sx, self.quality, clip.speed.abs().max(1e-6) / self.fps);
                            let eval = kimchi_core::motion::EvalOptions { fps: self.fps, duration: Some(scene_length(clip)) };
                            let mut pics = ScenePictures { r: self, clip: clip.id, streaming, used };
                            let mut fx = flat::Flat { pictures: &mut pics, scale: sx, quality, frame, eval };
                            flat::draw(target, s, st, base, &mut fx);
                        }
                        Scene::Space(s) => {
                            let (width, height, quality) = (self.width, self.height, self.quality);
                            // Scene seconds one output frame lasts (for motion blur).
                            let frame = clip.scene_time(t + 1.0 / self.fps) - st;
                            let mut pics = ScenePictures { r: self, clip: clip.id, streaming, used };
                            let img = lock(space::shared()).render_frame(s, st, frame, width, height, &mut pics, quality)?;
                            draw_picture(target, &img, Transform::identity(), 1.0);
                        }
                    }
                }
                if let Some(mut own) = own {
                    if blur > 0.0 {
                        paint::blur(&mut own, blur);
                    }
                    grade::apply(&mut own, &fx, self.sx);
                    let paint = PixmapPaint { opacity: alpha, quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
                    canvas.draw_pixmap(0, 0, own.as_ref(), &paint, ts, None);
                }
                Ok(())
            }
        }
    }

    /// Draws with `f` straight onto the canvas, or onto a layer that is blurred and graded first.
    fn layer(&self, canvas: &mut Pixmap, alpha: f32, blur: f32, fx: &Effects, f: impl FnOnce(&mut Pixmap, f32)) {
        if blur <= 0.0 && !fx.is_active() {
            f(canvas, alpha);
            return;
        }
        let mut own = Pixmap::new(self.width, self.height).expect("non-empty");
        f(&mut own, 1.0);
        paint::blur(&mut own, blur);
        grade::apply(&mut own, fx, self.sx);
        canvas.draw_pixmap(0, 0, own.as_ref(), &PixmapPaint { opacity: alpha, ..PixmapPaint::default() }, Transform::identity(), None);
    }

    /// A media picture with the clip's effects. Stills keep their graded copy while neither the
    /// picture nor the effects change.
    fn grade(&mut self, clip: &Clip, pic: Arc<Pixmap>, fx: &Effects, still: bool) -> Arc<Pixmap> {
        if !fx.is_active() {
            return pic;
        }
        let made_from = Arc::as_ptr(&pic) as usize;
        let key = serde_json::to_string(fx).unwrap_or_default();
        if still
            && let Some((from, k, p)) = self.graded.get(&clip.id)
            && *from == made_from
            && *k == key
        {
            return p.clone();
        }
        let mut out = (*pic).clone();
        grade::apply(&mut out, fx, self.sx);
        let out = Arc::new(out);
        if still {
            self.graded.insert(clip.id, (made_from, key, out.clone()));
        }
        out
    }

    /// A title's picture (canvas-sized), redrawn only when what it shows changed.
    fn title(&mut self, clip: &Clip, pl: &Placement, t: f64) -> Arc<Pixmap> {
        let style = clip.text_at(t).expect("text clip");
        let key = format!(
            "{}|{}|{}|{}|{}|{}|{}x{}",
            serde_json::to_string(&style).unwrap_or_default(),
            pl.x,
            pl.y,
            pl.scale_x,
            pl.scale_y,
            pl.rotation,
            self.width,
            self.height
        );
        if let Some((k, p)) = lock(titles()).get(&clip.id)
            && *k == key
        {
            return p.clone();
        }
        let ps = &self.project.settings;
        let p = Arc::new(crate::text::rasterize_title(&style, pl, ps.width, ps.height, self.sx));
        let mut map = lock(titles());
        if map.len() > 256 {
            map.clear();
        }
        map.insert(clip.id, (key, p.clone()));
        p
    }

    /// A media item's picture size, upright: stills imported before EXIF orientation was read
    /// have it sideways in their metadata.
    fn upright_size(&self, asset: &kimchi_core::Asset) -> (Option<u32>, Option<u32>) {
        if asset.kind == MediaKind::Image
            && let Some((w, h)) = source::dimensions(&self.tools, Path::new(&asset.path))
        {
            return (Some(w), Some(h));
        }
        (asset.meta.width, asset.meta.height)
    }

    /// Size to decode a clip's picture at: its fitted size at the largest scale it reaches,
    /// never more than the source or twice the output.
    fn decode_size(&self, clip: &Clip, sw: Option<u32>, sh: Option<u32>) -> (u32, u32) {
        let (w, h) = (self.width as f32, self.height as f32);
        let (fw, fh) = fitted(clip.transform.fit, sw, sh, w, h);
        let s = clip.max_scale().max(0.01) as f32;
        let (mut dw, mut dh) = (fw * s, fh * s);
        let (src_w, src_h) = (sw.unwrap_or(self.width) as f32, sh.unwrap_or(self.height) as f32);
        let cap = (src_w / dw).min(src_h / dh).min(2.0 * w.max(h) / dw.max(dh)).min(1.0);
        if cap < 1.0 {
            dw *= cap;
            dh *= cap;
        }
        ((dw.round() as u32).max(2), (dh.round() as u32).max(2))
    }

    fn still_image(&mut self, path: &Path, w: u32, h: u32) -> MediaResult<Arc<Pixmap>> {
        let key = (path.to_path_buf(), w, h);
        if let Some(p) = lock(stills()).get(&key) {
            return Ok(p.clone());
        }
        let p = Arc::new(source::grab(&self.tools, path, None, w, h)?);
        let mut map = lock(stills());
        if map.len() > 64 {
            map.clear();
        }
        map.insert(key, p.clone());
        Ok(p)
    }

    fn start_stream(&mut self, clip: &Clip, key: StreamKey, local: f64) -> MediaResult<()> {
        let Some(asset) = clip.asset_id().and_then(|id| self.project.asset(id)).cloned() else { return Ok(()) };
        if asset.kind != MediaKind::Video || !asset.meta.has_video {
            return Ok(());
        }
        let (mw, mh) = self.upright_size(&asset);
        let (dw, dh) = self.decode_size(clip, mw, mh);
        // Before the media's first frame (a transition showing the clip early) the stream starts
        // at that frame, which is held until then.
        let before = asset.duration().map_or(0.0, |len| clip.room(len).0);
        let local = local.max(-before);
        let src = clip.source_time(clip.start + local).max(0.0);
        let path = Path::new(&asset.path);
        let s = if clip.reverse {
            VideoStream::start_reversed(&self.tools, path, src, clip.speed, self.fps, dw, dh, local)?
        } else {
            let decode = self.decode_caps.as_ref().filter(|_| self.streams.len() < crate::accel::MAX_HW_DECODERS)
                .map(|caps| crate::accel::decode_args(caps, &asset.meta)).unwrap_or_default();
            VideoStream::start_with_decode(&self.tools, path, src, clip.speed, self.fps, dw, dh, local, decode)?
        };
        self.streams.insert(key, s);
        Ok(())
    }

    fn stream_frame(&mut self, clip: &Clip, key: StreamKey, local: f64) -> MediaResult<Option<Arc<Pixmap>>> {
        if !self.streams.get(&key).is_some_and(|s| s.serves(local)) {
            self.start_stream(clip, key.clone(), local)?;
        }
        self.streams.get_mut(&key).map(|s| s.at(local)).unwrap_or(Ok(None))
    }

    /// Decodes, in parallel, the frame every visible video clip shows at `t`.
    fn grab_all(&mut self, t: f64) {
        let mut jobs = vec![];
        for clip in self.visible_clips(t) {
            let Some(asset) = clip.asset_id().and_then(|id| self.project.asset(id)) else { continue };
            if asset.kind != MediaKind::Video || !asset.meta.has_video {
                continue;
            }
            let (dw, dh) = self.decode_size(&clip, asset.meta.width, asset.meta.height);
            let len = asset.duration().unwrap_or(f64::INFINITY);
            jobs.push((StreamKey(clip.id, None), PathBuf::from(&asset.path), clip.source_time(t).clamp(0.0, len), dw, dh));
        }
        let tools = &self.tools;
        let results: Vec<(StreamKey, MediaResult<Pixmap>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = jobs
                .into_iter()
                .map(|(key, path, at, w, h)| scope.spawn(move || (key, source::grab(tools, &path, Some(at), w, h))))
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        for (key, r) in results {
            match r {
                Ok(p) => {
                    self.grabbed.insert(key, Arc::new(p));
                }
                Err(e) => tracing::warn!("preview frame: {e}"),
            }
        }
    }

    /// A media item, by id, name or path, for scenes.
    fn resolve(&self, reference: &str) -> Option<(PathBuf, MediaKind, Option<u32>, Option<u32>)> {
        let p = &self.project;
        let asset = reference
            .parse::<Id>()
            .ok()
            .and_then(|id| p.asset(id))
            .or_else(|| p.assets.iter().find(|a| a.name == reference))
            .or_else(|| p.assets.iter().find(|a| a.name.eq_ignore_ascii_case(reference)));
        if let Some(a) = asset {
            let (w, h) = self.upright_size(a);
            return Some((PathBuf::from(&a.path), a.kind, w, h));
        }
        let path = PathBuf::from(reference);
        if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let video = ["mp4", "mov", "webm", "mkv", "m4v", "avi"].contains(&ext.as_str());
            return Some((path, if video { MediaKind::Video } else { MediaKind::Image }, None, None));
        }
        None
    }
}

/// Image layers and textures of a motion clip, from the renderer's decoders and stills.
struct ScenePictures<'a> {
    r: &'a mut Renderer,
    clip: Id,
    streaming: bool,
    used: &'a mut HashSet<StreamKey>,
}

impl ScenePictures<'_> {
    fn get(&mut self, reference: &str, time: f64) -> Option<(Arc<Pixmap>, f64, f64)> {
        let (path, kind, w, h) = self.r.resolve(reference)?;
        let (nw, nh) = match (w, h) {
            (Some(w), Some(h)) => (w as f64, h as f64),
            _ => match source::dimensions(&self.r.tools, &path) {
                Some((w, h)) => (w as f64, h as f64),
                None => return None,
            },
        };
        let k = (MAX_STILL / nw.max(nh)).min(1.0);
        let (dw, dh) = (((nw * k).round() as u32).max(1), ((nh * k).round() as u32).max(1));
        let pic = match kind {
            MediaKind::Video => {
                let key = StreamKey(self.clip, Some(reference.to_string()));
                self.used.insert(key.clone());
                if self.streaming {
                    if !self.r.streams.get(&key).is_some_and(|s| s.serves(time)) {
                        let s = VideoStream::start(&self.r.tools, &path, time.max(0.0), 1.0, self.r.fps, dw, dh, time.max(0.0)).ok()?;
                        self.r.streams.insert(key.clone(), s);
                    }
                    self.r.streams.get_mut(&key)?.at(time).ok()??
                } else {
                    Arc::new(source::grab(&self.r.tools, &path, Some(time.max(0.0)), dw, dh).ok()?)
                }
            }
            _ => self.r.still_image(&path, dw, dh).ok()?,
        };
        Some((pic, nw, nh))
    }
}

impl flat::Pictures for ScenePictures<'_> {
    fn picture(&mut self, asset: &str, time: f64) -> Option<(Arc<Pixmap>, f64, f64)> {
        self.get(asset, time)
    }
}

impl space::Pictures for ScenePictures<'_> {
    fn picture(&mut self, asset: &str, time: f64) -> Option<Arc<Pixmap>> {
        self.get(asset, time).map(|(p, _, _)| p)
    }

    fn path(&self, reference: &str) -> Option<PathBuf> {
        self.r.resolve(reference).map(|(p, ..)| p)
    }
}

/// Draws a picture with a transform: pixel-exact when it is only moved by whole pixels.
fn draw_picture(target: &mut Pixmap, pic: &Pixmap, ts: Transform, alpha: f32) {
    let whole = ts.sx == 1.0 && ts.sy == 1.0 && ts.kx == 0.0 && ts.ky == 0.0 && ts.tx.fract() == 0.0 && ts.ty.fract() == 0.0;
    let quality = if whole { tiny_skia::FilterQuality::Nearest } else { tiny_skia::FilterQuality::Bilinear };
    target.draw_pixmap(0, 0, pic.as_ref(), &PixmapPaint { opacity: alpha, quality, ..PixmapPaint::default() }, ts, None);
}

/// A picture `sw`×`sh` fitted into `w`×`h` per `fit` (the output canvas at scale 1).
fn fitted(fit: Fit, sw: Option<u32>, sh: Option<u32>, w: f32, h: f32) -> (f32, f32) {
    let (iw, ih) = (sw.unwrap_or(w as u32).max(1) as f32, sh.unwrap_or(h as u32).max(1) as f32);
    match fit {
        Fit::Stretch => (w, h),
        Fit::Contain => {
            let s = (w / iw).min(h / ih);
            (iw * s, ih * s)
        }
        Fit::Cover => {
            let s = (w / iw).max(h / ih);
            (iw * s, ih * s)
        }
    }
}

/// How many scene seconds a motion clip shows (what expressions call `duration`).
fn scene_length(c: &Clip) -> f64 {
    (c.scene_time(c.end()) - c.scene_time(c.start)).abs()
}

/// Opacity from the clip's fades at `t` (linear, like ffmpeg's `fade`).
fn fade(c: &Clip, t: f64) -> f64 {
    let local = t - c.start;
    let mut k = 1.0;
    if c.fade_in > 1e-6 {
        k *= (local / c.fade_in).clamp(0.0, 1.0);
    }
    if c.fade_out > 1e-6 {
        k *= ((c.duration - local) / c.fade_out).clamp(0.0, 1.0);
    }
    k
}

/// The project as the renderer reads it: proxies where they exist (not for VP8/VP9 with an
/// alpha channel, which an H.264 proxy loses), and without the media whose files are gone (their
/// clips are skipped instead of failing the whole frame).
pub(crate) fn playable(tools: &Tools, project: &Project) -> Project {
    let mut project = project.clone();
    project.assets.retain_mut(|a| {
        let alpha = || {
            matches!(a.meta.video_codec.as_deref(), Some("vp8" | "vp9"))
                && crate::probe::source(tools, Path::new(&a.path)).is_some_and(|s| s.alpha.is_some())
        };
        if let Some(proxy) = a.proxy.as_deref().filter(|p| Path::new(p).is_file())
            && !alpha()
        {
            a.path = proxy.to_string();
        }
        Path::new(&a.path).is_file()
    });
    project
}

fn even(x: u32) -> u32 {
    (x.max(2) / 2) * 2
}

/// Straight (not premultiplied) RGBA bytes of an opaque frame.
pub fn to_rgba(p: Pixmap) -> Vec<u8> {
    // The background is opaque, so premultiplied = straight.
    p.take()
}

#[allow(dead_code)]
fn _assert_send() {
    fn is_send<T: Send>() {}
    is_send::<Renderer>();
}
