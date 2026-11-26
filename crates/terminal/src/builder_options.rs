//! The inputs [`TerminalBuilder`](crate::TerminalBuilder)'s Zetta constructors
//! take as one value rather than positionally.
//!
//! [`TerminalSpawnOptions`] is what `TerminalBuilder::spawn` starts a child
//! from. Upstream's `TerminalBuilder::new` takes these positionally. Zetta
//! added a console palette, initial geometry, a PTY provider and a restore mode
//! on top, which took the list to nineteen arguments across four constructors
//! that differed only in which of them they defaulted. One owned bundle with
//! [`TerminalSpawnOptions::new`] supplying the defaults replaces all four.

use std::{path::PathBuf, process::ExitStatus, sync::Arc};

use async_channel::Sender;
use collections::HashMap;
use gpui::BackgroundExecutor;
use task::Shell;
use util::paths::PathStyle;

use crate::{
    ConsoleLaunchOptions, InitialTerminalSize, PtyProvider, TaskState,
    terminal_settings::{AlternateScroll, CursorShape as SettingsCursorShape},
};

/// Whether a terminal may fall back to a local PTY when `pty_provider` cannot
/// open one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PtyRequirement {
    /// Use the provider when there is one, and a local PTY otherwise or when the
    /// provider fails.
    #[default]
    LocalFallback,
    /// The PTY must come from `pty_provider`. Disk restoration uses this: a
    /// local fallback would silently create a different session and make the
    /// restored pane impossible to reconnect.
    ProviderRequired,
}

/// Everything a spawned terminal is started from, bar the `App` it is built in.
pub struct TerminalSpawnOptions {
    pub working_directory: Option<PathBuf>,
    pub task: Option<TaskState>,
    pub shell: Shell,
    pub env: HashMap<String, String>,
    pub cursor_shape: SettingsCursorShape,
    pub alternate_scroll: AlternateScroll,
    pub max_scroll_history_lines: Option<usize>,
    pub path_hyperlink_regexes: Vec<String>,
    pub path_hyperlink_timeout_ms: u64,
    pub is_remote_terminal: bool,
    pub window_id: u64,
    pub completion_tx: Option<Sender<Option<ExitStatus>>>,
    pub activation_script: Vec<String>,
    pub path_style: PathStyle,
    pub pty_provider: Option<Arc<dyn PtyProvider>>,
    pub initial_console_palette: Option<ConsoleLaunchOptions>,
    pub initial_bounds: Option<InitialTerminalSize>,
    pub pty_requirement: PtyRequirement,
}

impl TerminalSpawnOptions {
    /// `shell` in window `window_id`, with every optional input left empty:
    /// no task, no extra environment, default cursor and scrollback, no
    /// hyperlink patterns, a local PTY. Override the rest with struct update
    /// syntax.
    pub fn new(shell: Shell, window_id: u64, path_style: PathStyle) -> Self {
        Self {
            working_directory: None,
            task: None,
            shell,
            env: HashMap::default(),
            cursor_shape: SettingsCursorShape::default(),
            alternate_scroll: AlternateScroll::On,
            max_scroll_history_lines: None,
            path_hyperlink_regexes: Vec::new(),
            path_hyperlink_timeout_ms: 0,
            is_remote_terminal: false,
            window_id,
            completion_tx: None,
            activation_script: Vec::new(),
            path_style,
            pty_provider: None,
            initial_console_palette: None,
            initial_bounds: None,
            pty_requirement: PtyRequirement::LocalFallback,
        }
    }
}

/// How a terminal with no child of its own is displayed: the six inputs of
/// `TerminalBuilder::new_display_only`, which `new_byte_stream` forwards
/// unchanged. `new_display_only` keeps upstream's positional signature.
#[derive(Clone, Copy)]
pub struct DisplayOnlySettings<'a> {
    pub cursor_shape: SettingsCursorShape,
    pub alternate_scroll: AlternateScroll,
    pub max_scroll_history_lines: Option<usize>,
    pub window_id: u64,
    pub background_executor: &'a BackgroundExecutor,
    pub path_style: PathStyle,
}
