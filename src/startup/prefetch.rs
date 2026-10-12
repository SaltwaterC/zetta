//! Start-up work that needs no GPUI state, started on threads of its own before
//! the platform starts so that the platform's own start-up hides it.
//!
//! Parsing the bundled themes and the user's themes, and asking GNOME for the
//! window-button layout, used to take about 13 ms of the main thread before the
//! first window could spawn its terminal. Connecting to the compositor and
//! scanning the system's fonts takes the platform about 9 ms, and none of this
//! work has to wait for it. Each piece is joined where it used to run, so a
//! thread that could not be started, or that panicked, leaves the work to be done
//! there as before.

use super::*;

use std::thread::{self, JoinHandle};

use theme::ThemeFamily;

/// The parsed themes a launch registers, being prepared on other threads.
pub(super) struct StartupPrefetch {
    bundled_themes: Option<JoinHandle<Vec<ThemeFamily>>>,
    user_themes: Option<JoinHandle<Result<Vec<ThemeFamily>>>>,
}

impl StartupPrefetch {
    /// Starts every piece. Call it before the platform starts.
    pub(super) fn start() -> Self {
        #[cfg(target_os = "linux")]
        crate::window_frame::prefetch_gnome_button_layout();
        Self {
            bundled_themes: spawn("theme prefetch", || {
                theme_settings::parse_bundled_themes(&ZettaAssets)
            }),
            user_themes: spawn("user theme prefetch", prepare_user_themes),
        }
    }

    /// The bundled themes, parsed. `None` leaves `theme_settings` to parse them.
    pub(super) fn bundled_themes(&mut self) -> Option<Vec<ThemeFamily>> {
        join(self.bundled_themes.take()?)
    }

    /// The user's themes, parsed, or parsed here when the thread is not there.
    pub(super) fn user_themes(&mut self) -> Result<Vec<ThemeFamily>> {
        self.user_themes
            .take()
            .and_then(join)
            .unwrap_or_else(prepare_user_themes)
    }
}

fn spawn<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<JoinHandle<T>> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(work)
        .map_err(|error| log::warn!("could not start the {name} thread: {error}"))
        .ok()
}

/// What the thread produced; `None` when it panicked, and the caller then does
/// the work on this thread instead.
fn join<T>(handle: JoinHandle<T>) -> Option<T> {
    let name = handle.thread().name().unwrap_or("prefetch").to_owned();
    handle
        .join()
        .map_err(|_| log::error!("the {name} thread panicked; doing its work on this thread"))
        .ok()
}

#[cfg(test)]
#[path = "../tests/startup/prefetch.rs"]
mod tests;
