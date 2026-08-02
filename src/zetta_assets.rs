use std::borrow::Cow;

use anyhow::Context as _;
use assets::Assets as ZedAssets;
use gpui::{App, AssetSource, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[exclude = "**/*:Zone.Identifier"]
struct ZettaEmbeddedAssets;

/// The default notification icon on platforms that do not derive the small
/// identity icon from an application bundle.
#[cfg(all(feature = "notifications", any(not(target_os = "macos"), test)))]
pub(crate) const NOTIFICATION_ICON_ASSET_PATH: &str = "icons/zetta-terminal-icon-128.png";

#[cfg(all(feature = "notifications", any(not(target_os = "macos"), test)))]
pub(crate) fn embedded_notification_icon() -> Option<Cow<'static, [u8]>> {
    ZettaEmbeddedAssets::get(NOTIFICATION_ICON_ASSET_PATH).map(|asset| asset.data)
}

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
