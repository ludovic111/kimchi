//! Transitions between clips. A clip's `transition` is how it comes in at its start.
//!
//! When the clip before it on the same track ends exactly where it starts (a cut), the
//! transition is centred on the cut: the outgoing clip plays on for half the length past its
//! end and the incoming one starts half the length early, both using the media beyond their
//! edges (or holding their first/last frame when there is none). Clips never overlap on the
//! timeline, so nothing moves. With no clip right before it, the clip transitions in over what
//! is below it, starting at its start.
//!
//! The sound of the two clips crossfades over the same span.

use serde::{Deserialize, Serialize};

use crate::anim::Easing;
use crate::model::{Clip, Track};

/// How close two clip edges must be to count as a cut.
pub const CUT_TOLERANCE: f64 = 1e-3;
/// Default transition length in seconds.
pub const DEFAULT_LENGTH: f64 = 0.8;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum TransitionKind {
    /// Crossfade.
    Dissolve,
    /// Out to black, then in from black.
    DipToBlack,
    DipToWhite,
    /// The new clip is uncovered by a moving edge (the direction the edge travels).
    WipeLeft,
    WipeRight,
    WipeUp,
    WipeDown,
    /// The new clip slides in over the old one.
    SlideLeft,
    SlideRight,
    SlideUp,
    SlideDown,
    /// The new clip pushes the old one out of the frame.
    PushLeft,
    PushRight,
    PushUp,
    PushDown,
    /// The new clip grows from the centre while fading in.
    Zoom,
    /// The new clip appears in a circle that opens from the centre.
    Iris,
    /// Both blur through the dissolve.
    Blur,
}

pub struct KindInfo {
    pub kind: TransitionKind,
    pub id: &'static str,
    pub label: &'static str,
    pub doc: &'static str,
}

pub const KINDS: &[KindInfo] = &[
    KindInfo { kind: TransitionKind::Dissolve, id: "dissolve", label: "Dissolve", doc: "Crossfade from one clip to the next." },
    KindInfo { kind: TransitionKind::DipToBlack, id: "dipToBlack", label: "Dip to black", doc: "Fades out to black, then in from black." },
    KindInfo { kind: TransitionKind::DipToWhite, id: "dipToWhite", label: "Dip to white", doc: "Fades out to white, then in from white (a flash)." },
    KindInfo { kind: TransitionKind::WipeLeft, id: "wipeLeft", label: "Wipe left", doc: "A soft edge travels right to left, uncovering the next clip." },
    KindInfo { kind: TransitionKind::WipeRight, id: "wipeRight", label: "Wipe right", doc: "A soft edge travels left to right." },
    KindInfo { kind: TransitionKind::WipeUp, id: "wipeUp", label: "Wipe up", doc: "A soft edge travels bottom to top." },
    KindInfo { kind: TransitionKind::WipeDown, id: "wipeDown", label: "Wipe down", doc: "A soft edge travels top to bottom." },
    KindInfo { kind: TransitionKind::SlideLeft, id: "slideLeft", label: "Slide left", doc: "The next clip slides in from the right, over the old one." },
    KindInfo { kind: TransitionKind::SlideRight, id: "slideRight", label: "Slide right", doc: "The next clip slides in from the left." },
    KindInfo { kind: TransitionKind::SlideUp, id: "slideUp", label: "Slide up", doc: "The next clip slides in from below." },
    KindInfo { kind: TransitionKind::SlideDown, id: "slideDown", label: "Slide down", doc: "The next clip slides in from above." },
    KindInfo { kind: TransitionKind::PushLeft, id: "pushLeft", label: "Push left", doc: "The next clip pushes the old one out to the left." },
    KindInfo { kind: TransitionKind::PushRight, id: "pushRight", label: "Push right", doc: "The next clip pushes the old one out to the right." },
    KindInfo { kind: TransitionKind::PushUp, id: "pushUp", label: "Push up", doc: "The next clip pushes the old one out of the top." },
    KindInfo { kind: TransitionKind::PushDown, id: "pushDown", label: "Push down", doc: "The next clip pushes the old one out of the bottom." },
    KindInfo { kind: TransitionKind::Zoom, id: "zoom", label: "Zoom", doc: "The next clip grows from 60% while fading in." },
    KindInfo { kind: TransitionKind::Iris, id: "iris", label: "Iris", doc: "The next clip appears in a circle opening from the centre." },
    KindInfo { kind: TransitionKind::Blur, id: "blur", label: "Blur", doc: "Both clips blur through a dissolve." },
];

impl TransitionKind {
    pub fn info(self) -> &'static KindInfo {
        KINDS.iter().find(|k| k.kind == self).expect("every kind is listed")
    }

    pub fn id(self) -> &'static str {
        self.info().id
    }

    pub fn label(self) -> &'static str {
        self.info().label
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let key: String = s.chars().filter(|c| !matches!(c, '-' | '_' | ' ')).collect::<String>().to_ascii_lowercase();
        let alias = match key.as_str() {
            "crossfade" | "fade" | "crossdissolve" | "mix" => Some(TransitionKind::Dissolve),
            "black" | "fadetoblack" | "diptoblack" => Some(TransitionKind::DipToBlack),
            "white" | "flash" | "fadetowhite" => Some(TransitionKind::DipToWhite),
            "wipe" => Some(TransitionKind::WipeLeft),
            "slide" => Some(TransitionKind::SlideLeft),
            "push" => Some(TransitionKind::PushLeft),
            "circle" | "irisopen" => Some(TransitionKind::Iris),
            _ => None,
        };
        if let Some(k) = alias.or_else(|| KINDS.iter().find(|k| k.id.eq_ignore_ascii_case(&key)).map(|k| k.kind)) {
            return Ok(k);
        }
        let ids: Vec<&str> = KINDS.iter().map(|k| k.id).collect();
        let hint = crate::closest(s, &ids).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        Err(format!("Unknown transition `{s}`.{hint} Transitions: {}.", ids.join(", ")))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transition {
    pub kind: TransitionKind,
    /// Seconds.
    pub duration: f64,
    /// Shape of the progress; written like keyframe easings (default easeInOut, a sine).
    #[serde(default = "default_easing")]
    pub easing: Easing,
    /// A transition plugin drawn instead of `kind` (`kind` is drawn when the plugin isn't on this
    /// computer). Its parameters don't take keyframes; its slot id is `transition`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<crate::effects::PluginEffect>,
}

fn default_easing() -> Easing {
    Easing::Curve(crate::anim::Curve::Sine, crate::anim::Mode::InOut)
}

impl Transition {
    pub fn new(kind: TransitionKind, duration: f64) -> Self {
        Self { kind, duration, easing: default_easing(), plugin: None }
    }
}

/// A transition in play on a track: the incoming clip, the outgoing one (if the transition
/// sits on a cut) and the span it covers on the timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    /// Index of the incoming clip in the track.
    pub to: usize,
    /// Index of the clip that ends at the cut.
    pub from: Option<usize>,
    pub start: f64,
    pub end: f64,
}

impl Span {
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }

    /// Eased progress at timeline time `t` (0 = all the old clip, 1 = all the new one).
    pub fn progress(&self, t: f64, tr: &Transition) -> f64 {
        let x = ((t - self.start) / self.duration().max(1e-9)).clamp(0.0, 1.0);
        tr.easing.apply(x)
    }
}

/// The span of the transition into `track.clips[i]`, if it has one (see [`effective_length`]).
pub fn span(track: &Track, i: usize) -> Option<Span> {
    let to = track.clips.get(i)?;
    let tr = to.transition.as_ref()?;
    let from = i.checked_sub(1).filter(|&j| (track.clips[j].end() - to.start).abs() <= CUT_TOLERANCE);
    let len = effective_length(tr.duration, to, from.map(|j| &track.clips[j]));
    if len <= 1e-6 {
        return None;
    }
    Some(match from {
        Some(_) => Span { to: i, from, start: to.start - len / 2.0, end: to.start + len / 2.0 },
        None => Span { to: i, from: None, start: to.start, end: to.start + len },
    })
}

/// A transition's length once bounded by the clips: on a cut at most the shorter clip, else at
/// most half the clip. Each clip then gives at most half of itself to each of its two ends, so
/// the transitions on one track never overlap.
pub fn effective_length(wanted: f64, to: &Clip, from: Option<&Clip>) -> f64 {
    let bound = match from {
        Some(f) => f.duration.min(to.duration),
        None => to.duration / 2.0,
    };
    wanted.max(0.0).min(bound)
}

/// Every transition span on a track, by incoming clip.
pub fn spans(track: &Track) -> Vec<Span> {
    (0..track.clips.len()).filter_map(|i| span(track, i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ClipContent, TrackKind};

    fn solid(start: f64, d: f64) -> Clip {
        Clip::new("s", start, d, ClipContent::Solid { color: "#ff0000".into() })
    }

    #[test]
    fn spans_centre_on_cuts() {
        let mut t = Track::new(TrackKind::Video, "V");
        t.clips = vec![solid(0.0, 2.0), solid(2.0, 2.0), solid(5.0, 1.0)];
        t.clips[1].transition = Some(Transition::new(TransitionKind::Dissolve, 1.0));
        t.clips[2].transition = Some(Transition::new(TransitionKind::WipeLeft, 3.0));
        let s = span(&t, 1).unwrap();
        assert_eq!((s.from, s.start, s.end), (Some(0), 1.5, 2.5));
        // No clip right before: starts at the clip, at most half its length.
        let s = span(&t, 2).unwrap();
        assert_eq!((s.from, s.start, s.end), (None, 5.0, 5.5));
        assert!(span(&t, 0).is_none());
    }

    #[test]
    fn kinds_parse_with_aliases_and_hints() {
        assert_eq!(TransitionKind::parse("crossfade"), Ok(TransitionKind::Dissolve));
        assert_eq!(TransitionKind::parse("dip-to-white"), Ok(TransitionKind::DipToWhite));
        assert_eq!(TransitionKind::parse("pushUp"), Ok(TransitionKind::PushUp));
        assert!(TransitionKind::parse("dissolv").unwrap_err().contains("dissolve"));
    }

    #[test]
    fn serialises_with_the_easing_as_a_name() {
        let tr = Transition::new(TransitionKind::DipToBlack, 0.5);
        let json = serde_json::to_value(&tr).unwrap();
        assert_eq!(json["kind"], "dipToBlack");
        let back: Transition = serde_json::from_value(json).unwrap();
        assert_eq!(back, tr);
    }
}
