//! stub
use std::path::Path;
use kimchi_core::Project;
use super::{ExportOptions, Imported};
use crate::{Report, Result};
pub fn read(path: &Path) -> Result<Imported> { let _ = path; Err("todo".into()) }
pub fn write(project: &Project, path: &Path, opts: &ExportOptions) -> Result<Report> { let _ = (project, path, opts); Err("todo".into()) }
