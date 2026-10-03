//! Helpers shared by provider implementations.

use std::future::Future;
use std::time::{Duration, Instant};

use base64::Engine;
use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::provider::{Ctx, GenError, GenResult};
use crate::types::{OutputKind, Progress};

/// Turns a reqwest send error into a [`GenError::Network`].
pub fn net_err(cx: &Ctx) -> impl Fn(reqwest::Error) -> GenError + '_ {
    move |e| GenError::Network { provider: cx.provider.clone(), message: e.without_url().to_string() }
}

/// Sends a request and maps non-2xx answers to typed errors, extracting the
/// most useful message from common JSON error shapes.
pub async fn send(cx: &Ctx, req: reqwest::RequestBuilder) -> GenResult<reqwest::Response> {
    let resp = req.send().await.map_err(net_err(cx))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let code = status.as_u16();
    let body = resp.text().await.unwrap_or_default();
    // Several APIs answer 400/403/422 when a safety filter trips; that's not a key problem.
    if matches!(code, 400 | 403 | 422) && looks_moderated(&body) {
        return Err(GenError::Moderated(error_message(&body)));
    }
    if code == 401 || code == 403 {
        return Err(GenError::Unauthorized { provider: cx.provider.clone(), status: code, message: error_message(&body) });
    }
    Err(GenError::Http { provider: cx.provider.clone(), status: code, message: error_message(&body) })
}

/// Heuristic for safety-filter refusals across providers.
pub fn looks_moderated(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    ["moderat", "content policy", "safety", "nsfw", "flagged", "content_filter", "responsible ai"].iter().any(|k| b.contains(k))
}

/// [`send`] + JSON decode.
pub async fn send_json<T: DeserializeOwned>(cx: &Ctx, req: reqwest::RequestBuilder) -> GenResult<T> {
    let resp = send(cx, req).await?;
    let text = resp.text().await.map_err(net_err(cx))?;
    serde_json::from_str(&text).map_err(|e| GenError::Decode {
        provider: cx.provider.clone(),
        message: format!("{e}: {}", truncate(&text, 300)),
    })
}

/// Best-effort human message from an error body.
pub fn error_message(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return truncate(body.trim(), 400);
    };
    let candidates = [
        "/error/message",
        "/error/metadata/raw",
        "/error",
        "/message",
        "/detail/0/msg",
        "/detail/message",
        "/detail",
        "/errors/0/message",
        "/errors/0",
        "/failure",
        "/title",
    ];
    for p in candidates {
        match v.pointer(p) {
            Some(Value::String(s)) if !s.is_empty() => return truncate(s, 400),
            _ => {}
        }
    }
    truncate(&v.to_string(), 400)
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

pub fn decode_err(cx: &Ctx, message: impl Into<String>) -> GenError {
    GenError::Decode { provider: cx.provider.clone(), message: message.into() }
}

/// Network trouble and busy or failing servers (429, 5xx) are worth asking again;
/// anything the provider said about the job itself is not.
pub fn is_transient(e: &GenError) -> bool {
    match e {
        GenError::Network { .. } => true,
        GenError::Http { status, .. } => *status == 429 || (500..600).contains(status),
        _ => false,
    }
}

/// Consecutive transient errors [`poll`] rides out before giving up.
pub const POLL_RETRIES: u32 = 5;

/// Polls `f` until it yields `Some`, sleeping `every` between attempts.
/// A transient error ([`is_transient`]) is retried with a growing pause, up to
/// [`POLL_RETRIES`] in a row; any other `Err` aborts immediately.
pub async fn poll<T, F, Fut>(every: Duration, timeout: Duration, mut f: F) -> GenResult<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = GenResult<Option<T>>>,
{
    let started = Instant::now();
    let mut failures = 0u32;
    loop {
        let mut pause = every;
        match f().await {
            Ok(Some(v)) => return Ok(v),
            Ok(None) => failures = 0,
            Err(e) if is_transient(&e) && failures < POLL_RETRIES => {
                failures += 1;
                tracing::debug!("poll retry {failures}/{POLL_RETRIES}: {e}");
                pause = (every * 2u32.pow(failures)).min(Duration::from_secs(30)).max(every);
            }
            Err(e) => return Err(e),
        }
        if started.elapsed() > timeout {
            return Err(GenError::Timeout(timeout.as_secs()));
        }
        tokio::time::sleep(pause).await;
    }
}

/// Reports a rough progress fraction for services that don't give one,
/// assuming a typical run takes `expected`.
pub fn estimate(cx: &Ctx, started: Instant, expected: Duration, message: &str) {
    let f = started.elapsed().as_secs_f64() / expected.as_secs_f64().max(1.0);
    // Ease towards 95% so the bar never looks finished before it is.
    let eased = 0.95 * (1.0 - (-2.2 * f).exp());
    cx.report(Progress::fraction(eased, message));
}

pub fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn b64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    let s = s.trim();
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(s.trim_end_matches('=')))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')))
}

/// Parses `data:<mime>;base64,<payload>`.
pub fn parse_data_url(s: &str) -> Option<(String, Bytes)> {
    let rest = s.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mime = meta.split(';').next().unwrap_or("application/octet-stream").to_string();
    let data = if meta.ends_with(";base64") { b64_decode(payload).ok()? } else { payload.as_bytes().to_vec() };
    Some((mime, Bytes::from(data)))
}

/// Guesses a MIME type from magic bytes.
pub fn sniff_mime(data: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| data.starts_with(sig);
    if starts(b"\x89PNG") {
        Some("image/png")
    } else if starts(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if starts(b"GIF8") {
        Some("image/gif")
    } else if data.len() > 12 && &data[4..8] == b"ftyp" {
        let brand = &data[8..12];
        if brand == b"qt  " {
            Some("video/quicktime")
        } else if brand.starts_with(b"avif") || brand == b"avis" {
            Some("image/avif")
        } else if brand.starts_with(b"hei") || brand == b"mif1" {
            Some("image/heic")
        } else {
            Some("video/mp4")
        }
    } else if starts(b"\x1A\x45\xDF\xA3") {
        Some("video/webm")
    } else if starts(b"ID3") || starts(b"\xFF\xFB") {
        Some("audio/mpeg")
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
        Some("audio/wav")
    } else {
        None
    }
}

pub fn extension_for(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or("").trim() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        "image/heic" => "heic",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "video/webm" => "webm",
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/ogg" => "ogg",
        _ => "bin",
    }
}

pub fn kind_for_mime(mime: &str) -> Option<OutputKind> {
    match mime.split('/').next()? {
        "image" => Some(OutputKind::Image),
        "video" => Some(OutputKind::Video),
        "audio" => Some(OutputKind::Audio),
        _ => None,
    }
}

/// Reduced ratio like `"16:9"`.
pub fn ratio_string(w: u32, h: u32) -> String {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let g = gcd(w, h).max(1);
    format!("{}:{}", w / g, h / g)
}

pub fn parse_ratio(r: &str) -> Option<(f64, f64)> {
    let (a, b) = r.split_once(':').or_else(|| r.split_once('x')).or_else(|| r.split_once('/'))?;
    let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
    (a > 0.0 && b > 0.0).then_some((a, b))
}

/// Picks the supported ratio closest to `wanted` (compared in log space).
pub fn closest_ratio<'a>(wanted: &str, supported: &'a [&'a str]) -> &'a str {
    let Some((a, b)) = parse_ratio(wanted) else { return supported.first().copied().unwrap_or("16:9") };
    let target = (a / b).ln();
    supported
        .iter()
        .copied()
        .min_by(|x, y| {
            let d = |r: &str| parse_ratio(r).map(|(a, b)| ((a / b).ln() - target).abs()).unwrap_or(f64::MAX);
            d(x).total_cmp(&d(y))
        })
        .unwrap_or("16:9")
}

/// Pixel size for a ratio with roughly `megapixels` area, both sides a multiple of `multiple`.
pub fn size_for_ratio(ratio: &str, megapixels: f64, multiple: u32) -> (u32, u32) {
    let (a, b) = parse_ratio(ratio).unwrap_or((1.0, 1.0));
    let area = megapixels * 1_000_000.0;
    let h = (area * b / a).sqrt();
    let w = h * a / b;
    let round = |v: f64| ((v / multiple as f64).round() as u32).max(1) * multiple;
    (round(w), round(h))
}

/// Picks the allowed duration closest to `wanted`.
pub fn closest_duration(wanted: Option<f64>, allowed: &[f64], default: f64) -> f64 {
    let Some(w) = wanted else { return default };
    allowed.iter().copied().min_by(|a, b| (a - w).abs().total_cmp(&(b - w).abs())).unwrap_or(default)
}

/// Downloads a URL into memory, returning bytes and a MIME type.
pub async fn download(cx: &Ctx, url: &str, headers: &[(String, String)]) -> GenResult<(Bytes, String)> {
    if let Some((mime, data)) = parse_data_url(url) {
        return Ok((data, mime));
    }
    let mut req = cx.http.get(url).timeout(Duration::from_secs(600));
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let resp = send(cx, req).await?;
    let header_mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string());
    let data = resp.bytes().await.map_err(net_err(cx))?;
    let mime = match header_mime {
        Some(m) if m.contains('/') && m != "application/octet-stream" && m != "binary/octet-stream" => m,
        _ => sniff_mime(&data).map(str::to_string).unwrap_or_else(|| guess_from_url(url)),
    };
    Ok((data, mime))
}

fn guess_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(status: u16) -> GenError {
        GenError::Http { provider: "p".into(), status, message: String::new() }
    }

    #[tokio::test(start_paused = true)]
    async fn poll_rides_out_transient_errors() {
        let calls = std::cell::Cell::new(0);
        let got = poll(Duration::from_secs(1), Duration::from_secs(3600), || {
            calls.set(calls.get() + 1);
            let n = calls.get();
            async move {
                match n {
                    1 => Err(http(503)),
                    2 => Err(GenError::Network { provider: "p".into(), message: "reset".into() }),
                    3 => Ok(None),
                    4 => Err(http(429)),
                    _ => Ok(Some(n)),
                }
            }
        })
        .await;
        assert_eq!(got.unwrap(), 5);
    }

    #[tokio::test(start_paused = true)]
    async fn poll_gives_up_after_consecutive_failures_and_on_real_errors() {
        let calls = std::cell::Cell::new(0u32);
        let got: GenResult<()> = poll(Duration::from_secs(1), Duration::from_secs(3600), || {
            calls.set(calls.get() + 1);
            async { Err(http(502)) }
        })
        .await;
        assert!(matches!(got, Err(GenError::Http { status: 502, .. })));
        assert_eq!(calls.get(), POLL_RETRIES + 1);

        calls.set(0);
        let got: GenResult<()> = poll(Duration::from_secs(1), Duration::from_secs(3600), || {
            calls.set(calls.get() + 1);
            async { Err(GenError::Provider("the job failed".into())) }
        })
        .await;
        assert!(matches!(got, Err(GenError::Provider(_))));
        assert_eq!(calls.get(), 1);
        assert!(!is_transient(&http(404)));
    }

    #[test]
    fn ratios() {
        assert_eq!(ratio_string(1920, 1080), "16:9");
        assert_eq!(closest_ratio("1920:1080", &["1:1", "16:9", "9:16"]), "16:9");
        assert_eq!(closest_ratio("4:5", &["1:1", "16:9", "9:16"]), "1:1");
        assert_eq!(size_for_ratio("16:9", 1.0, 64), (1344, 768));
        assert_eq!(closest_duration(Some(7.0), &[5.0, 10.0], 5.0), 5.0);
    }

    #[test]
    fn data_urls() {
        let (mime, data) = parse_data_url("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!((mime.as_str(), &data[..]), ("image/png", &b"hello"[..]));
    }

    #[test]
    fn moderation_heuristic() {
        assert!(looks_moderated(r#"{"error":{"code":"moderation_blocked"}}"#));
        assert!(!looks_moderated(r#"{"error":"invalid api key"}"#));
    }

    #[test]
    fn error_messages() {
        assert_eq!(error_message(r#"{"error":{"message":"bad key"}}"#), "bad key");
        assert_eq!(error_message(r#"{"detail":"nope"}"#), "nope");
        assert_eq!(error_message("plain"), "plain");
    }
}
