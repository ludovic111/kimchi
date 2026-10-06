//! stub
use std::path::Path;
use kimchi_core::Project;
use super::{ExportOptions, Imported};
use crate::{Report, Result};
pub fn read(path: &Path, id: &str) -> Result<Imported> { let _ = (path, id); Err("todo".into()) }
pub fn write(project: &Project, path: &Path, id: &str, opts: &ExportOptions) -> Result<Report> { let _ = (project, path, id, opts); Err("todo".into()) }
