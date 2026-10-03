//! Hardware video: the GPU and media-engine encoders this computer can really use, the options
//! that drive them, and hardware decoding for heavy sources.
//!
//! ffmpeg lists the encoders it was built with, not the ones the machine has: the Windows build
//! carries NVENC, AMF, Quick Sync and Media Foundation whatever the GPU is, the Linux one VA-API
//! too. [`Hardware::detect`] encodes a few frames with each candidate once per process and keeps
//! the ones that worked, with the richest option set that worked (constant quality where the
//! driver takes it, a bitrate otherwise). An export that still fails on the GPU is redone on the
//! CPU ([`crate::export::export`]), so a flaky driver costs time, never the export.
//!
//! Order matters on machines with two GPUs (a laptop with Intel graphics and an NVIDIA card):
//! vendor APIs first, best first, then the generic ones (VA-API, Media Foundation).
//! `KIMCHI_HARDWARE=0` turns all of this off.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use kimchi_core::MediaMeta;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::export::Quality;
use crate::{Caps, Tools, process};

/// Which encoders an export may use.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EncoderChoice {
    /// The GPU or media engine when this computer has one for the format, the CPU otherwise.
    #[default]
    Auto,
    /// Only the GPU or media engine (an error when there is none); WebM may then be AV1.
    Hardware,
    /// Only the CPU (x264, x265, ProRes, libvpx): slower, the smallest files at a given quality.
    Software,
}

/// What a hardware encoder produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    H264,
    Hevc,
    Prores,
    Vp9,
    Av1,
}

/// The API a hardware encoder goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Api {
    VideoToolbox,
    Nvenc,
    Amf,
    Qsv,
    Vaapi,
    MediaFoundation,
}

impl Api {
    pub fn label(self) -> &'static str {
        match self {
            Api::VideoToolbox => "Apple VideoToolbox",
            Api::Nvenc => "NVIDIA NVENC",
            Api::Amf => "AMD AMF",
            Api::Qsv => "Intel Quick Sync",
            Api::Vaapi => "VA-API",
            Api::MediaFoundation => "Media Foundation",
        }
    }
}

/// One ffmpeg hardware encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HwEncoder {
    pub name: &'static str,
    pub codec: Codec,
    pub api: Api,
}

const fn enc(name: &'static str, codec: Codec, api: Api) -> HwEncoder {
    HwEncoder { name, codec, api }
}

/// Every hardware encoder kimchi knows how to drive, best first.
pub const CANDIDATES: &[HwEncoder] = &[
    enc("h264_videotoolbox", Codec::H264, Api::VideoToolbox),
    enc("hevc_videotoolbox", Codec::Hevc, Api::VideoToolbox),
    enc("prores_videotoolbox", Codec::Prores, Api::VideoToolbox),
    enc("h264_nvenc", Codec::H264, Api::Nvenc),
    enc("hevc_nvenc", Codec::Hevc, Api::Nvenc),
    enc("av1_nvenc", Codec::Av1, Api::Nvenc),
    enc("h264_amf", Codec::H264, Api::Amf),
    enc("hevc_amf", Codec::Hevc, Api::Amf),
    enc("av1_amf", Codec::Av1, Api::Amf),
    enc("h264_qsv", Codec::H264, Api::Qsv),
    enc("hevc_qsv", Codec::Hevc, Api::Qsv),
    enc("vp9_qsv", Codec::Vp9, Api::Qsv),
    enc("av1_qsv", Codec::Av1, Api::Qsv),
    enc("h264_vaapi", Codec::H264, Api::Vaapi),
    enc("hevc_vaapi", Codec::Hevc, Api::Vaapi),
    enc("vp9_vaapi", Codec::Vp9, Api::Vaapi),
    enc("av1_vaapi", Codec::Av1, Api::Vaapi),
    enc("h264_mf", Codec::H264, Api::MediaFoundation),
    enc("hevc_mf", Codec::Hevc, Api::MediaFoundation),
];

/// A hardware encoder that encoded frames on this machine, and how it was driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Verified {
    #[serde(flatten)]
    pub encoder: HwEncoder,
    /// Constant-quality options worked (otherwise a bitrate is used).
    pub constant_quality: bool,
}

/// The hardware encoders that work here.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Hardware {
    /// Best first.
    pub encoders: Vec<Verified>,
    /// The VA-API render node the VA-API encoders opened (`/dev/dri/renderD128`).
    pub vaapi_device: Option<String>,
}

/// Seconds a trial encode may take before the driver is considered stuck.
const TRIAL_TIMEOUT: Duration = Duration::from_secs(20);

static CACHE: OnceLock<Mutex<HashMap<PathBuf, Hardware>>> = OnceLock::new();

impl Hardware {
    /// No hardware: everything on the CPU.
    pub fn none() -> Self {
        Self::default()
    }

    /// The best encoder for `codec`.
    pub fn best(&self, codec: Codec) -> Option<&Verified> {
        self.encoders.iter().find(|v| v.encoder.codec == codec)
    }

    /// The verified encoder called `name`.
    pub fn get(&self, name: &str) -> Option<&Verified> {
        self.encoders.iter().find(|v| v.encoder.name == name)
    }

    /// Whether `KIMCHI_HARDWARE=0` (or `off`, `false`) turned hardware off.
    pub fn disabled() -> bool {
        std::env::var("KIMCHI_HARDWARE").is_ok_and(|v| matches!(v.trim(), "0" | "off" | "false" | "no"))
    }

    /// Tries every candidate this ffmpeg was built with, once per ffmpeg binary for the life of
    /// the process (about a second the first time, all candidates at once; free afterwards).
    pub async fn detect(tools: &Tools, caps: &Caps) -> Hardware {
        if Self::disabled() {
            return Hardware::none();
        }
        // Held across the trials so callers arriving meanwhile wait for one detection.
        let mut cache = CACHE.get_or_init(Default::default).lock().await;
        if let Some(hw) = cache.get(&tools.ffmpeg) {
            return hw.clone();
        }
        let devices = render_nodes();
        let mut trials = tokio::task::JoinSet::new();
        for (rank, candidate) in CANDIDATES.iter().enumerate().filter(|(_, c)| caps.has(c.name)) {
            let (tools, devices) = (tools.clone(), devices.clone());
            let candidate = *candidate;
            trials.spawn(async move { (rank, verify(&tools, candidate, &devices).await) });
        }
        let mut found: Vec<(usize, Verified, Option<String>)> = vec![];
        while let Some(joined) = trials.join_next().await {
            if let Ok((rank, Some((verified, device)))) = joined {
                found.push((rank, verified, device));
            }
        }
        found.sort_by_key(|(rank, ..)| *rank);
        let hw = Hardware {
            vaapi_device: found.iter().find_map(|(_, _, d)| d.clone()),
            encoders: found.into_iter().map(|(_, v, _)| v).collect(),
        };
        tracing::info!(encoders = ?hw.encoders.iter().map(|v| v.encoder.name).collect::<Vec<_>>(), "hardware encoders");
        cache.insert(tools.ffmpeg.clone(), hw.clone());
        hw
    }

    /// Replaces what [`Hardware::detect`] returns for `tools` (tests, and pinning a known setup).
    #[doc(hidden)]
    pub async fn assume(tools: &Tools, hw: Hardware) {
        CACHE.get_or_init(Default::default).lock().await.insert(tools.ffmpeg.clone(), hw);
    }
}

/// Options for one hardware encode.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HwArgs {
    /// `-c:v …` and its options.
    pub video: Vec<String>,
    /// What the graph hands the encoder.
    pub pix_fmt: &'static str,
    /// Global options that open a device (`-init_hw_device …`), before the inputs.
    pub device: Vec<String>,
    /// The frames go up to the device after the format conversion (`hwupload`).
    pub upload: bool,
}

/// How to drive `v` at quality `q`; `bitrate` is used where there is no constant-quality mode.
pub(crate) fn args(v: &Verified, q: Quality, bitrate: u64, vaapi_device: Option<&str>) -> HwArgs {
    options(v.encoder, v.constant_quality, q, bitrate, vaapi_device)
}

fn options(e: HwEncoder, constant_quality: bool, q: Quality, bitrate: u64, vaapi_device: Option<&str>) -> HwArgs {
    let by = |d, s, h| match q {
        Quality::Draft => d,
        Quality::Standard => s,
        Quality::High => h,
    };
    let mut video = vec![s("-c:v"), s(e.name)];
    let mut push = |xs: &[&str]| video.extend(xs.iter().map(|x| s(x)));
    let rate = bitrate.to_string();
    let peak = (bitrate * 3 / 2).to_string();
    let mut out = HwArgs { video: vec![], pix_fmt: "nv12", device: vec![], upload: false };
    match (e.api, e.codec) {
        (Api::VideoToolbox, Codec::Prores) => {
            push(&["-profile:v", "hq"]);
            out.pix_fmt = "p210le";
        }
        // Constant quality only exists on Apple Silicon (0–100).
        (Api::VideoToolbox, _) if constant_quality => push(&["-q:v", by("50", "62", "75")]),
        (Api::VideoToolbox, _) => push(&["-b:v", &rate]),
        (Api::Nvenc, _) => {
            // The p1–p7 presets and constant quality need SDK 10 drivers; older ones get a bitrate.
            if constant_quality {
                let cq = match e.codec {
                    Codec::H264 => by("30", "24", "19"),
                    _ => by("34", "28", "23"),
                };
                push(&["-preset", by("p2", "p5", "p6"), "-tune", "hq", "-rc", "vbr", "-cq", cq, "-b:v", "0"]);
            } else {
                push(&["-b:v", &rate]);
            }
        }
        (Api::Amf, _) => {
            if constant_quality {
                push(&["-quality", by("speed", "balanced", "quality"), "-rc", "vbr_peak", "-b:v", &rate, "-maxrate", &peak]);
            } else {
                push(&["-b:v", &rate]);
            }
        }
        (Api::Qsv, _) => {
            push(&["-preset", by("veryfast", "medium", "slow")]);
            if constant_quality {
                push(&["-global_quality", by("30", "24", "20")]);
            } else {
                push(&["-b:v", &rate]);
            }
        }
        (Api::Vaapi, _) => {
            if constant_quality {
                push(&["-rc_mode", "QVBR", "-global_quality", by("30", "24", "20"), "-b:v", &rate, "-maxrate", &peak]);
            } else {
                push(&["-b:v", &rate]);
            }
            let device = vaapi_device.unwrap_or("/dev/dri/renderD128");
            out.device = vec![s("-init_hw_device"), format!("vaapi=kva:{device}"), s("-filter_hw_device"), s("kva")];
            out.upload = true;
        }
        (Api::MediaFoundation, _) => {
            // Without hw_encoding, Media Foundation may pick a slow software encoder.
            push(&["-hw_encoding", "1", "-b:v", &rate]);
        }
    }
    out.video = video;
    out
}

/// The encoder at its best option set, else at its plainest; the VA-API render node it used.
async fn verify(tools: &Tools, e: HwEncoder, devices: &[String]) -> Option<(Verified, Option<String>)> {
    let nodes: Vec<Option<&str>> =
        if e.api == Api::Vaapi { devices.iter().map(|d| Some(d.as_str())).collect() } else { vec![None] };
    for node in nodes {
        for constant_quality in [true, false] {
            if trial(tools, &options(e, constant_quality, Quality::Standard, 2_000_000, node)).await {
                return Some((Verified { encoder: e, constant_quality }, node.map(str::to_owned)));
            }
            if e.api == Api::VideoToolbox && e.codec == Codec::Prores {
                break; // one option set
            }
        }
    }
    tracing::debug!(encoder = e.name, "hardware encoder not usable here");
    None
}

/// Encodes a few small frames with `a`; whether ffmpeg was happy.
async fn trial(tools: &Tools, a: &HwArgs) -> bool {
    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-v", "error"].map(s).to_vec();
    args.extend(a.device.iter().cloned());
    // 640x360: above every encoder's minimum size.
    args.extend(["-f", "lavfi", "-i", "testsrc2=s=640x360:r=30", "-frames:v", "8", "-vf"].map(s));
    args.push(if a.upload { format!("format={},hwupload", a.pix_fmt) } else { format!("format={}", a.pix_fmt) });
    args.extend(a.video.iter().cloned());
    args.extend(["-f", "null", "-"].map(s));
    let Ok(mut child) = process::spawn(&tools.ffmpeg, &args, false) else { return false };
    let stderr = process::collect_stderr(&mut child);
    match tokio::time::timeout(TRIAL_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) if status.success() => true,
        Ok(_) => {
            let stderr = stderr.await.unwrap_or_default();
            tracing::trace!(args = ?args, stderr = %stderr, "trial encode failed");
            false
        }
        Err(_) => {
            let _ = child.kill().await;
            false
        }
    }
}

/// DRM render nodes, `/dev/dri/renderD128` first.
fn render_nodes() -> Vec<String> {
    let mut nodes: Vec<String> = std::fs::read_dir("/dev/dri")
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("renderD"))
                .map(|e| e.path().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    nodes.sort();
    nodes
}

/// Largest picture `codec` hardware encoders take (H.264 encoders stop at 4096 on a side).
pub(crate) fn fits(codec: Codec, width: u32, height: u32) -> bool {
    let max = match codec {
        Codec::H264 => 4096,
        Codec::Prores => 16384,
        Codec::Hevc | Codec::Vp9 | Codec::Av1 => 8192,
    };
    width <= max && height <= max
}

/// Hardware decoders open per render at most: each one holds a decoder session and its frames.
pub(crate) const MAX_HW_DECODERS: usize = 4;

/// Input options that decode `meta` on the GPU or media engine, when that is worth it: 4K and up,
/// and the codecs that are heavy on a CPU (HEVC, AV1, VP9, ProRes). Frames come back to memory
/// for the filters; an unsupported stream quietly decodes in software.
pub(crate) fn decode_args(caps: &Caps, meta: &MediaMeta) -> Vec<String> {
    if Hardware::disabled() {
        return vec![];
    }
    let pixels = meta.width.unwrap_or(0) as u64 * meta.height.unwrap_or(0) as u64;
    let codec = meta.video_codec.as_deref().unwrap_or("");
    let heavy = pixels >= 3_000_000 || matches!(codec, "hevc" | "av1" | "vp9" | "prores");
    if !heavy {
        return vec![];
    }
    let method = if cfg!(target_os = "macos") {
        caps.hwaccels.contains("videotoolbox").then_some("videotoolbox")
    } else {
        // `auto` tries the build's hardware decoders in turn and falls back to software.
        ["cuda", "d3d11va", "d3d12va", "dxva2", "vaapi", "qsv", "vulkan"]
            .iter()
            .any(|m| caps.hwaccels.contains(*m))
            .then_some("auto")
    };
    method.map(|m| vec![s("-hwaccel"), s(m)]).unwrap_or_default()
}

/// A person-readable name for an ffmpeg video encoder: "NVIDIA NVENC", "x264 (CPU)".
pub fn label(encoder: &str) -> String {
    if let Some(c) = CANDIDATES.iter().find(|c| c.name == encoder) {
        return c.api.label().to_string();
    }
    let cpu = match encoder {
        "libx264" => "x264",
        "libx265" => "x265",
        "libopenh264" => "OpenH264",
        "prores_ks" | "prores" => "ProRes",
        "libvpx-vp9" => "libvpx VP9",
        "libvpx" => "libvpx VP8",
        "libsvtav1" => "SVT-AV1",
        "libaom-av1" => "libaom AV1",
        "mpeg4" => "MPEG-4",
        "gif" => "GIF",
        other => other,
    };
    format!("{cpu} (CPU)")
}

fn s(x: &str) -> String {
    x.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(name: &str, constant_quality: bool) -> Verified {
        Verified { encoder: *CANDIDATES.iter().find(|c| c.name == name).unwrap(), constant_quality }
    }

    #[test]
    fn options_per_api() {
        let nv = args(&v("h264_nvenc", true), Quality::High, 9_000_000, None);
        assert_eq!(nv.video.join(" "), "-c:v h264_nvenc -preset p6 -tune hq -rc vbr -cq 19 -b:v 0");
        assert_eq!((nv.pix_fmt, nv.upload, nv.device.len()), ("nv12", false, 0));
        let nv = args(&v("h264_nvenc", false), Quality::Draft, 1_000, None);
        assert_eq!(nv.video.join(" "), "-c:v h264_nvenc -b:v 1000");

        let va = args(&v("hevc_vaapi", false), Quality::Standard, 5_000, Some("/dev/dri/renderD129"));
        assert!(va.upload && va.device.join(" ") == "-init_hw_device vaapi=kva:/dev/dri/renderD129 -filter_hw_device kva");

        let pr = args(&v("prores_videotoolbox", false), Quality::Standard, 0, None);
        assert_eq!((pr.video.join(" ").as_str(), pr.pix_fmt), ("-c:v prores_videotoolbox -profile:v hq", "p210le"));
        let vt = args(&v("hevc_videotoolbox", true), Quality::Standard, 0, None);
        assert!(vt.video.ends_with(&[s("-q:v"), s("62")]));
        let mf = args(&v("h264_mf", false), Quality::Standard, 7, None);
        assert!(mf.video.join(" ").contains("-hw_encoding 1 -b:v 7"));
    }

    #[test]
    fn picks_best_first_and_knows_limits() {
        let hw = Hardware { encoders: vec![v("h264_nvenc", true), v("h264_qsv", true), v("hevc_qsv", false)], vaapi_device: None };
        assert_eq!(hw.best(Codec::H264).unwrap().encoder.name, "h264_nvenc");
        assert_eq!(hw.best(Codec::Hevc).unwrap().encoder.name, "hevc_qsv");
        assert!(hw.best(Codec::Av1).is_none() && Hardware::none().best(Codec::H264).is_none());
        assert!(fits(Codec::H264, 3840, 2160) && !fits(Codec::H264, 7680, 4320) && fits(Codec::Hevc, 7680, 4320));
        assert_eq!(label("h264_videotoolbox"), "Apple VideoToolbox");
        assert_eq!(label("libx264"), "x264 (CPU)");
    }

    #[test]
    fn decodes_heavy_sources_on_hardware() {
        let caps = Caps::new(9, ["libx264"]).with_hwaccels(["videotoolbox", "cuda"]);
        let meta = |w, h, codec: &str| MediaMeta { width: Some(w), height: Some(h), video_codec: Some(codec.into()), ..Default::default() };
        assert!(decode_args(&caps, &meta(1920, 1080, "h264")).is_empty());
        let want = if cfg!(target_os = "macos") { "videotoolbox" } else { "auto" };
        assert_eq!(decode_args(&caps, &meta(3840, 2160, "h264")), [s("-hwaccel"), s(want)]);
        assert_eq!(decode_args(&caps, &meta(1920, 1080, "hevc")), [s("-hwaccel"), s(want)]);
        assert!(decode_args(&Caps::new(9, ["libx264"]), &meta(3840, 2160, "hevc")).is_empty());
    }
}
