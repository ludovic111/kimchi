//! What each export format is: its id in commands, label, file extension, group in the export
//! dialog, the ffmpeg encoders it needs, and a note for people.

use serde::Serialize;

use super::ExportFormat;
use crate::Caps;

/// Where a format shows in the export dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FormatGroup {
    /// Web and social: files that play everywhere.
    Web,
    /// Editing and finishing: for other editors, colour and archives.
    Editing,
    /// Image sequences: a folder of numbered frames.
    Sequences,
    /// Sound only.
    Sound,
}

impl FormatGroup {
    pub const ALL: [FormatGroup; 4] = [Self::Web, Self::Editing, Self::Sequences, Self::Sound];

    pub fn label(self) -> &'static str {
        match self {
            Self::Web => "Web and social",
            Self::Editing => "Editing and finishing",
            Self::Sequences => "Image sequences",
            Self::Sound => "Sound",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatInfo {
    pub format: ExportFormat,
    /// The id commands take (`export.start format`).
    pub id: &'static str,
    /// Short name ("ProRes 4444").
    pub label: &'static str,
    /// What is inside ("ProRes 4444 + PCM in MOV").
    pub codecs: &'static str,
    /// The file extension (a sequence's frames).
    pub extension: &'static str,
    pub group: FormatGroup,
    /// Other extensions the path may end in (MXF for DNxHR, WebM for AV1).
    pub also: &'static [&'static str],
    /// It keeps transparency when the project's background is transparent.
    pub alpha: bool,
    /// What it is for, in a few words.
    pub note: &'static str,
    /// ffmpeg encoders that can write its picture (any one is enough; empty: sound only).
    pub encoders: &'static [&'static str],
}

const fn info(
    format: ExportFormat, id: &'static str, label: &'static str, codecs: &'static str, extension: &'static str, group: FormatGroup,
    also: &'static [&'static str], alpha: bool, note: &'static str, encoders: &'static [&'static str],
) -> FormatInfo {
    FormatInfo { format, id, label, codecs, extension, group, also, alpha, note, encoders }
}

use ExportFormat as F;
use FormatGroup as G;

const PRORES: &[&str] = &["prores_ks", "prores", "prores_videotoolbox"];
const DNXHR: &[&str] = &["dnxhd"];

/// Every format, in the order the dialog and `export.formats` list them.
static FORMATS: &[FormatInfo] = &[
    info(F::Mp4, "mp4", "MP4", "H.264 + AAC in MP4", "mp4", G::Web, &[], false, "Plays everywhere: phones, browsers, every site.", &["libx264", "h264_videotoolbox", "h264_mf", "libopenh264", "mpeg4"]),
    info(F::Hevc, "hevc", "HEVC", "H.265 + AAC in MP4", "mp4", G::Web, &[], false, "About half the size of MP4 at the same quality; newer devices.", &["libx265", "hevc_videotoolbox", "hevc_mf"]),
    info(F::Av1, "av1", "AV1", "AV1 + AAC in MP4 (or + Opus in .webm)", "mp4", G::Web, &["webm"], false, "The smallest files for YouTube and the web; slow on the CPU, fast on recent GPUs.", &["libsvtav1", "libaom-av1", "librav1e"]),
    info(F::Webm, "webm", "WebM", "VP9 + Opus in WebM", "webm", G::Web, &[], false, "For web pages.", &["libvpx-vp9", "libvpx", "libsvtav1", "libaom-av1"]),
    info(F::Gif, "gif", "GIF", "Animated GIF", "gif", G::Web, &[], false, "Loops, no sound.", &["gif"]),
    info(F::Prores, "prores", "ProRes 422 HQ", "ProRes 422 HQ + PCM in MOV", "mov", G::Editing, &[], false, "To keep editing in Premiere Pro, Final Cut Pro or Resolve.", PRORES),
    info(F::Prores4444, "prores4444", "ProRes 4444", "ProRes 4444 + PCM in MOV", "mov", G::Editing, &[], true, "Finishing and graphics: keeps transparency.", &["prores_ks", "prores"]),
    info(F::ProresLt, "prores_lt", "ProRes 422 LT", "ProRes 422 LT + PCM in MOV", "mov", G::Editing, &[], false, "Lighter ProRes for editing.", PRORES),
    info(F::ProresProxy, "prores_proxy", "ProRes Proxy", "ProRes 422 Proxy + PCM in MOV", "mov", G::Editing, &[], false, "Small ProRes for offline editing.", PRORES),
    info(F::DnxhrLb, "dnxhr_lb", "DNxHR LB", "DNxHR LB + PCM in MOV or MXF", "mov", G::Editing, &["mxf"], false, "Avid's offline quality: small files for editing.", DNXHR),
    info(F::DnxhrSq, "dnxhr_sq", "DNxHR SQ", "DNxHR SQ + PCM in MOV or MXF", "mov", G::Editing, &["mxf"], false, "Avid's standard quality.", DNXHR),
    info(F::DnxhrHq, "dnxhr_hq", "DNxHR HQ", "DNxHR HQ + PCM in MOV or MXF", "mov", G::Editing, &["mxf"], false, "For Media Composer, Premiere Pro and Resolve (8-bit).", DNXHR),
    info(F::DnxhrHqx, "dnxhr_hqx", "DNxHR HQX", "DNxHR HQX (10-bit) + PCM in MOV or MXF", "mov", G::Editing, &["mxf"], false, "10-bit, for grading.", DNXHR),
    info(F::Dnxhr444, "dnxhr_444", "DNxHR 444", "DNxHR 444 (10-bit 4:4:4) + PCM in MOV or MXF", "mov", G::Editing, &["mxf"], false, "Full colour detail, for finishing.", DNXHR),
    info(F::H264Mov, "mov", "H.264 MOV", "H.264 + AAC in MOV", "mov", G::Editing, &[], false, "QuickTime H.264, for apps that want a .mov.", &["libx264", "h264_videotoolbox", "h264_mf", "libopenh264", "mpeg4"]),
    info(F::Ffv1, "ffv1", "FFV1", "FFV1 (lossless) + FLAC in MKV", "mkv", G::Editing, &[], true, "Lossless, for archiving; plays in VLC, Resolve and kimchi.", &["ffv1"]),
    info(F::PngSequence, "png", "PNG", "8-bit PNG frames", "png", G::Sequences, &[], true, "A folder of frames, with transparency; the sound as a WAV beside them.", &["png"]),
    info(F::Png16Sequence, "png16", "PNG 16-bit", "16-bit PNG frames", "png", G::Sequences, &[], true, "16-bit frames for compositing apps.", &["png"]),
    info(F::TiffSequence, "tiff", "TIFF", "TIFF frames (Deflate)", "tif", G::Sequences, &[], true, "For print and photo apps.", &["tiff"]),
    info(F::JpegSequence, "jpeg", "JPEG", "JPEG frames", "jpg", G::Sequences, &[], false, "Small frames for previews and the web.", &["mjpeg"]),
    info(F::ExrSequence, "exr", "OpenEXR", "OpenEXR frames (half float, linear, ZIP)", "exr", G::Sequences, &[], true, "For Nuke, After Effects, Fusion and Blender: linear light.", &["exr"]),
    info(F::Audio, "audio", "Audio", "AAC in M4A, or WAV, AIFF, FLAC, MP3, Opus, Vorbis", "m4a", G::Sound, &[], false, "The mix, or one file per track.", &[]),
    info(F::Wav, "wav", "WAV", "24-bit WAV", "wav", G::Sound, &[], false, "The mix, uncompressed.", &[]),
];

impl ExportFormat {
    /// Every format, in the dialog's order.
    pub fn all() -> impl Iterator<Item = ExportFormat> {
        FORMATS.iter().map(|f| f.format)
    }

    pub fn info(self) -> &'static FormatInfo {
        FORMATS.iter().find(|f| f.format == self).expect("every format is listed")
    }

    pub fn id(self) -> &'static str {
        self.info().id
    }

    /// Reads a format id (and a few other names people use: `prores_hq`, `h264`, `dnxhd`…).
    pub fn parse(s: &str) -> Result<ExportFormat, String> {
        let k = s.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        let alias = match k.as_str() {
            "h264" | "mpeg4" => Some(F::Mp4),
            "h265" | "x265" => Some(F::Hevc),
            "prores_hq" | "prores422" | "prores_422_hq" => Some(F::Prores),
            "prores_4444" | "4444" => Some(F::Prores4444),
            "dnxhd" | "dnxhr" => Some(F::DnxhrHq),
            "dnxhr444" => Some(F::Dnxhr444),
            "h264_mov" | "quicktime" => Some(F::H264Mov),
            "png_sequence" | "png8" => Some(F::PngSequence),
            "png16_sequence" => Some(F::Png16Sequence),
            "tif" | "tiff_sequence" => Some(F::TiffSequence),
            "jpg" | "jpeg_sequence" => Some(F::JpegSequence),
            "openexr" | "exr_sequence" => Some(F::ExrSequence),
            "mkv" | "archive" => Some(F::Ffv1),
            "m4a" | "aac" => Some(F::Audio),
            _ => None,
        };
        if let Some(f) = FORMATS.iter().find(|f| f.id == k).map(|f| f.format).or(alias) {
            return Ok(f);
        }
        let ids: Vec<&str> = FORMATS.iter().map(|f| f.id).collect();
        let hint = kimchi_core::closest(&k, &ids).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        Err(format!("Unknown format \"{s}\".{hint} Formats: {} (export.formats says what each is).", ids.join(", ")))
    }

    /// Formats with no picture.
    pub fn is_audio_only(self) -> bool {
        matches!(self, F::Audio | F::Wav)
    }

    /// A folder of numbered frames.
    pub fn is_sequence(self) -> bool {
        self.info().group == G::Sequences
    }

    pub fn is_prores(self) -> bool {
        matches!(self, F::Prores | F::Prores4444 | F::ProresLt | F::ProresProxy)
    }

    pub fn is_dnxhr(self) -> bool {
        matches!(self, F::DnxhrLb | F::DnxhrSq | F::DnxhrHq | F::DnxhrHqx | F::Dnxhr444)
    }

    /// Whether this ffmpeg has an encoder for it; else what to do.
    pub fn check(self, caps: &Caps) -> Result<(), String> {
        let i = self.info();
        if i.encoders.is_empty() || i.encoders.iter().any(|e| caps.has(e)) {
            return Ok(());
        }
        Err(format!(
            "This ffmpeg can't write {} (it has none of the {} encoders). Use another format, or point kimchi at a full ffmpeg build with KIMCHI_FFMPEG.",
            i.label,
            i.encoders.join(", ")
        ))
    }
}

/// Every format's description.
pub fn infos() -> &'static [FormatInfo] {
    FORMATS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse_and_serde_agree() {
        for f in ExportFormat::all() {
            assert_eq!(ExportFormat::parse(f.id()).unwrap(), f);
            assert_eq!(serde_json::to_value(f).unwrap(), serde_json::json!(f.id()), "{f:?}");
        }
        assert_eq!(ExportFormat::parse("ProRes 4444").unwrap(), F::Prores4444);
        assert!(ExportFormat::parse("prores4445").unwrap_err().contains("prores4444"));
        assert_eq!(ExportFormat::all().count(), FORMATS.len());
        assert!(F::Wav.check(&Caps::new(9, ["aac"])).is_ok());
        assert!(F::ExrSequence.check(&Caps::new(9, ["png"])).unwrap_err().contains("OpenEXR"));
    }
}
