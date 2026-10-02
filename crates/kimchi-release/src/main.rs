//! kimchi-release: the release pipeline's signing tool, so releases need neither Node nor the
//! Tauri CLI. Signatures and keys use the Tauri updater's format (base64 of minisign files), so
//! kimchi 0.1.x (Tauri) installs and the GPUI app's updater both accept them.
//!
//! ```text
//! kimchi-release keygen <out-prefix> [--password P]      throwaway key pair: <prefix>.key, <prefix>.key.pub
//! kimchi-release sign <file>… [--version V]               writes <file>.sig
//! kimchi-release verify <file> [--version V]              checks <file>.sig
//! kimchi-release manifest <dir> --version V --base-url U [--notes-file F] [--out latest.json]
//! ```
//!
//! `sign` reads the secret key from `TAURI_SIGNING_PRIVATE_KEY` (the key itself, or a path to it)
//! and its password from `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (may be empty). `verify` and
//! `manifest` check against kimchi's update key unless `KIMCHI_UPDATE_PUBKEY` names another one
//! (base64, or a path to a `.pub` file), which local test runs use.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::Engine;
use serde_json::{Value, json};

/// kimchi's update key; the same value as `kimchi_control::update::PUBLIC_KEY`.
const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDRDMDcxNTc4RjA1N0Q5QTcKUldTbjJWZndlQlVIVEZyRWNtU2tqeWxCaFNvTGQxbU1qczFIMGY5cUYzVVdBWkxOL0FvalJocEkK";

/// Release files the updaters download, and the `latest.json` keys each one serves. The Tauri
/// updater looks for `{os}-{arch}-{installer}` then `{os}-{arch}`.
const ASSETS: &[(&str, &[&str])] = &[
    ("kimchi_aarch64.app.tar.gz", &["darwin-aarch64-app", "darwin-aarch64"]),
    ("kimchi_x64.app.tar.gz", &["darwin-x86_64-app", "darwin-x86_64"]),
    // MSI installs of 0.1.x get the NSIS installer too (the Tauri updater runs any .exe as NSIS).
    ("kimchi_x64-setup.exe", &["windows-x86_64-nsis", "windows-x86_64-msi", "windows-x86_64"]),
    ("kimchi_amd64.AppImage", &["linux-x86_64-appimage", "linux-x86_64"]),
    ("kimchi_amd64.deb", &["linux-x86_64-deb"]),
];

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

fn unb64(what: &str, s: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.trim()).map_err(|_| format!("{what} isn't valid base64"))?;
    String::from_utf8(bytes).map_err(|_| format!("{what} isn't text"))
}

/// A value given inline or as a path to a file holding it.
fn inline_or_file(value: &str) -> String {
    let p = Path::new(value.trim());
    if value.len() < 1024 && p.is_file() { std::fs::read_to_string(p).unwrap_or_default().trim().to_string() } else { value.trim().to_string() }
}

fn secret_key() -> Result<minisign::SecretKey, String> {
    let raw = std::env::var("TAURI_SIGNING_PRIVATE_KEY").map_err(|_| "TAURI_SIGNING_PRIVATE_KEY is not set".to_string())?;
    let text = unb64("TAURI_SIGNING_PRIVATE_KEY", &inline_or_file(&raw))?;
    let password = std::env::var("TAURI_SIGNING_PRIVATE_KEY_PASSWORD").unwrap_or_default();
    let boxed = || minisign::SecretKeyBox::from_string(&text).map_err(|e| format!("not a minisign secret key: {e}"));
    match boxed()?.into_secret_key(Some(password)) {
        Ok(sk) => Ok(sk),
        Err(e) if e.to_string().contains("not encrypted") => boxed()?.into_unencrypted_secret_key().map_err(|e| e.to_string()),
        Err(e) => Err(format!("couldn't open the signing key: {e}")),
    }
}

fn public_key_b64() -> String {
    std::env::var("KIMCHI_UPDATE_PUBKEY").ok().filter(|v| !v.trim().is_empty()).map(|v| inline_or_file(&v)).unwrap_or_else(|| PUBLIC_KEY.into())
}

/// Signs `file` like `tauri signer sign`: the trusted comment names the file and the version.
fn sign(file: &Path, sk: &minisign::SecretKey, version: &str) -> Result<PathBuf, String> {
    let name = file.file_name().ok_or("no file name")?.to_string_lossy().into_owned();
    let data = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let trusted = format!("timestamp:{ts}\tfile:{name}\tversion:{version}");
    let sig = minisign::sign(None, sk, std::io::BufReader::new(data), Some(&trusted), Some("signature from tauri secret key")).map_err(|e| e.to_string())?;
    let out = sig_path(file);
    std::fs::write(&out, b64(&sig.to_string())).map_err(|e| e.to_string())?;
    Ok(out)
}

fn sig_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(".sig");
    PathBuf::from(s)
}

/// Checks `file` against its `.sig` and the update key, and returns the signed version.
fn verify(file: &Path, public_key_b64: &str) -> Result<String, String> {
    let sig_b64 = std::fs::read_to_string(sig_path(file)).map_err(|e| format!("{}.sig: {e}", file.display()))?;
    let pk = minisign_verify::PublicKey::decode(&unb64("public key", public_key_b64)?).map_err(|e| format!("public key: {e}"))?;
    let sig = minisign_verify::Signature::decode(&unb64("signature", &sig_b64)?).map_err(|e| format!("signature: {e}"))?;
    let mut v = pk.verify_stream(&sig).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut f = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = std::io::Read::read(&mut f, &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        v.update(&buf[..n]);
    }
    v.finalize().map_err(|e| format!("{}: {e}", file.display()))?;
    sig.trusted_comment().split('\t').find_map(|f| f.strip_prefix("version:")).map(|v| v.trim().to_string()).ok_or_else(|| format!("{}: the signature names no version", file.display()))
}

fn keygen(prefix: &Path, password: Option<String>) -> Result<(), String> {
    let kp = match &password {
        Some(p) => minisign::KeyPair::generate_encrypted_keypair(Some(p.clone())),
        None => minisign::KeyPair::generate_unencrypted_keypair(),
    }
    .map_err(|e| e.to_string())?;
    let pk = b64(&kp.pk.to_box().map_err(|e| e.to_string())?.to_string());
    let sk = b64(&kp.sk.to_box(Some(if password.is_some() { "rsign encrypted secret key" } else { "minisign secret key" })).map_err(|e| e.to_string())?.to_string());
    let mut pub_path = prefix.as_os_str().to_owned();
    pub_path.push(".key.pub");
    let mut key_path = prefix.as_os_str().to_owned();
    key_path.push(".key");
    std::fs::write(&key_path, sk).map_err(|e| e.to_string())?;
    std::fs::write(&pub_path, pk).map_err(|e| e.to_string())?;
    eprintln!("wrote {} and {}", Path::new(&key_path).display(), Path::new(&pub_path).display());
    Ok(())
}

/// Builds `latest.json` from the signed assets found in `dir`, checking every signature first.
fn manifest(dir: &Path, version: &str, base_url: &str, notes: &str, public_key_b64: &str) -> Result<Value, String> {
    let mut platforms = BTreeMap::new();
    for (asset, keys) in ASSETS {
        let file = dir.join(asset);
        if !file.is_file() {
            continue;
        }
        let signed = verify(&file, public_key_b64)?;
        if signed.trim_start_matches('v') != version.trim_start_matches('v') {
            return Err(format!("{asset} was signed for {signed}, not {version}"));
        }
        let signature = std::fs::read_to_string(sig_path(&file)).map_err(|e| e.to_string())?.trim().to_string();
        let url = format!("{}/{asset}", base_url.trim_end_matches('/'));
        for key in *keys {
            platforms.insert(key.to_string(), json!({ "signature": signature, "url": url }));
        }
        eprintln!("{asset}: {}", keys.join(", "));
    }
    if platforms.is_empty() {
        return Err(format!("no signed update assets in {} (expected any of: {})", dir.display(), ASSETS.iter().map(|a| a.0).collect::<Vec<_>>().join(", ")));
    }
    Ok(json!({ "version": version, "notes": notes, "pub_date": now_rfc3339(), "platforms": platforms }))
}

/// UTC now as `YYYY-MM-DDTHH:MM:SSZ`, without a date crate.
fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

struct Args {
    positional: Vec<String>,
    flags: BTreeMap<String, String>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut positional = vec![];
    let mut flags = BTreeMap::new();
    let mut it = args.peekable();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            let value = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
            flags.insert(name.to_string(), value);
        } else {
            positional.push(a);
        }
    }
    Ok(Args { positional, flags })
}

fn run() -> Result<(), String> {
    let mut argv = std::env::args().skip(1);
    let cmd = argv.next().unwrap_or_default();
    let a = parse(argv)?;
    let version = a.flags.get("version").cloned().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    match cmd.as_str() {
        "keygen" => keygen(Path::new(a.positional.first().ok_or("keygen <out-prefix>")?), a.flags.get("password").cloned()),
        "sign" => {
            if a.positional.is_empty() {
                return Err("sign <file>…".into());
            }
            let sk = secret_key()?;
            for f in &a.positional {
                let out = sign(Path::new(f), &sk, &version)?;
                eprintln!("signed {f} ({version}) -> {}", out.display());
            }
            Ok(())
        }
        "verify" => {
            let pk = public_key_b64();
            for f in &a.positional {
                let signed = verify(Path::new(f), &pk)?;
                if a.flags.contains_key("version") && signed.trim_start_matches('v') != version.trim_start_matches('v') {
                    return Err(format!("{f} was signed for {signed}, not {version}"));
                }
                eprintln!("{f}: good signature for {signed}");
            }
            Ok(())
        }
        "manifest" => {
            let dir = Path::new(a.positional.first().ok_or("manifest <dir> --version V --base-url U")?);
            let base = a.flags.get("base-url").ok_or("manifest needs --base-url")?;
            let notes = match a.flags.get("notes-file") {
                Some(f) => std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?.trim().to_string(),
                None => format!("kimchi {version}"),
            };
            let m = manifest(dir, &version, base, &notes, &public_key_b64())?;
            let text = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
            match a.flags.get("out") {
                Some(out) => std::fs::write(out, text + "\n").map_err(|e| format!("{out}: {e}")),
                None => {
                    println!("{text}");
                    Ok(())
                }
            }
        }
        _ => Err("usage: kimchi-release keygen|sign|verify|manifest (see the source header)".into()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kimchi-release: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_verifies_and_writes_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("test");
        keygen(&prefix, Some("pw".into())).unwrap();
        let sk_text = unb64("key", &std::fs::read_to_string(dir.path().join("test.key")).unwrap()).unwrap();
        let sk = minisign::SecretKeyBox::from_string(&sk_text).unwrap().into_secret_key(Some("pw".into())).unwrap();
        let pk = std::fs::read_to_string(dir.path().join("test.key.pub")).unwrap();

        let archive = dir.path().join("kimchi_aarch64.app.tar.gz");
        std::fs::write(&archive, b"archive").unwrap();
        sign(&archive, &sk, "0.2.0").unwrap();
        assert_eq!(verify(&archive, &pk).unwrap(), "0.2.0");
        // The real update key didn't sign it.
        assert!(verify(&archive, PUBLIC_KEY).is_err());

        let m = manifest(dir.path(), "0.2.0", "https://github.com/ludovic111/kimchi/releases/download/v0.2.0/", "notes", &pk).unwrap();
        assert_eq!(m["platforms"]["darwin-aarch64-app"]["url"], "https://github.com/ludovic111/kimchi/releases/download/v0.2.0/kimchi_aarch64.app.tar.gz");
        assert_eq!(m["platforms"]["darwin-aarch64"], m["platforms"]["darwin-aarch64-app"]);
        assert!(m["platforms"].get("windows-x86_64").is_none());
        assert!(manifest(dir.path(), "0.3.0", "u", "n", &pk).is_err(), "version mismatch must fail");

        std::fs::write(&archive, b"tampered").unwrap();
        assert!(verify(&archive, &pk).is_err());
    }

    #[test]
    fn formats_dates() {
        let d = now_rfc3339();
        assert_eq!(d.len(), 20);
        assert!(d.starts_with("20") && d.ends_with('Z'));
    }
}
