//! Spawning ffmpeg/ffprobe and making sense of what they print.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Mutex, OnceLock};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

use crate::{MediaError, MediaResult, Tools};

pub(crate) fn spawn(program: &Path, args: &[impl AsRef<OsStr>], stdout: bool) -> MediaResult<Child> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(if stdout { Stdio::piped() } else { Stdio::null() })
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => MediaError::ToolsMissing,
        _ => MediaError::Io(e),
    })
}

/// Keeps the last lines of stderr (ffmpeg can be chatty on long runs).
pub(crate) fn collect_stderr(child: &mut Child) -> JoinHandle<String> {
    let stderr = child.stderr.take().expect("piped stderr");
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tail = VecDeque::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if tail.len() == 60 {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        Vec::from(tail).join("\n")
    })
}

/// Runs to completion and returns stdout, or a useful error.
pub(crate) async fn output(program: &Path, args: &[impl AsRef<OsStr>]) -> MediaResult<Vec<u8>> {
    let mut child = spawn(program, args, true)?;
    let stderr = collect_stderr(&mut child);
    let mut stdout = vec![];
    child.stdout.take().expect("piped stdout").read_to_end(&mut stdout).await?;
    let status = child.wait().await?;
    let stderr = stderr.await.unwrap_or_default();
    if !status.success() {
        return Err(MediaError::Ffmpeg(summarize(&stderr, status)));
    }
    Ok(stdout)
}

/// The last few meaningful stderr lines, which is where ffmpeg explains itself.
pub(crate) fn summarize(stderr: &str, status: ExitStatus) -> String {
    const NOISE: [&str; 7] = [
        "Task finished with error code",
        "Conversion failed!",
        "Exiting normally",
        "Error opening output files",
        "Error opening output file",
        "Terminating thread",
        "Nothing was written into output file",
    ];
    // "[in#0 @ 0x7a0c58000] moov atom not found" → "moov atom not found"
    let unprefix = |l: &'_ str| -> String {
        match l.split_once("] ") {
            Some((ctx, rest)) if l.starts_with('[') && ctx.contains(" @ 0x") => rest.trim().to_string(),
            _ => l.to_string(),
        }
    };
    let lines: Vec<String> = stderr
        .lines()
        .map(|l| unprefix(l.trim()))
        .filter(|l| {
            l.chars().any(char::is_alphanumeric)
                && !NOISE.iter().any(|n| l.starts_with(n))
                && !l.starts_with("x265 [info]")
        })
        .collect();
    let mut tail: Vec<&str> = lines.iter().rev().take(4).rev().map(String::as_str).collect();
    tail.dedup();
    if tail.is_empty() { format!("ffmpeg exited with {status}") } else { tail.join("\n") }
}

/// What the local ffmpeg build can do.
#[derive(Debug, Clone, Default)]
pub struct Caps {
    /// Major version, 0 if unknown (e.g. git builds).
    pub version: u32,
    pub encoders: HashSet<String>,
}

impl Caps {
    pub fn new(version: u32, encoders: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self { version, encoders: encoders.into_iter().map(Into::into).collect() }
    }

    pub fn has(&self, encoder: &str) -> bool {
        self.encoders.contains(encoder)
    }

    /// First available encoder among `names`.
    pub fn pick<'a>(&self, names: &[&'a str]) -> Option<&'a str> {
        names.iter().copied().find(|n| self.has(n))
    }

    /// Asks ffmpeg once per binary; cached for the life of the process.
    pub async fn detect(tools: &Tools) -> MediaResult<Caps> {
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, Caps>>> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        if let Some(caps) = cache.lock().unwrap().get(&tools.ffmpeg) {
            return Ok(caps.clone());
        }
        let version = output(&tools.ffmpeg, &["-hide_banner", "-version"]).await?;
        let encoders = output(&tools.ffmpeg, &["-hide_banner", "-encoders"]).await?;
        let caps = Caps {
            version: parse_version(&String::from_utf8_lossy(&version)),
            encoders: parse_encoders(&String::from_utf8_lossy(&encoders)),
        };
        cache.lock().unwrap().insert(tools.ffmpeg.clone(), caps.clone());
        Ok(caps)
    }
}

fn parse_version(s: &str) -> u32 {
    // "ffmpeg version 9.0.2 ..." / "ffmpeg version n8.1.3-..." / "ffmpeg version N-1234-g..."
    s.split_whitespace()
        .nth(2)
        .map(|v| v.trim_start_matches('n'))
        .and_then(|v| v.split(['.', '-']).next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn parse_encoders(s: &str) -> HashSet<String> {
    // Lines look like " V....D libx264   libx264 H.264 / AVC ..." after a "------" separator.
    s.lines()
        .skip_while(|l| !l.trim_start().starts_with("------"))
        .skip(1)
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let flags = parts.next()?;
            (flags.len() == 6).then(|| parts.next().map(str::to_owned))?
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffmpeg_output() {
        assert_eq!(parse_version("ffmpeg version 9.0.2 Copyright (c)"), 9);
        assert_eq!(parse_version("ffmpeg version n8.1.3-12-gabc Copyright"), 8);
        assert_eq!(parse_version("ffmpeg version N-118000-gdeadbeef"), 0);
        let enc = parse_encoders(
            "Encoders:\n V..... = Video\n ------\n V....D libx264              libx264 H.264\n A....D aac   AAC\n",
        );
        assert!(enc.contains("libx264") && enc.contains("aac") && !enc.contains("="));
    }

    #[cfg(unix)]
    #[test]
    fn summarizes_stderr() {
        use std::os::unix::process::ExitStatusExt;
        let status = ExitStatus::from_raw(1 << 8);
        let stderr = "[in#0 @ 0x77a0c58000] moov atom not found\n\
                      [vf#0:0 @ 0x7c9b048000] Task finished with error code: -22 (Invalid argument)\n\
                      Error opening input file /x/a.mp4.\n.\nConversion failed!\n";
        assert_eq!(summarize(stderr, status), "moov atom not found\nError opening input file /x/a.mp4.");
        assert_eq!(summarize("", status), "ffmpeg exited with exit status: 1");
    }
}
