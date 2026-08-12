use std::time::Duration;

#[cfg(target_os = "linux")]
use std::process::Command;

#[cfg(not(target_os = "macos"))]
use gpui::AppContext;
use gpui::{App, Context, Entity, Window};
use terminal_view::TerminalView;

use crate::{
    RequestFocusStatusAccess, TerminalPane, ToggleSilentMode, ToggleTabSilentMode, Zetta,
    ZettaProcessState, process_zetta_entities,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SystemSilentState {
    #[default]
    Unknown,
    Inactive,
    Active,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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
        let tab_silent_mode = self
            .tabs
            .iter()
            .chain(self.background_sessions.iter())
            .find(|tab| tab.id == tab_id)
            .is_some_and(|tab| tab.silent_mode);
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
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
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
            zetta.apply_silent_mode_to_views(enabled, cx)
        });
    }
}

pub(crate) fn start_observer(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            let (observed, focus_status_access) = {
                #[cfg(target_os = "macos")]
                {
                    cx.update(|_| {
                        let (access, state) = detect_macos_focus_observation();
                        (state, Some(access))
                    })
                }
                #[cfg(not(target_os = "macos"))]
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
            cx.background_executor().timer(Duration::from_secs(5)).await;
        }
    })
    .detach();
}

fn detect_system_silent_state() -> SystemSilentState {
    #[cfg(target_os = "windows")]
    {
        return detect_windows_system_silent_state();
    }
    #[cfg(target_os = "linux")]
    {
        return detect_linux_system_silent_state();
    }
    #[cfg(target_os = "macos")]
    {
        return detect_macos_system_silent_state();
    }
    #[allow(unreachable_code)]
    SystemSilentState::Unknown
}

// Windows 10 1803 replaced "Quiet Hours" with Focus Assist, and never shipped
// a supported API to read its live profile: SHQueryUserNotificationState's
// only DND-adjacent value, QUNS_QUIET_TIME, is the automatic one-hour grace
// period after a fresh login/upgrade, not the user's Focus Assist toggle. The
// only place that toggle is observable is the per-user CloudStore cache that
// Settings itself reads from, an undocumented REG_BINARY blob embedding a
// UTF-16LE `Microsoft.QuietHoursProfile.*` profile name. Try that first and
// fall back to the legacy API (still valid for QUNS_BUSY/PRESENTATION_MODE)
// when the key is missing (never provisioned until Focus Assist is toggled
// once) or the blob doesn't parse.
#[cfg(target_os = "windows")]
const QUIET_HOURS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\Cache\DefaultAccount\$$windows.data.notifications.quiethourssettings\Current";
#[cfg(target_os = "windows")]
const QUIET_HOURS_VALUE: &str = "Data";

#[cfg(target_os = "windows")]
fn detect_windows_system_silent_state() -> SystemSilentState {
    let quiet_hours = windows_registry::CURRENT_USER
        .open(QUIET_HOURS_KEY)
        .and_then(|key| key.get_value(QUIET_HOURS_VALUE))
        .map(|value| parse_quiet_hours_profile(value.as_ref()))
        .unwrap_or(SystemSilentState::Unknown);
    if quiet_hours != SystemSilentState::Unknown {
        return quiet_hours;
    }
    detect_windows_notification_state()
}

#[cfg(any(test, target_os = "windows"))]
fn parse_quiet_hours_profile(data: &[u8]) -> SystemSilentState {
    let wide = data
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let decoded = String::from_utf16_lossy(&wide);
    if decoded.contains("Microsoft.QuietHoursProfile.Unrestricted") {
        SystemSilentState::Inactive
    } else if decoded.contains("Microsoft.QuietHoursProfile.PriorityOnly")
        || decoded.contains("Microsoft.QuietHoursProfile.AlarmsOnly")
    {
        SystemSilentState::Active
    } else {
        SystemSilentState::Unknown
    }
}

#[cfg(target_os = "windows")]
fn detect_windows_notification_state() -> SystemSilentState {
    use windows::Win32::UI::Shell::SHQueryUserNotificationState;

    match unsafe { SHQueryUserNotificationState() } {
        Ok(state) => windows_notification_state(state.0),
        Err(_) => SystemSilentState::Unknown,
    }
}

#[cfg(any(test, target_os = "windows"))]
fn windows_notification_state(state: i32) -> SystemSilentState {
    // QUNS_RUNNING_D3D_FULL_SCREEN is intentionally inactive: fullscreen by
    // itself is not Do Not Disturb. QUNS_BUSY is the Windows state used for
    // presentation/blocked-notification modes and is therefore active.
    match state {
        2 | 4 | 6 => SystemSilentState::Active, // BUSY, PRESENTATION_MODE, QUIET_TIME
        3 | 5 | 7 => SystemSilentState::Inactive, // FULL_SCREEN, ACCEPTS_NOTIFICATIONS, APP
        _ => SystemSilentState::Unknown,
    }
}

#[cfg(target_os = "linux")]
fn detect_linux_system_silent_state() -> SystemSilentState {
    futures::executor::block_on(async {
        if let Some(inhibited) = query_kde_inhibited().await {
            return if inhibited {
                SystemSilentState::Active
            } else {
                SystemSilentState::Inactive
            };
        }
        if let Some(inhibited) = query_xfce_inhibited().await {
            return if inhibited {
                SystemSilentState::Active
            } else {
                SystemSilentState::Inactive
            };
        }
        gnome_show_banners_state()
    })
}

#[cfg(target_os = "linux")]
async fn query_kde_inhibited() -> Option<bool> {
    query_dbus_bool(
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.DBus.Properties",
        "Get",
        &("org.freedesktop.Notifications", "Inhibited"),
    )
    .await
}

#[cfg(target_os = "linux")]
async fn query_xfce_inhibited() -> Option<bool> {
    query_dbus_bool(
        "org.xfce.Xfconf",
        "/org/xfce/Xfconf",
        "org.xfce.Xfconf",
        "GetProperty",
        &("xfce4-notifyd", "/do-not-disturb"),
    )
    .await
}

#[cfg(target_os = "linux")]
async fn query_dbus_bool<B>(
    destination: &str,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> Option<bool>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    let connection = zbus::Connection::session().await.ok()?;
    let reply = connection
        .call_method(Some(destination), path, Some(interface), method, body)
        .await
        .ok()?;
    let (value,): (zbus::zvariant::OwnedValue,) = reply.body().deserialize().ok()?;
    bool::try_from(value).ok()
}

#[cfg(any(target_os = "linux", test))]
fn parse_gnome_show_banners(value: &str) -> Option<SystemSilentState> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Some(SystemSilentState::Inactive),
        "false" => Some(SystemSilentState::Active),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn gnome_show_banners_state() -> SystemSilentState {
    let output = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.notifications", "show-banners"])
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| parse_gnome_show_banners(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or(SystemSilentState::Unknown)
}

#[cfg(target_os = "macos")]
fn is_packaged_gui_launch() -> bool {
    let Some(executable) = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.canonicalize().ok())
    else {
        return false;
    };
    let Some(macos) = executable.parent() else {
        return false;
    };
    let Some(contents) = macos.parent() else {
        return false;
    };
    let Some(bundle) = contents.parent() else {
        return false;
    };
    macos.file_name().and_then(|name| name.to_str()) == Some("MacOS")
        && contents.file_name().and_then(|name| name.to_str()) == Some("Contents")
        && bundle.extension().and_then(|extension| extension.to_str()) == Some("app")
}

#[cfg(target_os = "macos")]
fn request_focus_status_authorization() -> Option<FocusStatusAccess> {
    use block2::RcBlock;
    use objc2_intents::{INFocusStatusAuthorizationStatus, INFocusStatusCenter};

    if !is_packaged_gui_launch() {
        return None;
    }
    let center = unsafe { INFocusStatusCenter::defaultCenter() };
    let status = unsafe { center.authorizationStatus() };
    let access = focus_status_access(status);
    if status == INFocusStatusAuthorizationStatus::NotDetermined {
        let completion = RcBlock::new(|status: INFocusStatusAuthorizationStatus| {
            eprintln!("zetta: macOS Focus status authorization result: {status:?}");
        });
        unsafe {
            center.requestAuthorizationWithCompletionHandler(Some(&completion));
        }
    }
    Some(if access == FocusStatusAccess::Authorized {
        detect_macos_focus_observation().0
    } else {
        access
    })
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

#[cfg(target_os = "macos")]
fn focus_status_access(
    status: objc2_intents::INFocusStatusAuthorizationStatus,
) -> FocusStatusAccess {
    use objc2_intents::INFocusStatusAuthorizationStatus;

    if status == INFocusStatusAuthorizationStatus::NotDetermined {
        FocusStatusAccess::NotDetermined
    } else if status == INFocusStatusAuthorizationStatus::Denied {
        FocusStatusAccess::Denied
    } else if status == INFocusStatusAuthorizationStatus::Restricted {
        FocusStatusAccess::Restricted
    } else if status == INFocusStatusAuthorizationStatus::Authorized {
        FocusStatusAccess::Authorized
    } else {
        FocusStatusAccess::Unknown
    }
}

#[cfg(target_os = "macos")]
fn detect_macos_focus_observation() -> (FocusStatusAccess, SystemSilentState) {
    use objc2_intents::INFocusStatusCenter;

    if !is_packaged_gui_launch() {
        return (FocusStatusAccess::Unknown, SystemSilentState::Unknown);
    }
    let center = unsafe { INFocusStatusCenter::defaultCenter() };
    let access = focus_status_access(unsafe { center.authorizationStatus() });
    if access != FocusStatusAccess::Authorized {
        return (access, SystemSilentState::Unknown);
    }
    let Some(focused) = (unsafe { center.focusStatus().isFocused() }) else {
        return (
            FocusStatusAccess::AuthorizedButUnavailable,
            SystemSilentState::Unknown,
        );
    };
    (access, macos_focus_status(access, Some(focused.as_bool())))
}

#[cfg(target_os = "macos")]
fn detect_macos_system_silent_state() -> SystemSilentState {
    detect_macos_focus_observation().1
}

#[cfg(test)]
#[path = "tests/silent_mode.rs"]
mod tests;
