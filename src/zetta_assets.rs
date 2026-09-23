use std::borrow::Cow;

use anyhow::Context as _;
use assets::Assets as ZedAssets;
use gpui::{App, AssetSource, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[exclude = "**/*:Zone.Identifier"]
struct ZettaEmbeddedAssets;

pub struct ZettaAssets;

impl AssetSource for ZettaAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
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
        Ok(paths)
    }
}

impl ZettaAssets {
    pub fn load_fonts(&self, cx: &App) -> anyhow::Result<()> {
        let mut fonts = Vec::new();
        for path in self.list("fonts/")? {
            if path.ends_with(".ttf") {
                fonts.push(
                    self.load(&path)?
                        .with_context(|| format!("embedded font {path:?} is missing"))?,
                );
            }
        }
        cx.text_system().add_fonts(fonts)
    }
}
#[cfg(test)]
#[path = "tests/zetta_assets.rs"]
mod tests;
