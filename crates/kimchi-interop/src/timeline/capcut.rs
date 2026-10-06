//! stub
use std::path::{Path, PathBuf};
use super::Imported;
use crate::Result;
pub fn read(path: &Path) -> Result<Imported> { let _ = path; Err("todo".into()) }
pub fn draft_file(dir: &Path) -> Option<PathBuf> { ["draft_content.json", "draft_info.json"].iter().map(|n| dir.join(n)).find(|p| p.is_file()) }
