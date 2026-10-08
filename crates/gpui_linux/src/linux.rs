mod dispatcher;
mod headless;
mod keyboard;
mod platform;
#[cfg(any(feature = "wayland", feature = "x11"))]
mod text_system;
#[cfg(feature = "wayland")]
mod wayland;
#[cfg(feature = "x11")]
mod x11;

#[cfg(any(feature = "wayland", feature = "x11"))]
mod xdg_desktop_portal;

pub use dispatcher::*;
pub(crate) use headless::*;
pub(crate) use keyboard::*;
pub(crate) use platform::*;
#[cfg(any(feature = "wayland", feature = "x11"))]
pub(crate) use text_system::*;
#[cfg(feature = "wayland")]
pub(crate) use wayland::*;
#[cfg(feature = "x11")]
pub(crate) use x11::*;

use std::{path::PathBuf, rc::Rc};

#[cfg(feature = "wayland")]
use std::sync::{Mutex, OnceLock};

#[cfg(feature = "wayland")]
static NEXT_ACTIVATION_TOKEN: OnceLock<Mutex<Option<String>>> = OnceLock::new();

#[cfg(feature = "wayland")]
pub(crate) fn set_next_activation_token(activation_token: &str) {
    *NEXT_ACTIVATION_TOKEN
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        (!activation_token.is_empty()).then(|| activation_token.to_owned());
}

#[cfg(not(feature = "wayland"))]
pub(crate) fn set_next_activation_token(_activation_token: &str) {}

#[cfg(feature = "wayland")]
pub(crate) fn take_next_activation_token() -> Option<String> {
    NEXT_ACTIVATION_TOKEN
        .get()
        .and_then(|token| token.lock().ok()?.take())
}

/// A path picker rooted at a starting directory.
///
/// `Platform::prompt_for_paths` cannot express one and `PathPromptOptions` is
/// upstream, so the platform hands this out beside itself instead.
pub type DirectoryPathPrompt = Rc<
    dyn Fn(
        Option<PathBuf>,
        gpui::PathPromptOptions,
    ) -> futures::channel::oneshot::Receiver<gpui::Result<Option<Vec<PathBuf>>>>,
>;

/// Returns the default platform implementation for the current OS.
pub fn current_platform(headless: bool) -> Rc<dyn gpui::Platform> {
    current_platform_with_path_prompt(headless).0
}

/// Returns the default platform implementation for the current OS, together with
/// its [`DirectoryPathPrompt`].
pub fn current_platform_with_path_prompt(
    headless: bool,
) -> (Rc<dyn gpui::Platform>, DirectoryPathPrompt) {
    #[cfg(feature = "x11")]
    use anyhow::Context as _;

    if headless {
        return with_path_prompt(LinuxPlatform {
            inner: HeadlessClient::new(),
        });
    }

    match gpui::guess_compositor() {
        #[cfg(feature = "wayland")]
        "Wayland" => with_path_prompt(LinuxPlatform {
            inner: WaylandClient::new(),
        }),

        #[cfg(feature = "x11")]
        "X11" => with_path_prompt(LinuxPlatform {
            inner: X11Client::new()
                .context("Failed to initialize X11 client.")
                .unwrap(),
        }),

        "Headless" => with_path_prompt(LinuxPlatform {
            inner: HeadlessClient::new(),
        }),
        _ => unreachable!(
            r#"At least one of the "wayland" or "x11" features must be enabled on gpui_linux or gpui_platform."#
        ),
    }
}

fn with_path_prompt<P: LinuxClient + 'static>(
    platform: LinuxPlatform<P>,
) -> (Rc<dyn gpui::Platform>, DirectoryPathPrompt) {
    let platform = Rc::new(platform);
    // Weak, because the application owns the platform and the prompt is kept in a thread-local.
    // Owning it from there kept the platform alive until thread-local teardown, where dropping
    // its GPU context touched wgpu's own, already destroyed, thread-locals and aborted the exit.
    let prompt = Rc::downgrade(&platform);
    (
        platform,
        Rc::new(move |directory, options| match prompt.upgrade() {
            Some(platform) => platform.prompt_for_paths_in(directory, options),
            // Nothing can ask once the application is gone; a dropped sender reads as cancelled.
            None => futures::channel::oneshot::channel().1,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_prompt_does_not_keep_the_platform_alive() {
        let (platform, prompt) = with_path_prompt(LinuxPlatform {
            inner: HeadlessClient::new(),
        });
        let weak = Rc::downgrade(&platform);

        drop(platform);

        assert!(weak.upgrade().is_none());
        drop(prompt);
    }
}
