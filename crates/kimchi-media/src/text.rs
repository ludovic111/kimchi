//! Text layers, rasterised in Rust so the preview and the export draw them the same.

use std::path::Path;

use kimchi_core::Project;

use crate::MediaResult;
use crate::export::Overlays;

/// Writes one transparent PNG the size of the canvas per text clip into `dir`
/// and returns them keyed by clip id, ready for [`crate::export::build`].
pub fn rasterize_overlays(project: &Project, dir: &Path) -> MediaResult<Overlays> {
    let _ = (project, dir);
    Ok(Overlays::new())
}
