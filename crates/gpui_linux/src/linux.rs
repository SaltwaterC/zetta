mod dispatcher;
mod display_connection;
mod headless;
mod keyboard;
#[cfg(any(feature = "wayland", feature = "x11"))]
mod main_task_watchdog;
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
pub(crate) use display_connection::{Backend, DisplayConnection, select_backend};
pub(crate) use headless::*;
pub(crate) use keyboard::*;
#[cfg(any(feature = "wayland", feature = "x11"))]
pub(crate) use main_task_watchdog::MainTaskWatchdog;
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

use gpui::WindowingModes;

/// Returns the default platform implementation for the current OS.
///
/// A windowed platform connects to the display server the process environment names, or starts
/// headless when it names none, and can switch between those modes later. A headless platform
/// stays headless.
pub fn current_platform(headless: bool) -> Rc<dyn gpui::Platform> {
    current_platform_with_path_prompt(headless).0
}

/// [`current_platform`], together with its [`DirectoryPathPrompt`].
pub fn current_platform_with_path_prompt(
    headless: bool,
) -> (Rc<dyn gpui::Platform>, DirectoryPathPrompt) {
    let platform = LinuxPlatform::new(default_modes(headless));
    // Zetta patch: see `LinuxPlatform::connect_early`.
    platform.connect_early();
    with_path_prompt(platform)
}

fn default_modes(headless: bool) -> WindowingModes {
    if headless || std::env::var_os("ZED_HEADLESS").is_some() {
        WindowingModes::HEADLESS
    } else {
        WindowingModes::all()
    }
}

/// Returns a platform that may switch among `allowed_modes`.
///
/// It starts windowed in the process's own environment, or headless if that names no allowed
/// display server. Set another initial mode with [`gpui::Application::with_windowing`].
pub fn linux_platform(allowed_modes: WindowingModes) -> Rc<dyn gpui::Platform> {
    Rc::new(LinuxPlatform::new(allowed_modes))
}

fn with_path_prompt(platform: LinuxPlatform) -> (Rc<dyn gpui::Platform>, DirectoryPathPrompt) {
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
        let (platform, prompt) = with_path_prompt(LinuxPlatform::new(WindowingModes::HEADLESS));
        let weak = Rc::downgrade(&platform);

        drop(platform);

        assert!(weak.upgrade().is_none());
        drop(prompt);
    }
}
