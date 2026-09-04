//! Opening, tracking and closing this process's windows.
//!
//! A window is registered with `ZettaProcessState` as it opens and unregistered
//! as it closes, which is what lets a control request, a reconnect, or a
//! project reload find a window to act on — and what decides whether the last
//! close should quit the process or leave it holding sessions.

use super::*;

/// What a launch does to the window once the `Zetta` view exists: apply a split
/// template, switch the performance instrumentation on, and hand the window's
/// activation token to the platform.
///
/// Separate from [`ZettaLaunchOptions`], which is what the view itself is built
/// from. Both derive `Default`, because most callers open a plain window and
/// set one or two of these.
#[derive(Default)]
pub(crate) struct WindowLaunchOptions {
    pub(crate) launch_split: Option<String>,
    pub(crate) enable_performance_overlay: bool,
    pub(crate) performance_report: Option<(PerformanceReportOptions, PerformanceReportStatus)>,
    pub(crate) profile_pane_stress: bool,
    pub(crate) activation_token: Option<String>,
}

pub(crate) fn open_zetta_window(
    config: Config,
    configuration_error: Option<String>,
    launch: ZettaLaunchOptions,
    window_launch: WindowLaunchOptions,
    cx: &mut App,
) -> Result<()> {
    let WindowLaunchOptions {
        launch_split,
        enable_performance_overlay,
        performance_report,
        profile_pane_stress,
        activation_token,
    } = window_launch;
    let options = zetta_window_options(cx);
    let window_handle = cx
        .open_window(options, move |window, cx| {
            window.set_window_title("Zetta");
            let zetta = cx.new(|cx| Zetta::new(config, configuration_error, launch, window, cx));
            track_zetta_window(&zetta, window, cx);
            prepare_background_tabs_before_window_close(&zetta, window, cx);
            if let Some(name) = launch_split {
                zetta.update(cx, |zetta, cx| {
                    zetta.apply_pane_split_template(&ApplyPaneSplitTemplate { name }, window, cx);
                });
            }
            if profile_pane_stress {
                zetta.update(cx, |zetta, cx| {
                    zetta.configure_pane_profile_stress(window, cx);
                });
            }
            if enable_performance_overlay {
                zetta.update(cx, |zetta, cx| {
                    zetta.toggle_performance_overlay(&TogglePerformanceOverlay, window, cx);
                });
            }
            if let Some((options, status)) = performance_report {
                zetta.update(cx, |zetta, cx| {
                    zetta.start_performance_report(options, status, cx);
                });
            }
            zetta
        })
        .context("opening Zetta window")?;
    if let Some(activation_token) = activation_token {
        window_handle.update(cx, |_, window, _| {
            gpui_platform::activate_window_with_token(window, &activation_token);
        })?;
    }
    if let Ok(surface) = std::env::var("ZETTA_SCREENSHOT_SURFACE") {
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(3))
                .await;
            window_handle
                .update(cx, |zetta, window, cx| match surface.as_str() {
                    "palette" => {
                        zetta.toggle_command_palette(&ToggleCommandPalette, window, cx);
                        if let Some(palette) = zetta.command_palette.as_mut() {
                            palette.query.insert("tab");
                            palette.refresh_matches();
                        }
                    }
                    "theme" => {
                        zetta.open_theme_picker(crate::ThemeScope::Pane, window, cx);
                        if let Some(picker) = zetta.theme_picker.as_mut() {
                            picker.query.insert("dark");
                            picker.refresh_matches();
                        }
                    }
                    "icons" => {
                        zetta.open_tab_icon_picker(0, window, cx);
                        if let Some(picker) = zetta.tab_icon_picker.as_mut() {
                            picker.query.insert("term");
                        }
                    }
                    "search" => {
                        zetta.search_tab_scrollback(&SearchTabScrollback, window, cx);
                        if let Some(search) = zetta.tab_search.as_mut() {
                            search.query.insert("zetta");
                        }
                    }
                    "command" => {
                        zetta.toggle_multi_command(&ToggleMultiCommand, window, cx);
                    }
                    "rename" => {
                        zetta.rename_tab(&RenameTab, window, cx);
                    }
                    "overlay" => {
                        zetta.set_pane_overlay(&SetPaneOverlay, window, cx);
                    }
                    "settings" => {
                        zetta.toggle_settings(&ToggleSettings, window, cx);
                        zetta.focus_settings_input(
                            crate::settings_ui::SettingsInput::Configuration(
                                ConfigTextField::FontSize,
                            ),
                            window,
                            cx,
                        );
                    }
                    _ => {}
                })
                .ok();
        })
        .detach();
    }
    cx.activate(true);
    Ok(())
}

fn zetta_window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(ZETTA_MINIMUM_WINDOW_SIZE),
        is_resizable: true,
        is_minimizable: true,
        app_id: Some(ZETTA_APP_ID.to_owned()),
        titlebar: Some(TitlebarOptions {
            title: Some("Zetta".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.), px(9.))),
        }),
        app_owns_titlebar_drag: true,
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    }
}

fn track_zetta_window(zetta: &Entity<Zetta>, window: &Window, cx: &mut App) {
    if cx.has_global::<ZettaProcessState>() {
        let runner_id = zetta.read(cx).background_sessions.runner_id();
        let process = cx.global_mut::<ZettaProcessState>();
        process
            .windows
            .insert(window.window_handle().window_id(), zetta.clone());
        process.runners.insert(runner_id, zetta.clone());
    }
}

fn prepare_background_tabs_before_window_close(
    zetta: &Entity<Zetta>,
    window: &mut Window,
    cx: &mut App,
) {
    let zetta = zetta.downgrade();
    window.on_window_should_close(cx, move |_, cx| {
        zetta
            .update(cx, |zetta, cx| {
                zetta.prepare_for_background_window_close(cx);
            })
            .ok();
        true
    });
}

pub(crate) fn process_zetta_entities(cx: &App) -> Vec<Entity<Zetta>> {
    if !cx.has_global::<ZettaProcessState>() {
        return Vec::new();
    }
    let process = cx.global::<ZettaProcessState>();
    process
        .windows
        .values()
        .chain(process.dormant.iter())
        .cloned()
        .collect()
}

// GNOME Shell refreshes its desktop-entry cache asynchronously. When the
// primary Exec line changes, its app cache can drop the old ShellApp without
// retracking windows that still belong to it. Re-publishing the app ID after
// the cache refresh makes Mutter notify the window tracker, which then
// associates the window with the current Zetta.desktop entry.
#[cfg(target_os = "linux")]
const LINUX_DESKTOP_REASSOCIATION_DELAY: Duration = Duration::from_secs(10);

#[cfg(target_os = "linux")]
pub(crate) fn schedule_linux_desktop_window_reassociation(cx: &mut App) {
    let generation = {
        let process = cx.global_mut::<ZettaProcessState>();
        process.linux_desktop_reassociation_generation = process
            .linux_desktop_reassociation_generation
            .wrapping_add(1);
        process.linux_desktop_reassociation_generation
    };
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        executor.timer(LINUX_DESKTOP_REASSOCIATION_DELAY).await;
        let window_ids = cx.update(|cx| {
            let process = cx.global::<ZettaProcessState>();
            (process.linux_desktop_reassociation_generation == generation)
                .then(|| process.windows.keys().copied().collect::<Vec<_>>())
        });
        let Some(window_ids) = window_ids else {
            return;
        };
        cx.update(|cx| {
            for window_id in &window_ids {
                let _ = gpui::WindowHandle::<Zetta>::new(*window_id).update(cx, |_, window, _| {
                    // Mutter emits `notify::wm-class` for every Wayland
                    // `set_app_id`, even when the value is unchanged. GNOME
                    // Shell uses that notification to retrack the window,
                    // so republish the stable ID after its desktop cache has
                    // loaded the new entry. Passing through a temporary ID
                    // can race with frame throttling and briefly creates a
                    // second, unrelated app identity.
                    window.set_app_id(ZETTA_APP_ID);
                    window.refresh();
                });
            }
        });
    })
    .detach();
}

pub(crate) fn reload_projects_in_other_windows(current_window: WindowId, cx: &mut App) {
    if !cx.has_global::<ZettaProcessState>() {
        return;
    }
    let (windows, dormant) = {
        let process = cx.global::<ZettaProcessState>();
        (
            process
                .windows
                .keys()
                .filter(|window_id| **window_id != current_window)
                .copied()
                .collect::<Vec<_>>(),
            process.dormant.clone(),
        )
    };
    for window_id in windows {
        if let Err(error) = gpui::WindowHandle::<Zetta>::new(window_id)
            .update(cx, |zetta, window, cx| zetta.reload_projects(window, cx))
            .and_then(|result| result)
        {
            log::error!("could not reload projects in another Zetta window: {error:#}");
        }
    }
    for zetta in dormant {
        if let Err(error) = zetta.update(cx, |zetta, _| {
            zetta.reload_project_registry_without_window()
        }) {
            log::error!("could not reload projects in a dormant Zetta window: {error:#}");
        }
    }
}

pub(crate) fn zetta_for_runner(runner_id: u64, cx: &App) -> Option<Entity<Zetta>> {
    if !cx.has_global::<ZettaProcessState>() {
        return None;
    }
    cx.global::<ZettaProcessState>()
        .runners
        .get(&runner_id)
        .cloned()
}

pub(super) fn should_quit_after_window_closed(
    window_count: usize,
    dormant_runner_count: usize,
) -> bool {
    window_count == 0 && dormant_runner_count == 0
}

pub(super) fn zetta_quit_mode() -> gpui::QuitMode {
    gpui::QuitMode::Explicit
}

pub(crate) fn quit_zetta_process(cx: &mut App) {
    cx.global::<ZettaProcessState>()
        .control_server
        .begin_shutdown();
    shutdown_multiplexer_if_idle(cx);
    cx.quit();
}

/// Stops the multiplexer if this was the last Zetta process relying on it.
///
/// Safe to call unconditionally on every quit: the daemon is the one that
/// knows whether it is idle, and it refuses `Request::Shutdown` while it
/// still holds any session — a background one, or a live pane belonging to
/// another Zetta process that has not quit yet. So this is a no-op whenever
/// anything still depends on the daemon, and only actually stops it once
/// nothing does.
pub(super) fn shutdown_multiplexer_if_idle(cx: &App) {
    if cx.has_global::<ZettaProcessState>() && cx.global::<ZettaProcessState>().no_mux {
        return;
    }
    match zmux::client::Client::connect_existing() {
        Ok(Some(client)) => {
            if let Err(error) = client.shutdown() {
                log::debug!("multiplexer still needed, not stopped: {error:#}");
            }
        }
        Ok(None) => {}
        Err(error) => log::debug!("checking for a running multiplexer to stop: {error:#}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowOpenTarget {
    Existing(WindowId),
    Dormant,
    Fresh,
}

fn select_window_open_target(
    existing_window: Option<WindowId>,
    has_dormant_session: bool,
) -> WindowOpenTarget {
    if let Some(window_id) = existing_window {
        WindowOpenTarget::Existing(window_id)
    } else if has_dormant_session {
        WindowOpenTarget::Dormant
    } else {
        WindowOpenTarget::Fresh
    }
}

pub(crate) fn open_dormant_or_new_window(cx: &mut App) -> Result<()> {
    let (target, config, configuration_error, no_mux) = {
        let process = cx.global::<ZettaProcessState>();
        (
            select_window_open_target(
                process.windows.keys().next().copied(),
                !process.dormant.is_empty(),
            ),
            process.config.clone(),
            process.configuration_error.clone(),
            process.no_mux,
        )
    };
    if let WindowOpenTarget::Existing(window_id) = target {
        gpui::WindowHandle::<Zetta>::new(window_id).update(cx, |zetta, window, cx| {
            zetta.resume_hidden_window(window, cx);
        })?;
        cx.activate(true);
        return Ok(());
    }
    let dormant = matches!(target, WindowOpenTarget::Dormant)
        .then(|| cx.global_mut::<ZettaProcessState>().dormant.pop())
        .flatten();
    if let Some(zetta) = dormant {
        let zetta_for_window = zetta.clone();
        match cx.open_window(zetta_window_options(cx), move |window, cx| {
            window.set_window_title("Zetta");
            zetta_for_window.update(cx, |zetta, cx| zetta.attach_to_reopened_window(window, cx));
            track_zetta_window(&zetta_for_window, window, cx);
            prepare_background_tabs_before_window_close(&zetta_for_window, window, cx);
            zetta_for_window
        }) {
            Ok(_) => (),
            Err(error) => {
                cx.global_mut::<ZettaProcessState>().dormant.push(zetta);
                return Err(error).context("opening Zetta window");
            }
        };
        cx.activate(true);
        Ok(())
    } else {
        open_zetta_window(
            config,
            configuration_error,
            ZettaLaunchOptions {
                no_mux,
                ..Default::default()
            },
            WindowLaunchOptions::default(),
            cx,
        )
    }
}

/// Opens a new OS window from process-wide configuration without selecting a
/// dormant entity or applying a project from another window.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn open_fresh_zetta_window(cx: &mut App) -> Result<()> {
    open_fresh_zetta_window_with_profile_and_activation_token(cx, None, None)
}

pub(super) fn open_fresh_zetta_window_with_profile_and_activation_token(
    cx: &mut App,
    profile_name: Option<String>,
    activation_token: Option<String>,
) -> Result<()> {
    let (config, configuration_error, no_mux) = {
        let process = cx.global::<ZettaProcessState>();
        (
            process.config.clone(),
            process.configuration_error.clone(),
            process.no_mux,
        )
    };
    let initial_profile = select_launch_profile(&config, profile_name.as_deref())?;
    open_zetta_window(
        config,
        configuration_error,
        ZettaLaunchOptions {
            initial_profile,
            no_mux,
            ..Default::default()
        },
        WindowLaunchOptions {
            activation_token,
            ..Default::default()
        },
        cx,
    )
}

#[cfg(windows)]
pub(super) fn open_windows_handoff_window(
    request: crate::windows_integration::WindowsHandoffRequest,
    cx: &mut App,
) -> Result<()> {
    let (config, configuration_error, no_mux) = {
        let process = cx.global::<ZettaProcessState>();
        (
            process.config.clone(),
            process.configuration_error.clone(),
            process.no_mux,
        )
    };
    open_zetta_window(
        config,
        configuration_error,
        ZettaLaunchOptions {
            no_mux,
            initial_launch: Some(crate::app::TerminalLaunch::Handoff(request)),
            ..Default::default()
        },
        WindowLaunchOptions::default(),
        cx,
    )
}

pub(super) fn handle_zetta_window_closed(cx: &mut App, window_id: WindowId) {
    let entity = cx
        .global_mut::<ZettaProcessState>()
        .windows
        .remove(&window_id);
    if let Some(entity) = entity {
        entity.update(cx, |zetta, cx| {
            zetta.prepare_for_background_window_close(cx);
        });
        let (has_background_sessions, runner_id) = {
            let entity_state = entity.read(cx);
            (
                !entity_state.background_sessions.is_empty(),
                entity_state.background_sessions.runner_id(),
            )
        };
        if has_background_sessions {
            cx.global_mut::<ZettaProcessState>().dormant.push(entity);
        } else {
            cx.global_mut::<ZettaProcessState>()
                .runners
                .remove(&runner_id);
        }
    }
    let process = cx.global::<ZettaProcessState>();
    if should_quit_after_window_closed(process.windows.len(), process.dormant.len()) {
        quit_zetta_process(cx);
    }
}

pub(super) fn focus_visible_tab_by_attention_id(cx: &mut App, attention_id: u64) -> bool {
    let windows = cx
        .global::<ZettaProcessState>()
        .windows
        .keys()
        .copied()
        .collect::<Vec<_>>();
    windows.into_iter().any(|window_id| {
        gpui::WindowHandle::<Zetta>::new(window_id)
            .update(cx, |zetta, window, cx| {
                if zetta.has_visible_tab_by_attention_id(attention_id) {
                    window.activate_window();
                    zetta.focus_tab_by_attention_id(attention_id, window, cx)
                } else {
                    false
                }
            })
            .unwrap_or(false)
    })
}

pub(super) fn reconnect_window_id(
    runner_id: u64,
    attention_id: Option<u64>,
    cx: &App,
) -> Option<WindowId> {
    let process = cx.global::<ZettaProcessState>();
    if let Some(attention_id) = attention_id {
        return process.windows.iter().find_map(|(window_id, zetta)| {
            zetta
                .read(cx)
                .has_visible_tab_by_attention_id(attention_id)
                .then_some(*window_id)
        });
    }
    let runner = process.runners.get(&runner_id)?;
    process.windows.iter().find_map(|(window_id, zetta)| {
        (zetta.entity_id() == runner.entity_id()).then_some(*window_id)
    })
}

#[cfg(test)]
#[path = "../tests/startup/window.rs"]
mod tests;
