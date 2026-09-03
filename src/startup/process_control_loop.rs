//! The process-control event loop.
//!
//! Every `zetta <subcommand>` that has to reach a running window arrives here:
//! `process_control.rs` decodes the request off the control socket, sends it
//! down the channel as a [`ProcessControlCommand`], and this loop applies it on
//! the main thread and answers on the request's `completion` sender.
//!
//! Two things hold across the handlers.
//!
//! - Every handler that answers a `completion` sender rejects rather than acts
//!   once the control server has begun shutting down, so a request that races a
//!   quit cannot leave a half-applied change behind. That is what
//!   [`accepting_control_requests`] guards. `open_urls` is the exception: it
//!   has no sender and never had the guard.
//! - Every handler answers exactly once. The three session handlers move their
//!   sender into a window update, so they answer the rejection themselves when
//!   no window took it.

use super::*;

use gpui::AsyncApp;
use std::sync::mpsc::Sender;

use crate::background_sessions::SessionSecret;

pub(super) async fn serve(
    mut control_rx: futures::channel::mpsc::UnboundedReceiver<ProcessControlCommand>,
    cx: &mut AsyncApp,
) {
    while let Some(command) = control_rx.next().await {
        dispatch(command, cx);
    }
}

fn dispatch(command: ProcessControlCommand, cx: &mut AsyncApp) {
    match command {
        #[cfg(windows)]
        ProcessControlCommand::OpenWindowsHandoff {
            request,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| open_windows_handoff(request, cx)));
        }
        #[cfg(target_os = "macos")]
        ProcessControlCommand::OpenUrls(urls) => cx.update(|cx| open_urls(&urls, cx)),
        ProcessControlCommand::ReloadConfiguration {
            config_path,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| reload_configuration(&config_path, cx)));
        }
        ProcessControlCommand::OpenWindow { completion } => {
            let _ = completion.send(cx.update(open_window));
        }
        ProcessControlCommand::OpenNewWindow {
            profile,
            activation_token,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| open_new_window(profile, activation_token, cx)));
        }
        ProcessControlCommand::OpenProject {
            root,
            working_directory,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| open_project(root, working_directory, cx)));
        }
        ProcessControlCommand::ReloadProjects { completion } => {
            let _ = completion.send(cx.update(reload_projects));
        }
        ProcessControlCommand::ReplacePane {
            request,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| replace_pane(request, cx)));
        }
        ProcessControlCommand::OpenCommand {
            request,
            working_directory,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| open_command(request, working_directory, cx)));
        }
        ProcessControlCommand::RunPane {
            request,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| run_pane(request, cx)));
        }
        ProcessControlCommand::RunShellCommand {
            request,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| run_shell_command(request, cx)));
        }
        ProcessControlCommand::RunWait {
            request,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| run_wait(request, cx)));
        }
        ProcessControlCommand::ListPaneLabels {
            attention_id,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| list_pane_labels(attention_id, cx)));
        }
        ProcessControlCommand::SetTabIcon { icon, completion } => {
            let _ = completion.send(cx.update(|cx| {
                with_any_window(cx, |zetta, _, cx| {
                    zetta.set_active_tab_icon_from_cli(icon, cx)
                })
                .unwrap_or(false)
            }));
        }
        ProcessControlCommand::SetTheme {
            scope,
            theme,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| {
                with_any_window(cx, |zetta, _, cx| zetta.set_theme(scope, theme, cx))
                    .unwrap_or(false)
            }));
        }
        ProcessControlCommand::ListThemes { completion } => {
            let _ = completion.send(cx.update(list_themes));
        }
        ProcessControlCommand::GetPaneTheme {
            attention_id,
            pane_id,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| pane_theme(attention_id, pane_id, cx)));
        }
        ProcessControlCommand::SetPaneOverlay {
            text,
            font_size,
            opacity,
            color,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| {
                with_any_window(cx, |zetta, _, cx| {
                    zetta.set_active_pane_overlay(text, font_size, opacity, color, cx)
                })
                .unwrap_or(false)
            }));
        }
        ProcessControlCommand::SetTabAttention {
            request,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| set_tab_attention(&request, cx)));
        }
        ProcessControlCommand::FocusTab {
            attention_id,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| {
                accepting_control_requests(cx)
                    && focus_visible_tab_by_attention_id(cx, attention_id)
            }));
        }
        ProcessControlCommand::SetTabName {
            request,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| {
                accepting_control_requests(cx)
                    && process_zetta_entities(cx).into_iter().any(|zetta| {
                        zetta.update(cx, |zetta, cx| zetta.set_tab_name(request.clone(), cx))
                    })
            }));
        }
        ProcessControlCommand::SetWorktreeName {
            request,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| {
                accepting_control_requests(cx)
                    && process_zetta_entities(cx).into_iter().any(|zetta| {
                        zetta.update(cx, |zetta, cx| zetta.set_worktree_name(request.clone(), cx))
                    })
            }));
        }
        ProcessControlCommand::GetSilentMode {
            attention_id,
            completion,
        } => {
            let _ = completion.send(cx.update(|cx| silent_mode(attention_id, cx)));
        }
        ProcessControlCommand::ReconnectSession {
            runner_id,
            session_id,
            attention_id,
            secret,
            completion,
        } => reconnect_session(runner_id, session_id, attention_id, secret, completion, cx),
        ProcessControlCommand::OpenRemoteSession {
            target,
            port,
            session_id,
            secret,
            completion,
        } => open_remote_session(target, port, session_id, secret, completion, cx),
        ProcessControlCommand::ResumeDiskSession {
            session_id,
            identity_paths,
            identity_passphrases,
            secret,
            completion,
        } => resume_disk_session(
            session_id,
            identity_paths,
            identity_passphrases,
            secret,
            completion,
            cx,
        ),
    }
}

/// Whether this process is still accepting control requests.
fn accepting_control_requests(cx: &App) -> bool {
    cx.has_global::<ZettaProcessState>()
        && cx
            .global::<ZettaProcessState>()
            .control_server
            .is_accepting()
}

/// The window a request that does not name one acts on: the active window when
/// there is one, otherwise any window this process still owns.
fn any_window_id(cx: &App) -> Option<WindowId> {
    cx.active_window()
        .map(|window| window.window_id())
        .or_else(|| {
            cx.global::<ZettaProcessState>()
                .windows
                .keys()
                .next()
                .copied()
        })
}

/// Runs `handler` against any window this process owns, opening or resuming one
/// when it has none.
///
/// `None` covers every way the request could not be applied — the process is
/// shutting down, no window could be opened, or the window went away before the
/// update ran — because each caller answers all three the same way.
fn with_any_window<T>(
    cx: &mut App,
    handler: impl FnOnce(&mut Zetta, &mut Window, &mut Context<Zetta>) -> T,
) -> Option<T> {
    if !accepting_control_requests(cx) {
        return None;
    }
    if cx.global::<ZettaProcessState>().windows.is_empty()
        && open_dormant_or_new_window(cx).is_err()
    {
        return None;
    }
    let window_id = cx
        .global::<ZettaProcessState>()
        .windows
        .keys()
        .next()
        .copied()?;
    gpui::WindowHandle::<Zetta>::new(window_id)
        .update(cx, handler)
        .ok()
}

#[cfg(windows)]
fn open_windows_handoff(
    request: crate::windows_integration::WindowsHandoffRequest,
    cx: &mut App,
) -> bool {
    if !accepting_control_requests(cx) {
        return false;
    }
    if cx.active_window().is_none() {
        let process = cx.global::<ZettaProcessState>();
        // A process with no window at all can put the handed-off console
        // straight into the window it opens, rather than opening an empty one
        // and then replacing its tab.
        if process.windows.is_empty() && process.dormant.is_empty() {
            return open_windows_handoff_window(request, cx).is_ok();
        }
        if open_dormant_or_new_window(cx).is_err() {
            return false;
        }
    }
    let Some(window_id) = any_window_id(cx) else {
        return false;
    };
    gpui::WindowHandle::<Zetta>::new(window_id)
        .update(cx, |zetta, window, cx| {
            zetta.open_windows_handoff(request, window, cx)
        })
        .unwrap_or(false)
}

/// macOS `open`/URL-scheme activation, which arrives through
/// `Application::on_open_urls` rather than the control socket, so there is no
/// sender to answer and no shutdown guard.
#[cfg(target_os = "macos")]
fn open_urls(urls: &[String], cx: &mut App) {
    if cx.active_window().is_none() && open_dormant_or_new_window(cx).is_err() {
        return;
    }
    let Some(window_id) = any_window_id(cx) else {
        return;
    };
    let _ = gpui::WindowHandle::<Zetta>::new(window_id).update(cx, |zetta, window, cx| {
        zetta.open_script_urls(urls, window, cx);
    });
}

fn reload_configuration(config_path: &str, cx: &mut App) -> bool {
    if !accepting_control_requests(cx) {
        return false;
    }
    // Only the process that owns this configuration file applies the reload;
    // another process watching a different file answers no and lets the client
    // keep looking.
    if crate::process_control::config_path_identity(
        &cx.global::<ZettaProcessState>().config.config_path,
    ) != config_path
    {
        return false;
    }
    match reload_process_configuration(cx) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("Could not reload {config_path}: {error:#}");
            false
        }
    }
}

fn open_window(cx: &mut App) -> bool {
    if !accepting_control_requests(cx) {
        return false;
    }
    if let Err(error) = reload_process_configuration_if_changed(cx) {
        eprintln!("Could not refresh configuration before opening a window: {error:#}");
    }

    match open_dormant_or_new_window(cx) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("Could not open the requested Zetta window: {error:#}");
            false
        }
    }
}

fn open_new_window(
    profile: Option<String>,
    activation_token: Option<String>,
    cx: &mut App,
) -> bool {
    if !accepting_control_requests(cx) {
        return false;
    }
    let profile_requested = profile.is_some();
    let refresh_error = reload_process_configuration_if_changed(cx).err();
    if let Some(error) = refresh_error.as_ref() {
        eprintln!("Could not refresh configuration before opening a fresh window: {error:#}");
    }
    // A named profile can only be resolved against configuration that actually
    // loaded, so a stale read rejects the request instead of opening a window
    // with the wrong profile. A request that names no profile is unaffected.
    if refresh_error.is_some() && profile_requested {
        return false;
    }
    match open_fresh_zetta_window_with_profile_and_activation_token(cx, profile, activation_token) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("Could not open the requested fresh Zetta window: {error:#}");
            false
        }
    }
}

fn open_project(root: PathBuf, working_directory: Option<PathBuf>, cx: &mut App) -> bool {
    if !accepting_control_requests(cx) {
        return false;
    }
    let registry = match ProjectRegistry::load() {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("Could not load project registry: {error:#}");
            return false;
        }
    };
    if !registry.contains(&root) {
        return false;
    }
    // A working directory is only honoured when it still resolves to the
    // project being opened, so a request cannot use one project's root to open
    // a directory belonging to another.
    let working_directory = match working_directory {
        Some(directory) => {
            let Ok(directory) = canonical_project_root(&directory) else {
                return false;
            };
            if resolve_registered_project_root(&directory, &registry)
                .is_none_or(|resolved| !paths_equal(&resolved, &root))
            {
                return false;
            }
            Some(directory)
        }
        None => None,
    };
    if cx.active_window().is_none()
        && let Err(error) = open_dormant_or_new_window(cx)
    {
        eprintln!("Could not open a window for project: {error:#}");
        return false;
    }
    let Some(window_id) = cx.active_window().map(|window| window.window_id()) else {
        return false;
    };
    gpui::WindowHandle::<Zetta>::new(window_id)
        .update(cx, |zetta, window, cx| {
            zetta.projects.registry = registry;
            zetta.open_project_tab_with_working_directory(root, working_directory, window, cx);
            true
        })
        .unwrap_or(false)
}

fn reload_projects(cx: &mut App) -> bool {
    if !accepting_control_requests(cx) {
        return false;
    }
    let windows = cx
        .global::<ZettaProcessState>()
        .windows
        .keys()
        .copied()
        .collect::<Vec<_>>();
    for window_id in windows {
        let applied = gpui::WindowHandle::<Zetta>::new(window_id)
            .update(cx, |zetta, window, cx| zetta.reload_projects(window, cx));
        if !matches!(applied, Ok(Ok(()))) {
            return false;
        }
    }
    let dormant = cx.global::<ZettaProcessState>().dormant.clone();
    for entity in dormant {
        if entity
            .update(cx, |zetta, _| {
                zetta.reload_project_registry_without_window()
            })
            .is_err()
        {
            return false;
        }
    }
    true
}

fn replace_pane(request: ReplacePaneRequest, cx: &mut App) -> bool {
    if !accepting_control_requests(cx) {
        return false;
    }
    // Replacing a pane only makes sense against the window the user is looking
    // at, so this never falls back to another window.
    let Some(window_id) = cx.active_window().map(|window| window.window_id()) else {
        return false;
    };
    gpui::WindowHandle::<Zetta>::new(window_id)
        .update(cx, |zetta, window, cx| {
            zetta.replace_active_pane_from_cli(request, window, cx)
        })
        .unwrap_or(false)
}

fn open_command(
    request: crate::command_panes::PaneCommand,
    working_directory: Option<PathBuf>,
    cx: &mut App,
) -> bool {
    if !accepting_control_requests(cx) {
        return false;
    }
    if cx.active_window().is_none() && open_dormant_or_new_window(cx).is_err() {
        return false;
    }
    let Some(window_id) = any_window_id(cx) else {
        return false;
    };
    gpui::WindowHandle::<Zetta>::new(window_id)
        .update(cx, |zetta, window, cx| {
            zetta
                .open_command_in_new_tab(request, working_directory, window, cx)
                .is_ok()
        })
        .unwrap_or(false)
}

fn run_pane(
    request: crate::command_panes::PaneCommand,
    cx: &mut App,
) -> std::result::Result<(), String> {
    if !accepting_control_requests(cx) {
        return Err("the Zetta process is shutting down".to_owned());
    }
    let Some(window_id) = cx.active_window().map(|window| window.window_id()) else {
        return Err("the running Zetta process has no active window".to_owned());
    };
    gpui::WindowHandle::<Zetta>::new(window_id)
        .update(cx, |zetta, window, cx| {
            zetta
                .run_command_pane(request, window, cx)
                .map_err(|error| format!("{error:#}"))
        })
        .map_err(|error| format!("{error:#}"))?
}

fn run_shell_command(
    request: crate::command_panes::ShellCommandRequest,
    cx: &mut App,
) -> std::result::Result<(), String> {
    if !accepting_control_requests(cx) {
        return Err("the Zetta process is shutting down".to_owned());
    }
    let Some(window_id) = cx.active_window().map(|window| window.window_id()) else {
        return Err("the running Zetta process has no active window".to_owned());
    };
    gpui::WindowHandle::<Zetta>::new(window_id)
        .update(cx, |zetta, window, cx| {
            zetta
                .run_shell_command(request, window, cx)
                .map_err(|error| format!("{error:#}"))
        })
        .map_err(|error| format!("{error:#}"))?
}

fn run_wait(
    request: RunWaitRequest,
    cx: &mut App,
) -> std::result::Result<crate::run_command::RunRegistration, String> {
    if !accepting_control_requests(cx) {
        return Err("the Zetta process is shutting down".to_owned());
    }
    // The wrapper is waiting on behalf of one specific pane, so the
    // registration goes to the entity that still holds its tab — including a
    // dormant one, whose panes keep running.
    for entity in process_zetta_entities(cx) {
        if !entity
            .read(cx)
            .has_tab_by_attention_id(request.owner.attention_id)
        {
            continue;
        }
        return entity
            .update(cx, |zetta, cx| {
                zetta
                    .register_run_wait(request, &process_run_registry(), cx)
                    .map_err(|error| format!("{error:#}"))
            })
            .map_err(|error| format!("{error:#}"));
    }
    Err("the originating Zetta tab is no longer available".to_owned())
}

fn list_pane_labels(
    attention_id: Option<u64>,
    cx: &mut App,
) -> std::result::Result<Vec<String>, String> {
    if !accepting_control_requests(cx) {
        return Err("the Zetta process is shutting down".to_owned());
    }
    // A request from inside a pane lists that tab's panes wherever the tab
    // lives; one from outside lists the active window's.
    if let Some(attention_id) = attention_id {
        for entity in process_zetta_entities(cx) {
            if !entity.read(cx).has_tab_by_attention_id(attention_id) {
                continue;
            }
            return Ok(entity.update(cx, |zetta, _| {
                zetta.command_pane_labels_for_attention(Some(attention_id))
            }));
        }
        return Err("the originating Zetta tab is no longer available".to_owned());
    }
    let Some(window_id) = cx.active_window().map(|window| window.window_id()) else {
        return Err("the running Zetta process has no active window".to_owned());
    };
    gpui::WindowHandle::<Zetta>::new(window_id)
        .update(cx, |zetta, _, _| {
            Ok(zetta.command_pane_labels_for_attention(None))
        })
        .map_err(|error| format!("{error:#}"))?
}

fn list_themes(cx: &mut App) -> Vec<String> {
    if !accepting_control_requests(cx) {
        return Vec::new();
    }
    let mut names = ThemeRegistry::global(cx)
        .list()
        .into_iter()
        .map(|theme| theme.name.to_string())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

fn pane_theme(
    attention_id: u64,
    pane_id: Option<u64>,
    cx: &mut App,
) -> std::result::Result<String, String> {
    if !accepting_control_requests(cx) {
        return Err("Zetta is shutting down".to_owned());
    }
    process_zetta_entities(cx)
        .into_iter()
        .find_map(|zetta| {
            zetta
                .read(cx)
                .pane_theme_by_attention_id(attention_id, pane_id, cx)
        })
        .ok_or_else(|| "the originating Zetta pane is no longer available".to_owned())
}

fn set_tab_attention(request: &TabAttentionRequest, cx: &mut App) -> bool {
    if !accepting_control_requests(cx) {
        return false;
    }
    let process = cx.global::<ZettaProcessState>();
    let windows = process.windows.keys().copied().collect::<Vec<_>>();
    let dormant = process.dormant.clone();
    // Every entity is offered the request rather than stopping at the first
    // match; the accumulated result reports whether any tab took it.
    let mut accepted = false;
    for window_id in windows {
        let found = gpui::WindowHandle::<Zetta>::new(window_id)
            .update(cx, |zetta, window, cx| {
                let found = zetta.set_tab_attention(request.clone(), Some(window), cx);
                if found && !window.is_window_active() {
                    window.request_attention();
                }
                found
            })
            .unwrap_or(false);
        accepted |= found;
    }
    for zetta in dormant {
        accepted |= zetta.update(cx, |zetta, cx| {
            zetta.set_tab_attention(request.clone(), None, cx)
        });
    }
    accepted
}

fn silent_mode(attention_id: Option<u64>, cx: &mut App) -> bool {
    if !accepting_control_requests(cx) {
        return false;
    }
    let global_silent_mode = cx.global::<ZettaProcessState>().silent_mode.effective();
    let tab_silent_mode = attention_id.is_some_and(|attention_id| {
        process_zetta_entities(cx).into_iter().any(|zetta| {
            zetta
                .read(cx)
                .tab_silent_mode_by_attention_id(attention_id)
                .unwrap_or(false)
        })
    });
    crate::silent_mode::combined_silent_mode(global_silent_mode, tab_silent_mode)
}

fn reconnect_session(
    runner_id: u64,
    session_id: u64,
    attention_id: Option<u64>,
    secret: Option<SessionSecret>,
    completion: Sender<ReconnectSessionResult>,
    cx: &mut AsyncApp,
) {
    let mut completion = Some(completion);
    let dispatched = cx.update(|cx| {
        if !accepting_control_requests(cx) {
            return false;
        }
        let window_id = if attention_id.is_some() {
            // A request carrying an originating tab must never fall
            // back to another window: that would make a shared-session
            // reconnect look successful while moving it elsewhere.
            reconnect_window_id(runner_id, attention_id, cx)
        } else {
            if cx.global::<ZettaProcessState>().windows.is_empty()
                && open_dormant_or_new_window(cx).is_err()
            {
                return false;
            }
            reconnect_window_id(runner_id, None, cx).or_else(|| {
                cx.global::<ZettaProcessState>()
                    .windows
                    .keys()
                    .next()
                    .copied()
            })
        };
        let Some(window_id) = window_id else {
            return false;
        };
        gpui::WindowHandle::<Zetta>::new(window_id)
            .update(cx, |zetta, window, cx| {
                if attention_id.is_some() {
                    window.activate_window();
                }
                zetta.reconnect_session_from_cli(
                    runner_id,
                    session_id,
                    secret,
                    completion.take().expect("completion sender"),
                    window,
                    cx,
                );
            })
            .is_ok()
    });
    if !dispatched && let Some(completion) = completion {
        let _ = completion.send(ReconnectSessionResult::Rejected);
    }
}

fn open_remote_session(
    target: String,
    port: Option<u16>,
    session_id: u64,
    secret: Option<SessionSecret>,
    completion: Sender<ReconnectSessionResult>,
    cx: &mut AsyncApp,
) {
    let mut completion = Some(completion);
    let dispatched = cx.update(|cx| {
        with_any_window(cx, |zetta, window, cx| {
            zetta.open_remote_session_from_cli(
                target,
                port,
                session_id,
                secret,
                completion.take().expect("completion sender"),
                window,
                cx,
            );
        })
        .is_some()
    });
    if !dispatched && let Some(completion) = completion {
        let _ = completion.send(ReconnectSessionResult::Rejected);
    }
}

fn resume_disk_session(
    session_id: u64,
    identity_paths: Vec<PathBuf>,
    identity_passphrases: Vec<Option<SessionSecret>>,
    secret: Option<SessionSecret>,
    completion: Sender<ReconnectSessionResult>,
    cx: &mut AsyncApp,
) {
    let mut completion = Some(completion);
    let dispatched = cx.update(|cx| {
        with_any_window(cx, |zetta, window, cx| {
            zetta.resume_disk_session_from_cli(
                session_id,
                secret,
                crate::background_session_ui::DiskResumeIdentities {
                    paths: identity_paths,
                    passphrases: identity_passphrases,
                },
                completion.take().expect("completion sender"),
                window,
                cx,
            );
        })
        .is_some()
    });
    if !dispatched && let Some(completion) = completion {
        let _ = completion.send(ReconnectSessionResult::Rejected);
    }
}
