use std::time::Duration;

#[cfg(target_os = "linux")]
use std::process::Command;

use gpui::{App, Context, Entity, Window};
use terminal_view::TerminalView;

use crate::{
    TerminalPane, ToggleSilentMode, ToggleTabSilentMode, Zetta, ZettaProcessState,
    process_zetta_entities,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SystemSilentState {
    #[default]
    Unknown,
    Inactive,
    Active,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SilentModeState {
    manual: bool,
    system: SystemSilentState,
}

impl SilentModeState {
    pub(crate) fn effective(self) -> bool {
        self.manual || self.system == SystemSilentState::Active
    }

    pub(crate) fn system_active(self) -> bool {
        self.system == SystemSilentState::Active
    }

    pub(crate) fn toggle_manual(&mut self) -> bool {
        if self.system_active() {
            return false;
        }
        self.manual = !self.manual;
        true
    }

    fn observe_system(&mut self, observed: SystemSilentState) -> bool {
        if observed == SystemSilentState::Unknown || self.system == observed {
            return false;
        }
        self.system = observed;
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

    pub(crate) fn tab_silent_mode_by_attention_id(&self, attention_id: u64) -> Option<bool> {
        self.tabs
            .iter()
            .chain(self.background_sessions.iter())
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
        #[cfg(target_os = "macos")]
        cx.update(request_focus_status_authorization);

        loop {
            let observed = {
                #[cfg(target_os = "macos")]
                {
                    cx.update(|_| detect_system_silent_state())
                }
                #[cfg(not(target_os = "macos"))]
                {
                    cx.background_spawn(async { detect_system_silent_state() })
                        .await
                }
            };
            cx.update(|cx| {
                if !cx.has_global::<ZettaProcessState>() {
                    return;
                }
                let changed = cx
                    .global_mut::<ZettaProcessState>()
                    .silent_mode
                    .observe_system(observed);
                if changed {
                    let enabled = cx.global::<ZettaProcessState>().silent_mode.effective();
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

#[cfg(target_os = "windows")]
fn detect_windows_system_silent_state() -> SystemSilentState {
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
fn request_focus_status_authorization(_: &mut App) {
    use block2::RcBlock;
    use objc2_intents::{INFocusStatusAuthorizationStatus, INFocusStatusCenter};

    if !is_packaged_gui_launch() {
        return;
    }
    let center = unsafe { INFocusStatusCenter::defaultCenter() };
    let status = unsafe { center.authorizationStatus() };
    if status != INFocusStatusAuthorizationStatus::NotDetermined {
        return;
    }
    let completion = RcBlock::new(|status: INFocusStatusAuthorizationStatus| {
        eprintln!("zetta: macOS Focus status authorization result: {status:?}");
    });
    unsafe {
        center.requestAuthorizationWithCompletionHandler(Some(&completion));
    }
}

#[cfg(any(test, target_os = "macos"))]
fn macos_focus_status(authorization_status: isize, focused: Option<bool>) -> SystemSilentState {
    if authorization_status != 3 {
        return SystemSilentState::Unknown;
    }
    match focused {
        Some(true) => SystemSilentState::Active,
        Some(false) => SystemSilentState::Inactive,
        None => SystemSilentState::Unknown,
    }
}

#[cfg(target_os = "macos")]
fn detect_macos_system_silent_state() -> SystemSilentState {
    use objc2_intents::INFocusStatusCenter;

    if !is_packaged_gui_launch() {
        return SystemSilentState::Unknown;
    }
    let center = unsafe { INFocusStatusCenter::defaultCenter() };
    let authorization_status = unsafe { center.authorizationStatus() }.0;
    if authorization_status != 3 {
        return SystemSilentState::Unknown;
    }
    let focused = unsafe { center.focusStatus().isFocused() }.map(|focused| focused.as_bool());
    macos_focus_status(authorization_status, focused)
}

#[cfg(test)]
#[path = "tests/silent_mode.rs"]
mod tests;
