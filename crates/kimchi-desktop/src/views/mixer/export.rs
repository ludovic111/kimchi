//! The export dialog's sound options: for a sound-only export, the file type (WAV, AIFF, FLAC,
//! MP3, AAC, Opus, Ogg Vorbis), the sample rate, the bit depth (lossless) or bitrate (lossy),
//! and stems (one file per track and bus into a folder). They become `export.start`'s
//! `audioFormat`, `sampleRate`, `bitDepth`, `bitrate`, `stems` and `stemsMaster`.

use gpui::{AnyElement, App, Context, Render, WeakEntity, div, prelude::*, px};
use serde_json::{Value, json};

use crate::theme::{ActiveTheme, size as sz};
use crate::ui::{segmented, switch};

/// The sound options as the dialog holds them.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundOptions {
    /// `wav`, `aiff`, `flac`, `mp3`, `aac`, `opus` or `vorbis`.
    pub format: &'static str,
    /// 0: the project's rate.
    pub rate: u32,
    pub bit_depth: u32,
    pub bitrate: u32,
    pub stems: bool,
    pub stems_master: bool,
}

impl Default for SoundOptions {
    fn default() -> Self {
        Self { format: "aac", rate: 0, bit_depth: 24, bitrate: 256, stems: false, stems_master: false }
    }
}

/// (id, label, extension, lossy).
pub const FORMATS: [(&str, &str, &str, bool); 7] = [
    ("wav", "WAV", "wav", false),
    ("aiff", "AIFF", "aiff", false),
    ("flac", "FLAC", "flac", false),
    ("mp3", "MP3", "mp3", true),
    ("aac", "AAC", "m4a", true),
    ("opus", "Opus", "opus", true),
    ("vorbis", "Ogg", "ogg", true),
];

impl SoundOptions {
    fn lossy(&self) -> bool {
        FORMATS.iter().find(|f| f.0 == self.format).is_some_and(|f| f.3)
    }

    /// The file's extension (none for stems: they go in a folder).
    pub fn extension(&self) -> Option<&'static str> {
        (!self.stems).then(|| FORMATS.iter().find(|f| f.0 == self.format).map(|f| f.2).unwrap_or("wav"))
    }

    /// The `export.start` parameters of a sound-only export.
    pub fn params(&self) -> Value {
        let mut p = json!({ "audioFormat": self.format });
        if self.rate > 0 && self.format != "opus" {
            p["sampleRate"] = json!(self.rate);
        }
        if self.lossy() {
            p["bitrate"] = json!(self.bitrate);
        } else {
            p["bitDepth"] = json!(if self.bit_depth == 32 && self.format != "wav" { 24 } else { self.bit_depth });
        }
        if self.stems {
            p["stems"] = json!(true);
            p["stemsMaster"] = json!(self.stems_master);
        }
        p
    }
}

/// The rows of a sound-only export, each change written back into `get(dialog)`.
pub fn rows<D: Render>(o: &SoundOptions, dialog: WeakEntity<D>, get: fn(&mut D) -> &mut SoundOptions, cx: &mut Context<D>) -> Vec<(&'static str, AnyElement)> {
    let t = cx.theme().clone();
    let set = move |f: Box<dyn Fn(&mut SoundOptions)>| {
        let dialog = dialog.clone();
        move |cx: &mut App| {
            dialog
                .update(cx, |d, cx| {
                    f(get(d));
                    cx.notify();
                })
                .ok();
        }
    };
    let mut out: Vec<(&'static str, AnyElement)> = vec![];
    let s1 = set.clone();
    out.push((
        "File type",
        segmented("export-sound-format", FORMATS.iter().map(|f| (f.0, f.1.into())).collect(), o.format, move |v, _, cx| {
            let v = *v;
            s1(Box::new(move |o| o.format = v))(cx)
        }, cx)
        .into_any_element(),
    ));
    if o.format != "opus" {
        let s2 = set.clone();
        out.push((
            "Sample rate",
            segmented("export-sound-rate", vec![(0u32, "Project".into()), (44_100, "44.1 kHz".into()), (48_000, "48 kHz".into()), (96_000, "96 kHz".into())], o.rate, move |v, _, cx| {
                let v = *v;
                s2(Box::new(move |o| o.rate = v))(cx)
            }, cx)
            .into_any_element(),
        ));
    }
    if o.lossy() {
        let s3 = set.clone();
        out.push((
            "Bitrate",
            segmented("export-sound-bitrate", vec![(128u32, "128".into()), (192, "192".into()), (256, "256".into()), (320, "320 kbit/s".into())], o.bitrate, move |v, _, cx| {
                let v = *v;
                s3(Box::new(move |o| o.bitrate = v))(cx)
            }, cx)
            .into_any_element(),
        ));
    } else {
        let s3 = set.clone();
        let mut depths = vec![(16u32, "16-bit".into()), (24, "24-bit".into())];
        if o.format == "wav" {
            depths.push((32, "32-bit float".into()));
        }
        out.push((
            "Bit depth",
            segmented("export-sound-depth", depths, o.bit_depth.min(if o.format == "wav" { 32 } else { 24 }), move |v, _, cx| {
                let v = *v;
                s3(Box::new(move |o| o.bit_depth = v))(cx)
            }, cx)
            .into_any_element(),
        ));
    }
    let s4 = set.clone();
    let s5 = set;
    let stems = o.stems;
    out.push((
        "Stems",
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(switch("export-stems", "One file per track and bus, in a folder", stems, move |v, _, cx| s4(Box::new(move |o| o.stems = v))(cx), cx))
            .when(stems, |d| {
                d.child(switch("export-stems-master", "Through the master's effects and limiter", o.stems_master, move |v, _, cx| s5(Box::new(move |o| o.stems_master = v))(cx), cx))
                    .child(div().text_size(px(sz::XS)).text_color(t.text_2).child("Each stem is the track as the mix hears it: its clips, effects, fader and pan."))
            })
            .into_any_element(),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_follow_the_file_type() {
        let mut o = SoundOptions { format: "flac", rate: 96_000, ..Default::default() };
        assert_eq!(o.params(), json!({ "audioFormat": "flac", "sampleRate": 96000, "bitDepth": 24 }));
        o.format = "mp3";
        assert_eq!(o.params()["bitrate"], 256);
        assert_eq!(o.extension(), Some("mp3"));
        o.stems = true;
        assert_eq!(o.extension(), None);
        o.format = "opus";
        assert!(o.params().get("sampleRate").is_none());
    }
}
