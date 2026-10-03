//! Speech to text with OpenAI's Whisper, run locally by candle on the CPU.
//!
//! The model (weights, config, tokenizer) is downloaded once from Hugging Face into a folder of
//! kimchi's data ([`download`]). [`Whisper::transcribe`] then works offline: 16 kHz mono audio
//! in 30 s windows, greedy decoding with Whisper's timestamp rules, giving segments with their
//! times. Windows that are silence (no-speech probability high, low confidence) are skipped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use candle_core::{D, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::whisper::{self as m, Config, audio};
use futures::StreamExt;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::Segment;

pub type Result<T> = std::result::Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Sample rate the audio must have.
pub const SAMPLE_RATE: u32 = m::SAMPLE_RATE as u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Model {
    Tiny,
    Base,
    Small,
}

pub struct ModelInfo {
    pub model: Model,
    pub id: &'static str,
    pub label: &'static str,
    /// Download size in MB.
    pub size_mb: u32,
    pub doc: &'static str,
}

pub const MODELS: &[ModelInfo] = &[
    ModelInfo { model: Model::Tiny, id: "tiny", label: "Tiny", size_mb: 151, doc: "Fastest; fine for clear speech." },
    ModelInfo { model: Model::Base, id: "base", label: "Base", size_mb: 290, doc: "Good balance of speed and accuracy (default)." },
    ModelInfo { model: Model::Small, id: "small", label: "Small", size_mb: 967, doc: "Most accurate here; several times slower." },
];

impl Model {
    pub fn info(self) -> &'static ModelInfo {
        MODELS.iter().find(|m| m.model == self).expect("listed")
    }

    pub fn parse(s: &str) -> Result<Model> {
        MODELS.iter().find(|m| m.id.eq_ignore_ascii_case(s.trim())).map(|m| m.model).ok_or_else(|| {
            format!("Unknown speech model `{s}`. Models: {}.", MODELS.iter().map(|m| m.id).collect::<Vec<_>>().join(", "))
        })
    }

    /// Where its files live under `root`.
    pub fn dir(self, root: &Path) -> PathBuf {
        root.join(format!("whisper-{}", self.info().id))
    }

    pub fn is_downloaded(self, root: &Path) -> bool {
        FILES.iter().all(|f| self.dir(root).join(f).is_file())
    }
}

const FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];

/// Downloads the model's files into `root` (once; files already there are kept). `progress`
/// gets 0–1.
pub async fn download(root: &Path, model: Model, progress: impl Fn(f64), cancel: &CancellationToken) -> Result<()> {
    let dir = model.dir(root);
    tokio::fs::create_dir_all(&dir).await.map_err(err)?;
    let client = reqwest::Client::builder().user_agent(concat!("kimchi/", env!("CARGO_PKG_VERSION"))).build().map_err(err)?;
    let total = model.info().size_mb as f64 * 1e6;
    let mut done = 0.0;
    for file in FILES {
        let out = dir.join(file);
        if out.is_file() {
            continue;
        }
        let url = format!("https://huggingface.co/openai/whisper-{}/resolve/main/{file}", model.info().id);
        let resp = client.get(&url).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("Couldn't download the speech model ({file}): {e}"))?;
        let part = dir.join(format!("{file}.part"));
        let mut f = tokio::fs::File::create(&part).await.map_err(err)?;
        let mut body = resp.bytes_stream();
        while let Some(chunk) = body.next().await {
            if cancel.is_cancelled() {
                drop(f);
                let _ = tokio::fs::remove_file(&part).await;
                return Err("cancelled".into());
            }
            let chunk = chunk.map_err(|e| format!("The speech model download stopped: {e}"))?;
            f.write_all(&chunk).await.map_err(err)?;
            done += chunk.len() as f64;
            progress((done / total).min(0.99));
        }
        f.flush().await.map_err(err)?;
        drop(f);
        tokio::fs::rename(&part, &out).await.map_err(err)?;
    }
    progress(1.0);
    Ok(())
}

/// What was heard.
#[derive(Debug, Clone, Serialize)]
pub struct Transcript {
    /// The language spoken (detected, or the one asked for), e.g. "en".
    pub language: String,
    pub segments: Vec<Segment>,
}

/// A loaded model.
pub struct Whisper {
    model: m::model::Whisper,
    config: Config,
    tok: Tokenizer,
    filters: Vec<f32>,
    device: Device,
}

impl Whisper {
    /// Loads the model downloaded in `dir`.
    pub fn load(dir: &Path) -> Result<Self> {
        let device = Device::Cpu;
        let config: Config = serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).map_err(err)?).map_err(err)?;
        let tok = Tokenizer::load(&dir.join("tokenizer.json"))?;
        // SAFETY: the weights file is ours and isn't changed while it is mapped.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[dir.join("model.safetensors")], m::DTYPE, &device) }.map_err(err)?;
        let model = m::model::Whisper::load(&vb, config.clone()).map_err(err)?;
        let filters = mel_filters(config.num_mel_bins);
        Ok(Self { model, config, tok, filters, device })
    }

    /// Languages the model knows (codes like "en", "fr").
    pub fn languages(&self) -> Vec<String> {
        self.tok.languages.iter().map(|(l, _)| l.clone()).collect()
    }

    /// Transcribes `samples` (mono, [`SAMPLE_RATE`]). `language` ("en", "fr"…) or detected from
    /// the first window. `progress` gets 0–1.
    pub fn transcribe(&mut self, samples: &[f32], language: Option<&str>, progress: &dyn Fn(f64), cancel: &CancellationToken) -> Result<Transcript> {
        let n_mels = self.config.num_mel_bins;
        let mel = audio::pcm_to_mel(&self.config, samples, &self.filters);
        let mel_len = mel.len() / n_mels;
        let mel = Tensor::from_vec(mel, (1, n_mels, mel_len), &self.device).map_err(err)?;
        let content = samples.len() / m::HOP_LENGTH;
        let mut lang: Option<u32> = match language.map(str::trim).filter(|l| !l.is_empty()) {
            Some(code) => Some(self.tok.language(code)?),
            None => None,
        };
        let mut segments = vec![];
        let mut seek = 0;
        while seek < content {
            if cancel.is_cancelled() {
                return Err("cancelled".into());
            }
            progress(seek as f64 / content.max(1) as f64);
            let size = m::N_FRAMES.min(mel_len - seek);
            let window = mel.narrow(2, seek, size).map_err(err)?;
            let feats = self.model.encoder.forward(&window, true).map_err(err)?;
            let lang_id = match lang {
                Some(l) => l,
                None => {
                    let l = self.detect_language(&feats)?;
                    lang = Some(l);
                    l
                }
            };
            let decoded = self.decode(&feats, lang_id, cancel)?;
            let offset = seek as f64 * m::HOP_LENGTH as f64 / m::SAMPLE_RATE as f64;
            let window_len = (size.min(content - seek)) as f64 * m::HOP_LENGTH as f64 / m::SAMPLE_RATE as f64;
            let silent = decoded.no_speech > m::NO_SPEECH_THRESHOLD && decoded.avg_logprob < m::LOGPROB_THRESHOLD;
            let (found, advance) = self.segments(&decoded.tokens, window_len);
            if !silent {
                segments.extend(found.into_iter().filter(|s| !s.text.is_empty()).map(|s| Segment { start: offset + s.start, end: (offset + s.end).min(offset + window_len), text: s.text }));
            }
            // At least a second forward, so a confused window can't stall.
            let frames = (advance * m::SAMPLE_RATE as f64 / m::HOP_LENGTH as f64).round() as usize;
            seek += frames.max(100).min(size);
        }
        progress(1.0);
        let language = lang.and_then(|id| self.tok.languages.iter().find(|(_, i)| *i == id).map(|(l, _)| l.clone())).unwrap_or_default();
        Ok(Transcript { language, segments })
    }

    fn logits(&mut self, tokens: &[u32], feats: &Tensor, flush: bool) -> Result<(Vec<f32>, Tensor)> {
        let t = Tensor::new(tokens, &self.device).and_then(|t| t.unsqueeze(0)).map_err(err)?;
        let ys = self.model.decoder.forward(&t, feats, flush).map_err(err)?;
        let (_, n, _) = ys.dims3().map_err(err)?;
        let last = self.model.decoder.final_linear(&ys.i((..1, n - 1..)).map_err(err)?).map_err(err)?;
        let v = last.i(0).and_then(|t| t.i(0)).and_then(|t| t.to_vec1::<f32>()).map_err(err)?;
        Ok((v, ys))
    }

    fn detect_language(&mut self, feats: &Tensor) -> Result<u32> {
        let (logits, _) = self.logits(&[self.tok.sot], feats, true)?;
        self.tok.languages.iter().map(|(_, id)| *id).max_by(|a, b| logits[*a as usize].total_cmp(&logits[*b as usize])).ok_or_else(|| "this model knows no languages".to_string())
    }

    /// Greedy decoding of one window with the timestamp rules.
    fn decode(&mut self, feats: &Tensor, lang: u32, cancel: &CancellationToken) -> Result<Decoded> {
        let tk = self.tok.clone_ids();
        let mut tokens = vec![tk.sot, lang, tk.transcribe];
        let begin = tokens.len();
        let (mut sum, mut no_speech) = (0.0f64, 0.0f64);
        let suppress: Vec<u32> = self.config.suppress_tokens.clone();
        let max_new = self.config.max_target_positions / 2 - begin;
        for i in 0..max_new {
            if cancel.is_cancelled() {
                return Err("cancelled".into());
            }
            let (mut logits, ys) = self.logits(&tokens, feats, i == 0)?;
            if i == 0
                && let Some(ns) = tk.nospeech
            {
                // At the start-of-transcript position.
                let first = self.model.decoder.final_linear(&ys.i((..1, ..1)).map_err(err)?).map_err(err)?;
                let probs = candle_nn::ops::softmax(&first, D::Minus1).and_then(|p| p.i((0, 0))).and_then(|p| p.to_vec1::<f32>()).map_err(err)?;
                no_speech = probs.get(ns as usize).copied().unwrap_or(0.0) as f64;
            }
            for &s in &suppress {
                if let Some(l) = logits.get_mut(s as usize) {
                    *l = f32::NEG_INFINITY;
                }
            }
            timestamp_rules(&mut logits, &tokens[begin..], &tk);
            let next = argmax(&logits);
            let lp = log_softmax_at(&logits, next);
            tokens.push(next);
            sum += lp as f64;
            if next == tk.eot || repeating(&tokens[begin..]) {
                break;
            }
        }
        let n = (tokens.len() - begin).max(1) as f64;
        Ok(Decoded { tokens: tokens[begin..].to_vec(), avg_logprob: sum / n, no_speech })
    }

    /// Segments (window-relative times) of decoded tokens, and how far to move on (seconds).
    fn segments(&self, tokens: &[u32], window_len: f64) -> (Vec<Segment>, f64) {
        let tk = &self.tok;
        let is_ts = |t: u32| t >= tk.ts_begin;
        let time = |t: u32| (t - tk.ts_begin) as f64 * 0.02;
        let ended = tokens.last() == Some(&tk.eot);
        let body: Vec<u32> = tokens.iter().copied().filter(|t| *t != tk.eot).collect();
        let mut out = vec![];
        let mut start: Option<f64> = None;
        let mut text: Vec<u32> = vec![];
        for &t in &body {
            if is_ts(t) {
                match start {
                    Some(s) if !text.is_empty() => {
                        out.push(Segment { start: s, end: time(t), text: tk.decode(&text) });
                        text.clear();
                        start = None;
                    }
                    _ => start = Some(time(t)),
                }
            } else if t < tk.eot {
                start.get_or_insert(0.0);
                text.push(t);
            }
        }
        // Words the model ended without a closing time run to the end of the window.
        if let (Some(s), false, true) = (start, text.is_empty(), ended) {
            out.push(Segment { start: s, end: window_len, text: tk.decode(&text) });
        }
        // Ending on a fresh timestamp pair: the next words may be cut by the window; start there.
        let n = body.len();
        let advance = if n >= 2 && is_ts(body[n - 1]) && is_ts(body[n - 2]) && time(body[n - 2]) > 1.0 {
            time(body[n - 2])
        } else if !ended && let Some(last) = out.last() {
            // Stopped early (too many tokens): go on from the last complete segment.
            last.end
        } else {
            window_len.max(0.01)
        };
        (out, advance)
    }
}

struct Decoded {
    tokens: Vec<u32>,
    avg_logprob: f64,
    no_speech: f64,
}

/// OpenAI's timestamp rules: timestamps come in pairs and never go back, the first token is a
/// timestamp within the first second, and a timestamp is forced when they are likelier
/// together than any word.
fn timestamp_rules(logits: &mut [f32], seq: &[u32], tk: &Ids) {
    let n = logits.len();
    let ts = tk.ts_begin as usize;
    // Special tokens other than the end and timestamps never come out.
    for l in &mut logits[(tk.eot as usize + 1).min(n)..ts.min(n)] {
        *l = f32::NEG_INFINITY;
    }
    let is_ts = |t: u32| t >= tk.ts_begin;
    let last = seq.last().is_some_and(|t| is_ts(*t));
    let penult = seq.len() < 2 || is_ts(seq[seq.len() - 2]);
    if last {
        if penult {
            logits[ts.min(n)..].fill(f32::NEG_INFINITY);
        } else {
            logits[..tk.eot as usize].fill(f32::NEG_INFINITY);
        }
    }
    if let Some(max) = seq.iter().copied().filter(|t| is_ts(*t)).max() {
        // Not before the last time (repeat it only to close a segment).
        let floor = if last && !penult { max } else { max + 1 } as usize;
        logits[ts.min(n)..floor.min(n)].fill(f32::NEG_INFINITY);
    }
    if seq.is_empty() {
        logits[..ts.min(n)].fill(f32::NEG_INFINITY);
        let max_initial = ts + 50;
        if max_initial + 1 < n {
            logits[max_initial + 1..].fill(f32::NEG_INFINITY);
        }
    }
    // Probability of any timestamp against the best word.
    let mx = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if mx.is_finite() {
        let lse = |s: &[f32]| {
            let m = s.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            if m.is_finite() { m + s.iter().map(|v| (v - m).exp()).sum::<f32>().ln() } else { f32::NEG_INFINITY }
        };
        let ts_mass = lse(&logits[ts.min(n)..]);
        let best_text = logits[..ts.min(n)].iter().copied().fold(f32::NEG_INFINITY, f32::max);
        if ts_mass > best_text {
            logits[..ts.min(n)].fill(f32::NEG_INFINITY);
        }
    }
}

fn argmax(v: &[f32]) -> u32 {
    v.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i as u32)
}

fn log_softmax_at(v: &[f32], i: u32) -> f32 {
    let m = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let lse = m + v.iter().map(|x| (x - m).exp()).sum::<f32>().ln();
    v[i as usize] - lse
}

/// The same few tokens over and over (a known Whisper failure): stop the window.
fn repeating(seq: &[u32]) -> bool {
    for n in 2..=8 {
        if seq.len() >= n * 4 {
            let tail = &seq[seq.len() - n..];
            if (1..4).all(|k| &seq[seq.len() - (k + 1) * n..seq.len() - k * n] == tail) {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------
// Tokenizer: Hugging Face's tokenizer.json, decoding only.

#[derive(Clone, Copy)]
struct Ids {
    sot: u32,
    eot: u32,
    transcribe: u32,
    nospeech: Option<u32>,
    ts_begin: u32,
}

struct Tokenizer {
    /// Bytes of each ordinary token, by id.
    pieces: Vec<Vec<u8>>,
    sot: u32,
    eot: u32,
    transcribe: u32,
    nospeech: Option<u32>,
    ts_begin: u32,
    /// (code, token), in the model's order.
    languages: Vec<(String, u32)>,
}

impl Tokenizer {
    fn load(path: &Path) -> Result<Self> {
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).map_err(err)?).map_err(err)?;
        let vocab = json["model"]["vocab"].as_object().ok_or("tokenizer.json has no vocab")?;
        let unbyte = byte_decoder();
        let mut pieces: Vec<Vec<u8>> = vec![];
        for (piece, id) in vocab {
            let id = id.as_u64().ok_or("bad vocab id")? as usize;
            if pieces.len() <= id {
                pieces.resize(id + 1, vec![]);
            }
            pieces[id] = piece.chars().filter_map(|c| unbyte.get(&c).copied()).collect();
        }
        let mut special: HashMap<String, u32> = HashMap::new();
        for t in json["added_tokens"].as_array().into_iter().flatten() {
            if let (Some(c), Some(id)) = (t["content"].as_str(), t["id"].as_u64()) {
                special.insert(c.to_string(), id as u32);
            }
        }
        let get = |k: &str| special.get(k).copied().ok_or_else(|| format!("tokenizer.json lacks {k}"));
        let (sot, eot, transcribe, translate) = (get(m::SOT_TOKEN)?, get(m::EOT_TOKEN)?, get(m::TRANSCRIBE_TOKEN)?, get(m::TRANSLATE_TOKEN)?);
        let notimestamps = get(m::NO_TIMESTAMPS_TOKEN)?;
        let nospeech = m::NO_SPEECH_TOKENS.iter().find_map(|k| special.get(*k).copied());
        let ts_begin = special.get("<|0.00|>").copied().unwrap_or(notimestamps + 1);
        let mut languages: Vec<(String, u32)> = special
            .iter()
            .filter(|(c, id)| **id > sot && **id < translate && c.starts_with("<|") && c.ends_with("|>"))
            .map(|(c, id)| (c[2..c.len() - 2].to_string(), *id))
            .filter(|(c, _)| c.chars().all(|ch| ch.is_ascii_lowercase()) && (2..=3).contains(&c.len()))
            .collect();
        languages.sort_by_key(|(_, id)| *id);
        Ok(Self { pieces, sot, eot, transcribe, nospeech, ts_begin, languages })
    }

    fn clone_ids(&self) -> Ids {
        Ids { sot: self.sot, eot: self.eot, transcribe: self.transcribe, nospeech: self.nospeech, ts_begin: self.ts_begin }
    }

    fn language(&self, code: &str) -> Result<u32> {
        let code = code.to_ascii_lowercase();
        self.languages.iter().find(|(c, _)| *c == code).map(|(_, id)| *id).ok_or_else(|| {
            format!("The speech model doesn't know the language `{code}`. Use a code like en, fr, es, de, ja.")
        })
    }

    fn decode(&self, tokens: &[u32]) -> String {
        let bytes: Vec<u8> = tokens.iter().filter(|t| **t < self.eot).flat_map(|t| self.pieces.get(*t as usize).cloned().unwrap_or_default()).collect();
        String::from_utf8_lossy(&bytes).trim().to_string()
    }
}

/// GPT-2's byte-level alphabet, reversed: each printable stand-in character → its byte.
fn byte_decoder() -> HashMap<char, u8> {
    let mut bs: Vec<u32> = (b'!' as u32..=b'~' as u32).chain(0xA1..=0xAC).chain(0xAE..=0xFF).collect();
    let mut cs = bs.clone();
    let mut n = 0;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    bs.into_iter().zip(cs).filter_map(|(b, c)| char::from_u32(c).map(|c| (c, b as u8))).collect()
}

/// librosa's mel filter bank (Slaney scale and norm), as Whisper was trained with:
/// `n_mels` rows of `N_FFT / 2 + 1` weights.
pub fn mel_filters(n_mels: usize) -> Vec<f32> {
    let (sr, n_fft) = (m::SAMPLE_RATE as f64, m::N_FFT);
    let bins = n_fft / 2 + 1;
    let hz_to_mel = |f: f64| {
        let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
        let min_log_mel = min_log_hz / f_sp;
        if f >= min_log_hz { min_log_mel + (f / min_log_hz).ln() / (6.4f64.ln() / 27.0) } else { f / f_sp }
    };
    let mel_to_hz = |m: f64| {
        let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
        let min_log_mel = min_log_hz / f_sp;
        if m >= min_log_mel { min_log_hz * ((6.4f64.ln() / 27.0) * (m - min_log_mel)).exp() } else { f_sp * m }
    };
    let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(sr / 2.0));
    let mel_f: Vec<f64> = (0..n_mels + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (n_mels + 1) as f64)).collect();
    let fft_f: Vec<f64> = (0..bins).map(|i| sr / 2.0 * i as f64 / (bins - 1) as f64).collect();
    let mut w = vec![0f32; n_mels * bins];
    for i in 0..n_mels {
        let (d0, d1) = (mel_f[i + 1] - mel_f[i], mel_f[i + 2] - mel_f[i + 1]);
        let enorm = 2.0 / (mel_f[i + 2] - mel_f[i]);
        for (k, f) in fft_f.iter().enumerate() {
            let lower = (f - mel_f[i]) / d0;
            let upper = (mel_f[i + 2] - f) / d1;
            w[i * bins + k] = (lower.min(upper).max(0.0) * enorm) as f32;
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_filters_match_whisper() {
        // Values from Whisper's own assets/mel_filters.npz (mel_80).
        let w = mel_filters(80);
        assert_eq!(w.len(), 80 * 201);
        let at = |r: usize, c: usize| w[r * 201 + c];
        for (r, c, want) in [(0, 1, 0.024_862_59), (1, 1, 0.001_990_82), (1, 2, 0.022_871_77), (79, 198, 0.000_897_518), (79, 199, 0.000_448_759), (79, 200, 0.0)] {
            assert!((at(r, c) - want).abs() < 1e-7, "[{r}][{c}] = {}, want {want}", at(r, c));
        }
        let peak = (0..201).map(|c| at(40, c)).fold(0.0, f32::max);
        assert!((peak - 0.014_735_566).abs() < 1e-7, "{peak}");
    }

    #[test]
    fn byte_level_pieces_decode() {
        let d = byte_decoder();
        assert_eq!(d.len(), 256);
        assert_eq!(d[&'Ġ'], b' ');
        assert_eq!(d[&'a'], b'a');
    }

    #[test]
    fn timestamps_come_first_and_in_pairs() {
        let tk = Ids { sot: 10, eot: 9, transcribe: 12, nospeech: None, ts_begin: 20 };
        // Vocabulary: 0..9 words, 9 eot, 10..19 specials, 20.. timestamps.
        let mut l = vec![1.0f32; 90];
        timestamp_rules(&mut l, &[], &tk);
        assert!(l[..20].iter().all(|v| v.is_infinite()), "starts with a time");
        let mut l = vec![1.0f32; 90];
        timestamp_rules(&mut l, &[20, 3], &tk);
        assert!(l[11].is_infinite(), "no specials");
        let mut l = vec![0.0f32; 90];
        l[3] = 5.0;
        timestamp_rules(&mut l, &[22, 3, 30], &tk);
        assert!(l[..9].iter().all(|v| v.is_infinite()), "after one time: another time or the end");
        assert!(l[21..30].iter().all(|v| v.is_infinite()) && l[30].is_finite(), "never back in time");
    }

    #[test]
    fn repetition_is_caught() {
        assert!(repeating(&[1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3]));
        assert!(!repeating(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
    }
}
