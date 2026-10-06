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

/// The interface face (`theme::SANS`), Chakra Petch (OFL, `fonts/chakrapetch/OFL.txt`).
const INTERFACE_FONTS: [&[u8]; 4] = [
    include_bytes!("../fonts/chakrapetch/ChakraPetch-Regular.ttf"),
    include_bytes!("../fonts/chakrapetch/ChakraPetch-Medium.ttf"),
    include_bytes!("../fonts/chakrapetch/ChakraPetch-SemiBold.ttf"),
    include_bytes!("../fonts/chakrapetch/ChakraPetch-Bold.ttf"),
];

/// Registers the interface face and the fonts shared with the text renderer in kimchi-media
/// (Manrope, IBM Plex Mono…).
pub fn load_fonts(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = INTERFACE_FONTS.into_iter().chain(kimchi_media::text::bundled_fonts()).map(Cow::Borrowed).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("couldn't load the bundled fonts: {e}");
    }
}

/// An icon by lucide name, e.g. `icon_path("play")`.
pub fn icon_path(name: &str) -> SharedString {
    format!("icons/{name}.svg").into()
}
