//! Silent mode: whether notifications and attention requests stay quiet,
//! combining the user's manual toggle with the desktop's own do-not-disturb
//! state where it exposes one.

use std::time::Duration;

#[cfg(not(target_os = "macos"))]
use gpui::AppContext;
use gpui::{App, Context, Entity, Window};
use terminal_view::TerminalView;

use crate::{
    RequestFocusStatusAccess, TerminalPane, ToggleSilentMode, ToggleTabSilentMode, Zetta,
    ZettaProcessState, process_zetta_entities,
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub(crate) use linux::LinuxSilentSource;
#[cfg(target_os = "linux")]
use linux::{detect_linux_system_silent_state_with_source, spawn_gnome_settings_watcher};
#[cfg(target_os = "macos")]
use macos::{detect_macos_focus_observation, request_focus_status_authorization};
#[cfg(target_os = "windows")]
use windows::detect_windows_system_silent_state;

/// Fallback cadence for desktops whose silence state has no change signal this
/// code can subscribe to.
const SILENT_MODE_POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SystemSilentState {
    #[default]
    Unknown,
    Inactive,
    Active,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(
    not(target_os = "macos"),
    expect(dead_code, reason = "only macOS reports a Focus status authorization")
)]
pub(crate) enum FocusStatusAccess {
    #[default]
    Unknown,
    NotDetermined,
    Denied,
    Restricted,
    Authorized,
    AuthorizedButUnavailable,
}

impl FocusStatusAccess {
    #[cfg(target_os = "macos")]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Unavailable",
            Self::NotDetermined => "Not requested",
            Self::Denied => "Denied",
            Self::Restricted => "Restricted",
            Self::Authorized => "Authorized",
            Self::AuthorizedButUnavailable => "Live status unavailable",
        }
    }

    pub(crate) fn tooltip(self) -> &'static str {
        match self {
            Self::Unknown => {
                "macOS Focus status is unavailable. Zetta falls back to manual Silent Mode."
            }
            Self::NotDetermined => {
                "Focus status access has not been requested. Use Request Focus Status Access."
            }
            Self::Denied => {
                "Focus status access was denied. Enable Zetta under System Settings > Privacy & Security > Focus."
            }
            Self::Restricted => {
                "macOS restricts Focus status access for Zetta. Zetta falls back to manual Silent Mode."
            }
            Self::Authorized => {
                "Focus status access is authorized and its live status is available. Zetta follows the app-specific Focus status; a Focus that allows Zetta through reports inactive. Manual Silent Mode remains available."
            }
            Self::AuthorizedButUnavailable => {
                "Focus access is authorized, but macOS did not provide a live status. Live Focus status also requires notification authorization and the Communication Notifications capability; Zetta falls back to manual Silent Mode."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SilentModeState {
    manual: bool,
    system: SystemSilentState,
    focus_status_access: FocusStatusAccess,
}

impl SilentModeState {
    pub(crate) fn effective(self) -> bool {
        self.manual || self.system == SystemSilentState::Active
    }

    pub(crate) fn system_active(self) -> bool {
        self.system == SystemSilentState::Active
    }

    pub(crate) fn focus_status_access(self) -> FocusStatusAccess {
        self.focus_status_access
    }

    pub(crate) fn toggle_manual(&mut self) -> bool {
        if self.system_active() {
            return false;
        }
        self.manual = !self.manual;
        true
    }

    fn observe_system(&mut self, observed: SystemSilentState) -> bool {
        if self.system == observed {
            return false;
        }
        self.system = observed;
        true
    }

    fn observe_focus_status_access(&mut self, observed: FocusStatusAccess) -> bool {
        if self.focus_status_access == observed {
            return false;
        }
        self.focus_status_access = observed;
        if observed != FocusStatusAccess::Authorized {
            self.system = SystemSilentState::Unknown;
        }
        true
    }
}

pub(crate) fn effective_silent_mode(cx: &App) -> bool {
    cx.has_global::<ZettaProcessState>() && cx.global::<ZettaProcessState>().silent_mode.effective()
}

pub(crate) fn combined_silent_mode(global_silent_mode: bool, tab_silent_mode: bool) -> bool {
    global_silent_mode || tab_silent_mode
}

fn toggle_tab_silent_mode_value(tab_silent_mode: &mut bool) -> bool {
    *tab_silent_mode = !*tab_silent_mode;
    *tab_silent_mode
}

#[cfg(feature = "notifications")]
pub(crate) fn system_silence_active_non_prompting() -> bool {
    detect_system_silent_state() == SystemSilentState::Active
}

impl Zetta {
    pub(crate) fn request_focus_status_access(
        &mut self,
        _: &RequestFocusStatusAccess,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(target_os = "macos")]
        if let Some(access) = request_focus_status_authorization() {
            let state = &mut cx.global_mut::<ZettaProcessState>().silent_mode;
            let system_was_active = state.system_active();
            if state.observe_focus_status_access(access) && system_was_active {
                let enabled = state.effective();
                cx.defer(move |cx| apply_silent_mode_to_process(enabled, cx));
            }
        }
        cx.notify();
    }

    pub(crate) fn toggle_silent_mode(
        &mut self,
        _: &ToggleSilentMode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((changed, enabled)) = cx.has_global::<ZettaProcessState>().then(|| {
            let state = &mut cx.global_mut::<ZettaProcessState>().silent_mode;
            let changed = state.toggle_manual();
            (changed, state.effective())
        }) else {
            return;
        };

        if changed {
            // The action is dispatched while this Zetta entity is leased by
            // GPUI. Defer the cross-window propagation until that lease has
            // been returned; otherwise the process entity list includes the
            // clicked Zetta and attempting to update it again panics.
            cx.defer(move |cx| apply_silent_mode_to_process(enabled, cx));
        } else {
            // The disabled title-bar control still needs to redraw if an action
            // reaches it while system Do Not Disturb is active.
            cx.notify();
        }
    }

    pub(crate) fn configure_terminal_view_silent_mode(
        &self,
        tab_id: u64,
        view: &Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        let tab_silent_mode = self.any_tab(tab_id).is_some_and(|tab| tab.silent_mode);
        let enabled = combined_silent_mode(effective_silent_mode(cx), tab_silent_mode);
        view.update(cx, |view, cx| {
            view.set_system_bell_enabled(!enabled, cx);
        });
    }

    pub(crate) fn toggle_tab_silent_mode(
        &mut self,
        _: &ToggleTabSilentMode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let global_silent_mode = effective_silent_mode(cx);
        let Some(tab) = self.current_tab_mut() else {
            return;
        };
        let tab_silent_mode = toggle_tab_silent_mode_value(&mut tab.silent_mode);
        let enabled = combined_silent_mode(global_silent_mode, tab_silent_mode);
        let views = tab
            .panes
            .iter()
            .flat_map(TerminalPane::all_views)
            .cloned()
            .collect::<Vec<_>>();
        for view in views {
            view.update(cx, |view, cx| {
                view.set_system_bell_enabled(!enabled, cx);
            });
        }
        cx.notify();
    }

    pub(crate) fn apply_silent_mode_to_views(
        &mut self,
        global_silent_mode: bool,
        cx: &mut Context<Self>,
    ) {
        let views = self
            .tabs
            .iter()
            .flat_map(|tab| {
                let enabled = combined_silent_mode(global_silent_mode, tab.silent_mode);
                tab.panes
                    .iter()
                    .flat_map(TerminalPane::all_views)
                    .cloned()
                    .map(move |view| (view, enabled))
            })
            .collect::<Vec<_>>();
        for (view, enabled) in views {
            view.update(cx, |view, cx| {
                view.set_system_bell_enabled(!enabled, cx);
            });
        }
        cx.notify();
    }

    /// Answers `zetta notify --silent`-style queries, which arrive over the
    /// process control socket. Protected sessions are excluded so the reply
    /// cannot be used to confirm that one exists.
    pub(crate) fn tab_silent_mode_by_attention_id(&self, attention_id: u64) -> Option<bool> {
        self.tabs
            .iter()
            .chain(self.background_sessions.iter_unprotected())
            .find(|tab| tab.attention_id == attention_id)
            .map(|tab| tab.silent_mode)
    }
}

fn apply_silent_mode_to_process(enabled: bool, cx: &mut App) {
    let entities = process_zetta_entities(cx);
    for entity in entities {
        entity.update(cx, |zetta, cx| {
            zetta.apply_silent_mode_to_views(enabled, cx);
        });
    }
}

pub(crate) fn start_observer(cx: &mut App) {
    // On GNOME the state we track is a dconf key, and dconf announces writes to
    // it. Subscribing lets the loop park until something actually changes rather
    // than waking every few seconds to re-read an unchanged value, and it also
    // reacts immediately instead of within one poll interval. It is created only
    // once a reading actually comes from that key, so desktops answering through
    // D-Bus never subscribe to signals they would just discard.
    #[cfg(target_os = "linux")]
    let mut settings_changes = None;
    #[cfg(target_os = "linux")]
    let mut subscribed = false;

    cx.spawn(async move |cx| {
        loop {
            #[cfg(target_os = "linux")]
            let source;

            let (observed, focus_status_access) = {
                #[cfg(target_os = "macos")]
                {
                    cx.update(|_| {
                        let (access, state) = detect_macos_focus_observation();
                        (state, Some(access))
                    })
                }
                #[cfg(target_os = "linux")]
                {
                    let (observed, observed_source) = cx
                        .background_spawn(async { detect_linux_system_silent_state_with_source() })
                        .await;
                    source = observed_source;
                    (observed, None)
                }
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    (
                        cx.background_spawn(async { detect_system_silent_state() })
                            .await,
                        None,
                    )
                }
            };
            cx.update(|cx| {
                if !cx.has_global::<ZettaProcessState>() {
                    return;
                }
                let state = &mut cx.global_mut::<ZettaProcessState>().silent_mode;
                let access_changed = focus_status_access
                    .is_some_and(|access| state.observe_focus_status_access(access));
                let system_changed = state.observe_system(observed);
                if access_changed || system_changed {
                    let enabled = state.effective();
                    apply_silent_mode_to_process(enabled, cx);
                }
            });

            // Only the dconf-backed reading has a subscription behind it. A
            // D-Bus property answered by KDE or XFCE still needs polling, and a
            // dead subscription falls back to polling for good.
            #[cfg(target_os = "linux")]
            if source == LinuxSilentSource::GnomeShowBanners {
                if !subscribed {
                    subscribed = true;
                    settings_changes = Some(cx.update(|cx| spawn_gnome_settings_watcher(cx)));
                }
                if let Some(changes) = settings_changes.as_ref() {
                    match changes.recv().await {
                        Ok(()) => continue,
                        Err(async_channel::RecvError) => settings_changes = None,
                    }
                }
            }

            cx.background_executor()
                .timer(SILENT_MODE_POLL_INTERVAL)
                .await;
        }
    })
    .detach();
}

/// One-shot reading for callers that only want the current value. The observer
/// uses the platform detectors directly on Linux and macOS, so this is needed
/// there only by the notification path.
#[cfg(any(
    feature = "notifications",
    all(not(target_os = "macos"), not(target_os = "linux"))
))]
fn detect_system_silent_state() -> SystemSilentState {
    #[cfg(target_os = "windows")]
    {
        detect_windows_system_silent_state()
    }
    #[cfg(target_os = "linux")]
    {
        detect_linux_system_silent_state_with_source().0
    }
    #[cfg(target_os = "macos")]
    {
        detect_macos_focus_observation().1
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        SystemSilentState::Unknown
    }
}

#[cfg(any(test, target_os = "windows"))]
fn classify_windows_dnd_query(result: Option<(i32, u32, u32)>) -> SystemSilentState {
    let Some((status, buffer_size, profile)) = result else {
        return SystemSilentState::Unknown;
    };
    if status < 0 || buffer_size != size_of::<u32>() as u32 {
        return SystemSilentState::Unknown;
    }
    match profile {
        0 => SystemSilentState::Inactive,
        1 | 2 => SystemSilentState::Active,
        _ => SystemSilentState::Unknown,
    }
}

/// dconf announces writes as `Notify(prefix, changes, tag)`. A single-key write
/// sends the whole key path as `prefix` with one empty change; a directory write
/// sends the directory as `prefix` and the keys relative to it. Rebuilding each
/// full path and comparing it against the key covers both, and because either
/// side may be the shorter one, an overlap in either direction counts.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn dconf_notify_affects(prefix: &str, changes: &[String], key: &str) -> bool {
    fn overlaps(path: &str, key: &str) -> bool {
        path.starts_with(key) || key.starts_with(path)
    }

    if changes.is_empty() {
        return overlaps(prefix, key);
    }

    changes
        .iter()
        .any(|change| overlaps(&format!("{prefix}{change}"), key))
}

#[cfg(any(target_os = "linux", test))]
fn parse_gnome_show_banners(value: &str) -> Option<SystemSilentState> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Some(SystemSilentState::Inactive),
        "false" => Some(SystemSilentState::Active),
        _ => None,
    }
}

#[cfg(any(test, target_os = "macos"))]
fn macos_focus_status(access: FocusStatusAccess, focused: Option<bool>) -> SystemSilentState {
    if access != FocusStatusAccess::Authorized {
        return SystemSilentState::Unknown;
    }
    match focused {
        Some(true) => SystemSilentState::Active,
        Some(false) => SystemSilentState::Inactive,
        None => SystemSilentState::Unknown,
    }
}

#[cfg(test)]
#[path = "tests/silent_mode.rs"]
mod tests;
