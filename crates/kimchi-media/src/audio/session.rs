//! The audio timeline as a ryolune session.

use std::path::{Path, PathBuf};

use kimchi_core::Project;

use crate::{MediaError, MediaResult, Tools};

/// Writes the project's audio timeline as a ryolune session at `path`: one ryolune audio track
/// per kimchi track with sound (its clips, fades, gains, effects, fader and pan), the markers,
/// and the cut's length. Returns the file written.
pub async fn write_ryolune_session(tools: &Tools, project: &Project, path: &Path) -> MediaResult<PathBuf> {
    let _ = (tools, project, path);
    Err(MediaError::Unsupported("writing a ryolune session isn't built yet".into()))
}
