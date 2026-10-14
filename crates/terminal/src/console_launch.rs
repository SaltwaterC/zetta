//! Windows console launch options, kept together so backend selection happens
//! before any child is started. Palette and backend are independent of layout.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConsoleLaunchOptions {
    pub palette: crate::ConsolePalette,
    pub backend: zetta_profiles::ConsoleBackend,
}
impl From<crate::ConsolePalette> for ConsoleLaunchOptions {
    fn from(palette: crate::ConsolePalette) -> Self {
        Self {
            palette,
            backend: Default::default(),
        }
    }
}
