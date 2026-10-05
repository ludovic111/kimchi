//! Bundled assets: lucide icons (ISC, `assets/icons/LICENSE.lucide.txt`), the logos of the
//! services kimchi works with (`assets/logos/SOURCES.md`) and the fonts.

use std::borrow::Cow;

use gpui::{App, AssetSource, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/*.svg"]
#[include = "logos/*.png"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(Self::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Self::iter().filter(|p| p.starts_with(path)).map(SharedString::from).collect())
    }
}

/// Registers Manrope and IBM Plex Mono (shared with the text renderer in kimchi-media).
pub fn load_fonts(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = kimchi_media::text::bundled_fonts().into_iter().map(Cow::Borrowed).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("couldn't load the bundled fonts: {e}");
    }
}

/// An icon by lucide name, e.g. `icon_path("play")`.
pub fn icon_path(name: &str) -> SharedString {
    format!("icons/{name}.svg").into()
}
