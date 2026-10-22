use std::borrow::Cow;

use anyhow::Context as _;
use assets::Assets as ZedAssets;
use gpui::{AssetSource, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[exclude = "**/*:Zone.Identifier"]
// Served from `UNCOMPRESSED_FONTS` instead.
#[exclude = "fonts/**/*.ttf"]
struct ZettaEmbeddedAssets;

/// Zetta's own fonts, embedded as they are.
///
/// Zed's workspace turns on `rust-embed`'s `compression`, which Cargo then applies
/// to every `RustEmbed` derive in the build, this one included: a compressed file
/// is inflated on its first load. Every launch loads the fonts, and inflating
/// these ~10 MiB took ~45 ms of the main thread before the first window could
/// open, and kept a heap copy of them for the life of the process.
const UNCOMPRESSED_FONTS: &[(&str, &[u8])] = &[
    (
        "fonts/meslo-lg-nerd-font/MesloLGS-NF-Regular.ttf",
        include_bytes!("../assets/fonts/meslo-lg-nerd-font/MesloLGS-NF-Regular.ttf"),
    ),
    (
        "fonts/meslo-lg-nerd-font/MesloLGS-NF-Bold.ttf",
        include_bytes!("../assets/fonts/meslo-lg-nerd-font/MesloLGS-NF-Bold.ttf"),
    ),
    (
        "fonts/meslo-lg-nerd-font/MesloLGS-NF-Italic.ttf",
        include_bytes!("../assets/fonts/meslo-lg-nerd-font/MesloLGS-NF-Italic.ttf"),
    ),
    (
        "fonts/meslo-lg-nerd-font/MesloLGS-NF-Bold-Italic.ttf",
        include_bytes!("../assets/fonts/meslo-lg-nerd-font/MesloLGS-NF-Bold-Italic.ttf"),
    ),
];

pub struct ZettaAssets;

impl AssetSource for ZettaAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = UNCOMPRESSED_FONTS.iter().find(|(font, _)| *font == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some(asset) = ZettaEmbeddedAssets::get(path) {
            return Ok(Some(asset.data));
        }
        ZedAssets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = ZedAssets.list(path)?;
        paths.extend(
            ZettaEmbeddedAssets::iter()
                .filter(|asset_path| asset_path.starts_with(path))
                .map(SharedString::from),
        );
        paths.extend(
            UNCOMPRESSED_FONTS
                .iter()
                .map(|(font, _)| *font)
                .filter(|font| font.starts_with(path))
                .map(SharedString::new_static),
        );
        Ok(paths)
    }
}

impl ZettaAssets {
    /// Every embedded font, Zed's and Zetta's. Zed's are compressed, so this is
    /// called where it can run alongside the platform's start-up
    /// (`startup::prefetch`).
    pub fn embedded_fonts(&self) -> anyhow::Result<Vec<Cow<'static, [u8]>>> {
        let mut fonts = Vec::new();
        for path in self.list("fonts/")? {
            if path.ends_with(".ttf") {
                fonts.push(
                    self.load(&path)?
                        .with_context(|| format!("embedded font {path:?} is missing"))?,
                );
            }
        }
        Ok(fonts)
    }
}
#[cfg(test)]
#[path = "tests/zetta_assets.rs"]
mod tests;
