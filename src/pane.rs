//! Panes and the tabs that hold them.
//!
//! The root owns `TerminalPane` itself — a pane's identity, its profile, its
//! directory, and the settings a spawn is made with. The rest is split by what
//! it acts on: `pane/layout.rs` (the split tree), `pane/tab.rs` (the tab and
//! what it shows), `pane/stack.rs` (the command terminals sharing a pane's
//! region), and `pane/overlay.rs` (the overlay's text and colour model).

use super::*;

pub(crate) const MAX_PANES_PER_TAB: usize = 64;
pub(crate) const MAX_CONCURRENT_MULTI_COMMAND_SPAWNS: usize = 4;
pub(crate) const TERMINAL_SPAWN_NOTIFY_INTERVAL: Duration = Duration::from_millis(16);
pub(crate) const PANE_OUTPUT_DEFAULT_FILENAME: &str = "terminal-output.txt";
pub(crate) const PANE_SPLIT_RATIO_SCALE: u16 = 1_000;
pub(crate) const DEFAULT_PANE_SPLIT_RATIO: u16 = PANE_SPLIT_RATIO_SCALE / 2;
const PANE_ROTATION_AREA_EPSILON: f64 = 1e-9;
/// Below this score gap, two directional-navigation candidates are treated
/// as equally close and disambiguated by recency instead of tree order.
const ADJACENT_PANE_TIE_EPSILON: f32 = 1e-4;

pub(crate) fn terminal_size_label(columns: usize, rows: usize) -> String {
    format!("{columns} × {rows}")
}

pub(crate) fn can_add_panes(current: usize, additional: usize) -> bool {
    current
        .checked_add(additional)
        .is_some_and(|total| total <= MAX_PANES_PER_TAB)
}

pub(crate) fn begin_coalesced_notification(pending: &mut bool) -> bool {
    if *pending {
        false
    } else {
        *pending = true;
        true
    }
}

pub(crate) fn begin_pane_output_save(in_progress: &mut bool) -> bool {
    if *in_progress {
        false
    } else {
        *in_progress = true;
        true
    }
}

pub(crate) fn finish_pane_output_save(in_progress: &mut bool) {
    *in_progress = false;
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn prepare_pane_launches<T>(
    pane_ids: impl IntoIterator<Item = u64>,
    mut prepare: impl FnMut(u64) -> T,
) -> Vec<(u64, T)> {
    pane_ids
        .into_iter()
        .map(|pane_id| (pane_id, prepare(pane_id)))
        .collect()
}

pub(crate) fn pane_layout_from_configured_template(
    templates: &HashMap<String, PaneSplitTemplateConfig>,
    name: &str,
    pane_ids: &mut impl Iterator<Item = u64>,
) -> Option<PaneLayout> {
    templates
        .get(name)
        .map(|template| PaneLayout::from_template(&template.layout, pane_ids))
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zetta)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenProfile {
    pub(crate) slot: usize,
}

pub(crate) struct TerminalPane {
    pub(crate) id: u64,
    /// Stable subprocess routing identity. Unlike `id`, this is not changed
    /// when a tab moves to a window whose pane-id namespace must be remapped.
    pub(crate) routing_id: u64,
    pub(crate) label_number: usize,
    pub(crate) generated_label: Option<String>,
    pub(crate) custom_label: Option<String>,
    /// Free-form text shown over this pane's terminal content. Ephemeral:
    /// never written to `config.json`, so it is lost when the pane closes.
    pub(crate) overlay_text: Option<String>,
    /// Font size for `overlay_text`; falls back to [`OverlayFontSize::DEFAULT`].
    pub(crate) overlay_font_size: Option<OverlayFontSize>,
    /// Opacity for `overlay_text`, from `0.0` to `1.0`; falls back to a
    /// partly transparent default.
    pub(crate) overlay_opacity: Option<f32>,
    /// Text color for `overlay_text`; falls back to the theme's text color.
    pub(crate) overlay_color: Option<gpui::Hsla>,
    pub(crate) profile: Profile,
    /// A session-scoped theme selected explicitly for this pane. Kept on the
    /// logical pane rather than in window state so transfers and id remapping
    /// cannot detach the choice from the terminal it belongs to.
    pub(crate) theme_override: Option<String>,
    /// Environment overrides requested by a pane split template. Keeping the
    /// unexpanded overrides on the pane lets template application determine
    /// whether the active terminal actually needs to be restarted.
    pub(crate) environment_overrides: HashMap<String, String>,
    pub(crate) terminal: Option<Entity<Terminal>>,
    pub(crate) view: Option<Entity<TerminalView>>,
    pub(crate) error: Option<String>,
    /// Sanitized metadata for an unexpected interactive-terminal exit.
    pub(crate) exit: Option<BackgroundPaneExit>,
    /// Set when the original interactive shell has exited while stacked
    /// entries are still retaining this pane's region.
    pub(crate) base_exited: bool,
    pub(crate) wsl_cwd_file: Option<PathBuf>,
    pub(crate) pending_command: Option<String>,
    /// The command most recently reported by shell integration. Unlike a
    /// foreground process argv this is shell input the user actually started,
    /// so it is safe to offer as restore prefill.
    pub(crate) active_command: Option<String>,
    /// Automatically detected linked-worktree name for this pane's
    /// interactive-shell directory.
    pub(crate) detected_worktree_title: Option<String>,
    /// Shell directory associated with the current detection generation.
    pub(crate) worktree_detection_directory: Option<PathBuf>,
    pub(crate) worktree_detection_generation: u64,
    /// Whether the current detection directory came from an authoritative
    /// shell CWD (or the shell-owned process CWD), so a non-worktree result
    /// may clear the detected title.
    pub(crate) worktree_detection_can_clear: bool,
    /// Command terminals that share this pane's layout region. They are
    /// intentionally not part of [`PaneLayout`]: only the selected entry is
    /// expanded, while the others occupy compact status rows.
    pub(crate) stack: PaneStack,
}

pub(crate) fn select_current_directory(
    reported_directory: Option<PathBuf>,
    process_directory: Option<PathBuf>,
    foreground_process_is_shell: bool,
    prefer_reported_directory: bool,
) -> Option<(PathBuf, bool)> {
    if prefer_reported_directory {
        reported_directory
            .map(|directory| (directory, true))
            .or_else(|| process_directory.map(|directory| (directory, foreground_process_is_shell)))
    } else if foreground_process_is_shell {
        process_directory
            .or(reported_directory)
            .map(|directory| (directory, true))
    } else {
        reported_directory
            .map(|directory| (directory, true))
            .or_else(|| process_directory.map(|directory| (directory, false)))
    }
}

#[cfg(windows)]
fn shell_reports_current_directory(shell: &Shell) -> bool {
    if msys2_profile(shell).is_some() || cygwin_profile(shell).is_some() {
        return true;
    }
    let (Shell::Program(program) | Shell::WithArguments { program, .. }) = shell else {
        // Shell::System is resolved to a tracked native shell before spawn.
        return matches!(shell, Shell::System);
    };
    program.rsplit(['/', '\\']).next().is_some_and(|name| {
        [
            "cmd",
            "cmd.exe",
            "powershell",
            "powershell.exe",
            "pwsh",
            "pwsh.exe",
        ]
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
    })
}

#[cfg(not(windows))]
fn shell_reports_current_directory(_shell: &Shell) -> bool {
    false
}

pub(crate) struct TerminalSpawnSettings {
    pub(crate) cursor_shape: terminal::terminal_settings::CursorShape,
    pub(crate) alternate_scroll: terminal::terminal_settings::AlternateScroll,
    pub(crate) max_scroll_history_lines: Option<usize>,
    pub(crate) path_hyperlink_regexes: Vec<String>,
    pub(crate) path_hyperlink_timeout_ms: u64,
}

pub(crate) struct QueuedTerminalLaunch {
    pub(crate) tab_id: u64,
    pub(crate) pane_id: u64,
    pub(crate) profile: Profile,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) wsl_directory: Option<String>,
    pub(crate) wsl_cwd_file: Option<PathBuf>,
    pub(crate) terminal_theme: Option<Arc<Theme>>,
    pub(crate) settings: Arc<TerminalSpawnSettings>,
}

pub(crate) struct BoundedLaunchQueue<T> {
    pending: VecDeque<T>,
    in_flight: usize,
    limit: usize,
}

impl<T> BoundedLaunchQueue<T> {
    pub(crate) fn new(limit: usize) -> Self {
        assert!(limit > 0, "a launch queue must allow at least one launch");
        Self {
            pending: VecDeque::new(),
            in_flight: 0,
            limit,
        }
    }

    pub(crate) fn extend(&mut self, launches: impl IntoIterator<Item = T>) {
        self.pending.extend(launches);
    }

    pub(crate) fn pop_ready(&mut self) -> Option<T> {
        if self.in_flight >= self.limit {
            return None;
        }
        let launch = self.pending.pop_front()?;
        self.in_flight += 1;
        Some(launch)
    }

    pub(crate) fn complete(&mut self) {
        self.in_flight = self
            .in_flight
            .checked_sub(1)
            .expect("only an in-flight launch can complete");
    }
}

impl TerminalSpawnSettings {
    pub(crate) fn current(cx: &App) -> Self {
        let settings = TerminalSettings::get_global(cx);
        Self {
            cursor_shape: settings.cursor_shape,
            alternate_scroll: settings.alternate_scroll,
            max_scroll_history_lines: settings.max_scroll_history_lines,
            path_hyperlink_regexes: settings.path_hyperlink_regexes.clone(),
            path_hyperlink_timeout_ms: settings.path_hyperlink_timeout_ms,
        }
    }

    pub(crate) fn path_hyperlink_regexes(&mut self, final_spawn: bool) -> Vec<String> {
        clone_or_take_for_final_spawn(&mut self.path_hyperlink_regexes, final_spawn)
    }
}

pub(crate) fn clone_or_take_for_final_spawn<T: Clone + Default>(
    value: &mut T,
    final_spawn: bool,
) -> T {
    if final_spawn {
        std::mem::take(value)
    } else {
        value.clone()
    }
}

impl TerminalPane {
    pub(crate) fn new(id: u64, profile: Profile) -> Self {
        Self {
            id,
            routing_id: id,
            label_number: 0,
            generated_label: None,
            custom_label: None,
            overlay_text: None,
            overlay_font_size: None,
            overlay_opacity: None,
            overlay_color: None,
            profile,
            theme_override: None,
            environment_overrides: HashMap::new(),
            terminal: None,
            view: None,
            error: None,
            exit: None,
            base_exited: false,
            wsl_cwd_file: None,
            pending_command: None,
            active_command: None,
            detected_worktree_title: None,
            worktree_detection_directory: None,
            worktree_detection_generation: 0,
            worktree_detection_can_clear: false,
            stack: PaneStack::default(),
        }
    }

    pub(crate) fn with_label_number(mut self, label_number: usize) -> Self {
        self.label_number = label_number;
        self
    }

    pub(crate) fn with_generated_label(mut self, label: String) -> Self {
        self.generated_label = Some(label);
        self
    }

    pub(crate) fn with_pending_command(mut self, command: Option<String>) -> Self {
        self.pending_command = command;
        self
    }

    pub(crate) fn with_wsl_cwd_file(mut self, file: Option<PathBuf>) -> Self {
        self.wsl_cwd_file = file;
        self
    }

    pub(crate) fn with_environment_overrides(
        mut self,
        environment: HashMap<String, String>,
    ) -> Self {
        self.environment_overrides = environment;
        self
    }

    pub(crate) fn label(&self) -> String {
        self.custom_label
            .clone()
            .or_else(|| self.generated_label.clone())
            .unwrap_or_else(|| format!("Pane {}", self.label_number))
    }

    pub(crate) fn wsl_working_directory(&self, cx: &App) -> Option<String> {
        if !is_wsl_shell(&self.profile.command) {
            return None;
        }
        if let Some(directory) = self.terminal.as_ref().and_then(|terminal| {
            terminal
                .read(cx)
                .reported_working_directory()
                .map(str::to_owned)
        }) {
            return Some(directory);
        }

        let path = self.wsl_cwd_file.as_ref()?;
        let directory = fs::read_to_string(path).ok()?;
        let directory = directory.trim_end_matches(['\r', '\n']);
        (directory.starts_with('/') && !directory.contains(['\r', '\n', '\0']))
            .then(|| directory.to_owned())
    }

    /// Selects the native directory represented by this pane and whether that
    /// directory is authoritative for shell-owned state.
    ///
    /// Tracked Windows shells report their provider directory explicitly. That
    /// marker wins because PowerShell's Win32 process CWD can remain at the
    /// launch directory after `Set-Location`; process inspection is the
    /// fallback when no marker is available. For untracked shells, process CWD
    /// is preferred while the shell owns the foreground. Once a child is
    /// running, a process-only result is explicitly non-authoritative. MSYS2
    /// and Cygwin report POSIX paths even though the rest of the application
    /// operates on native Windows paths.
    pub(crate) fn current_directory(&self, cx: &App) -> Option<(PathBuf, bool)> {
        let terminal = self.terminal.as_ref()?.read(cx);
        let reported_directory = terminal.reported_working_directory().and_then(|directory| {
            if let Some((root, _)) = msys2_profile(&self.profile.command) {
                msys2_path_to_windows(&root, directory)
            } else if let Some((root, _)) = cygwin_profile(&self.profile.command) {
                cygwin_path_to_windows(&root, directory)
            } else {
                let directory = PathBuf::from(directory);
                directory.is_absolute().then_some(directory)
            }
        });
        let process_directory = terminal.process_working_directory();
        let foreground_process_is_shell = terminal.foreground_process_is_shell();
        select_current_directory(
            reported_directory,
            process_directory,
            foreground_process_is_shell,
            shell_reports_current_directory(&self.profile.command),
        )
    }

    pub(crate) fn working_directory(&self, cx: &App) -> Option<PathBuf> {
        // Windows process inspection sees the CWD of wsl.exe, not the Linux
        // shell. Keep the existing reported-WSL-path behavior for launch and
        // inheritance; native shell consumers use `current_directory` above.
        if is_wsl_shell(&self.profile.command) {
            return self.terminal.as_ref()?.read(cx).working_directory();
        }
        self.current_directory(cx).map(|(directory, _)| directory)
    }

    pub(crate) fn selected_view(&self) -> Option<Entity<TerminalView>> {
        self.stack.selected_view(self.view.as_ref())
    }

    pub(crate) fn selected_terminal(&self) -> Option<Entity<Terminal>> {
        self.stack.selected_terminal(self.terminal.as_ref())
    }

    pub(crate) fn theme_override(&self, selection: PaneStackSelection) -> Option<&str> {
        match selection {
            PaneStackSelection::Base => self.theme_override.as_deref(),
            PaneStackSelection::Stacked(id) => self
                .stack
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| entry.theme_override.as_deref()),
        }
    }

    /// Returns the stack entry represented by the focused terminal view. The
    /// model selection normally tracks focus, but keeping this fallback makes
    /// close actions reliable while a newly spawned stack view is replacing
    /// the host view's focus.
    pub(crate) fn focused_stack_selection(&self, window: &Window, cx: &App) -> PaneStackSelection {
        if let Some(entry) = self.stack.entries.iter().find(|entry| {
            entry
                .view
                .as_ref()
                .is_some_and(|view| view.focus_handle(cx).is_focused(window))
        }) {
            return PaneStackSelection::Stacked(entry.id);
        }
        if matches!(self.stack.selected, PaneStackSelection::Stacked(id) if self
            .stack
            .entries
            .iter()
            .any(|entry| entry.id == id))
        {
            return self.stack.selected;
        }
        if self
            .view
            .as_ref()
            .is_some_and(|view| view.focus_handle(cx).is_focused(window))
        {
            PaneStackSelection::Base
        } else {
            self.stack.selected
        }
    }

    pub(crate) fn all_terminals(&self) -> impl Iterator<Item = &Entity<Terminal>> {
        self.terminal.iter().chain(
            self.stack
                .entries
                .iter()
                .filter_map(|entry| entry.terminal.as_ref()),
        )
    }

    pub(crate) fn all_views(&self) -> impl Iterator<Item = &Entity<TerminalView>> {
        self.view.iter().chain(
            self.stack
                .entries
                .iter()
                .filter_map(|entry| entry.view.as_ref()),
        )
    }
}

mod layout;
mod overlay;
mod stack;
mod tab;

// `main.rs` pulls this module in with `use pane::*`, so the split is invisible
// to the rest of the crate: everything the submodules define is named
// `crate::pane::…` exactly as it was.
pub(crate) use layout::*;
pub(crate) use overlay::*;
pub(crate) use stack::*;
pub(crate) use tab::*;

#[cfg(test)]
#[path = "tests/pane.rs"]
mod tests;
