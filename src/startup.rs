use super::*;
#[cfg(cli_services)]
use crate::cli_services::CliServiceCommand;
use crate::cli_services::NotificationTarget;
#[cfg(all(target_os = "macos", feature = "notifications"))]
use crate::cli_services::macos_notification_target_for_response;
#[cfg(feature = "clipboard")]
use crate::cli_services::{copy_help, parse_copy_args, parse_paste_args, paste_help};
#[cfg(feature = "http-server")]
use crate::cli_services::{http_server_help, parse_http_args};
#[cfg(notify_cleanup_enabled)]
use crate::cli_services::{notify_cleanup_help, parse_notify_cleanup_args};
#[cfg(feature = "notifications")]
use crate::cli_services::{notify_help, parse_notify_args, run_notification};
#[cfg(feature = "serial-console")]
use crate::cli_services::{parse_serial_args, serial_help};
#[cfg(feature = "tftp-server")]
use crate::cli_services::{parse_tftp_server_args, tftp_server_help};
use crate::process_control::{
    ReplacePaneRequest, TabAttentionRequest, request_existing_process_command,
    request_existing_process_new_window, request_existing_process_pane,
    request_existing_process_pane_labels, request_existing_process_pane_overlay,
    request_existing_process_project_with_working_directory,
    request_existing_process_projects_reload, request_existing_process_replace_pane,
    request_existing_process_shell_command, request_existing_process_tab_icon,
    request_existing_process_theme, request_existing_process_theme_list, request_process_run_wait,
    request_process_tab_attention,
};
use crate::project_commands::{ProjectCommandInvocation, merge_command_environment};
use crate::run_command::{PaneWaitCommand, RunWaitRequest, process_run_registry};
#[cfg(feature = "worktree")]
use zwt::{WorktreeInvocation, run_for as run_worktree};

use gpui::{KeyBindingContextPredicate, Unbind};
#[cfg(target_os = "macos")]
use gpui::{Menu, MenuItem};
#[cfg(target_os = "macos")]
use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventModifierFlags};
use serde_json::Value;
use std::rc::Rc;

mod arg_parsing;
mod cli_help;
mod cli_modes;
mod keybindings;
mod process_control_loop;
mod workload;
mod wsl;

use arg_parsing::{
    StartupArgs, configured_split_names, parse_attention_target, select_launch_profile,
    should_handoff_to_existing_process, should_replace_pane_in_existing_process,
    validate_launch_split,
};
pub(crate) use arg_parsing::{
    StartupMode, load_startup_config, native_terminal_environment, parse_args,
};
#[cfg(not(feature = "tftp-client"))]
pub(crate) use cli_help::{TftpCommand, parse_tftp_args, tftp_help};
pub(crate) use cli_help::{command_help, format_help_table};
pub(crate) use keybindings::{
    PROFILE_SHORTCUT_KEYS, keymap_keystroke_display, keymap_keystroke_storage, load_keybindings,
    profile_keybindings, profile_shortcut_label,
};
#[cfg(test)]
pub(crate) use keybindings::{RENAME_TAB_KEYBINDING, keymap_keystroke_alias};
#[cfg(target_os = "macos")]
pub(crate) use keybindings::{
    install_native_macos_menus, update_native_macos_dock_menu, update_native_macos_menus,
};
#[cfg(windows)]
pub(crate) use wsl::Msys2Shell;
use wsl::paths_for_external_editor;
pub(crate) use wsl::{
    add_wsl_environment_variable_names, add_wsl_environment_variables, cygwin_path_to_windows,
    cygwin_profile, is_wsl_shell, launch_working_directory, msys2_cwd_tracking_environment,
    msys2_path_to_windows, msys2_profile, wsl_cwd_tracking_file, wsl_shell_with_tracking,
    wsl_terminal_environment,
};
#[cfg(windows)]
pub(crate) use wsl::{
    cygwin_cwd_tracking_environment_with_path, cygwin_shell_with_tracking,
    ensure_cygwin_environment,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ThemeFileStamp {
    pub(crate) modified: Option<SystemTime>,
    pub(crate) len: u64,
}

pub(crate) fn changed_theme_files(
    themes_dir: &Path,
    cache: &mut HashMap<PathBuf, ThemeFileStamp>,
) -> Result<Vec<PathBuf>> {
    let mut changed = Vec::new();
    let mut present = std::collections::HashSet::new();
    for entry in fs::read_dir(themes_dir)
        .with_context(|| format!("reading theme directory {}", themes_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let metadata = entry.metadata()?;
        let stamp = ThemeFileStamp {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        };
        present.insert(path.clone());
        if cache.get(&path) != Some(&stamp) {
            cache.insert(path.clone(), stamp);
            changed.push(path);
        }
    }
    cache.retain(|path, _| present.contains(path));
    Ok(changed)
}

pub(crate) fn load_user_themes(cx: &mut App) -> Result<()> {
    static THEME_FILE_CACHE: OnceLock<Mutex<HashMap<PathBuf, ThemeFileStamp>>> = OnceLock::new();
    let themes_dir = config::themes_dir();
    fs::create_dir_all(&themes_dir)
        .with_context(|| format!("creating theme directory {}", themes_dir.display()))?;
    let registry = ThemeRegistry::global(cx);
    let paths = changed_theme_files(
        &themes_dir,
        &mut THEME_FILE_CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )?;
    for path in paths {
        let bytes = fs::read(&path).with_context(|| format!("reading theme {}", path.display()))?;
        theme_settings::load_user_theme(&registry, &bytes)
            .with_context(|| format!("loading theme {}", path.display()))?;
    }
    Ok(())
}

/// Zetta's scrollbar colors, which every theme it ships or installs gets.
///
/// Idempotent: each field derives from a `text*` color rather than from itself,
/// so re-running it over an already-overridden theme is a no-op. That is what
/// lets [`bake_zetta_theme_overrides`] re-sweep the whole registry after a
/// reload without having to track which themes it already visited.
pub(crate) fn apply_zetta_theme_overrides(theme: &mut Theme) {
    let colors = &mut theme.styles.colors;
    colors.scrollbar_thumb_background = colors.text_muted.opacity(0.7);
    colors.scrollbar_thumb_hover_background = colors.text.opacity(0.85);
    colors.scrollbar_thumb_active_background = colors.text_accent.opacity(0.95);
}

/// Rewrites every registered theme with [`apply_zetta_theme_overrides`] applied.
///
/// The overrides used to be applied at each lookup instead, which cloned a whole
/// `Theme` every time. `window_theme`/`theme_for_tab` resolve a theme per tab per
/// frame, so that put one full theme clone per tab into every frame. Baking the
/// overrides into the registry reduces a lookup to a lock read and an `Arc` clone.
///
/// Call this after anything that can add themes to the registry; `apply_config_settings`
/// already does, and every reload path goes through it.
pub(crate) fn bake_zetta_theme_overrides(registry: &ThemeRegistry) {
    let overridden = registry
        .list_names()
        .into_iter()
        .filter_map(|name| registry.get(&name).ok())
        .map(|theme| {
            let mut theme = theme.as_ref().clone();
            apply_zetta_theme_overrides(&mut theme);
            theme
        })
        .collect::<Vec<_>>();
    registry.insert_themes(overridden);
}

pub(crate) fn resolve_profile_theme(profile: &Profile, cx: &App) -> Result<Option<Arc<Theme>>> {
    let configured_theme = if SystemAppearance::global(cx).is_light() {
        profile.theme.as_deref()
    } else {
        profile.dark_theme.as_deref()
    };
    configured_theme
        .map(|name| {
            ThemeRegistry::global(cx)
                .get(name)
                .with_context(|| format!("using theme {name:?} for profile {:?}", profile.name))
        })
        .transpose()
}

pub(crate) fn apply_config_settings(config: &Config, cx: &mut App) -> Result<()> {
    let registry = ThemeRegistry::global(cx);
    bake_zetta_theme_overrides(&registry);
    let theme_name = selected_theme_name_for_appearance(config, cx);
    let theme = registry
        .get(theme_name)
        .with_context(|| format!("using Zed theme {theme_name:?}"))?;
    GlobalTheme::update_theme(cx, theme);

    let mut terminal_settings = TerminalSettings::get_global(cx).clone();
    terminal_settings.font_family = Some(theme_settings::FontFamilyName(
        config.terminal_font_family.clone().into(),
    ));
    terminal_settings.font_size = config.terminal_font_size.map(px);
    terminal_settings.copy_on_select = true;
    terminal_settings.max_scroll_history_lines = Some(config.max_scroll_history_lines);
    TerminalSettings::override_global(terminal_settings, cx);
    Ok(())
}

pub(crate) fn selected_theme_name(configured_theme: Option<&str>) -> &str {
    configured_theme.unwrap_or(ZETTA_DEFAULT_THEME)
}

pub(crate) fn selected_dark_theme_name(configured_theme: Option<&str>) -> &str {
    configured_theme.unwrap_or(ZETTA_DEFAULT_DARK_THEME)
}

pub(crate) fn selected_theme_name_for_appearance<'a>(config: &'a Config, cx: &App) -> &'a str {
    if SystemAppearance::global(cx).is_light() {
        selected_theme_name(config.theme.as_deref())
    } else {
        selected_dark_theme_name(config.dark_theme.as_deref())
    }
}

pub(crate) fn normalize_keymap_key_names(content: &str) -> String {
    let content = content
        .replace("page-up", "pageup")
        .replace("page-down", "pagedown");
    let Ok(mut root) = serde_json::from_str::<Value>(&content) else {
        return content;
    };
    let Some(sections) = root.as_array_mut() else {
        return content;
    };

    let mut changed = false;
    for section in sections {
        let Some(bindings) = section.get_mut("bindings").and_then(Value::as_object_mut) else {
            continue;
        };
        let entries = std::mem::take(bindings);
        for (keystroke, action) in entries {
            let normalized = keymap_keystroke_storage(&keystroke);
            changed |= normalized != keystroke;
            bindings.insert(normalized, action);
        }
    }

    if changed {
        serde_json::to_string(&root).unwrap_or(content)
    } else {
        content
    }
}

pub(crate) fn validate_keymap_contents(content: &str, cx: &mut App) -> Result<()> {
    let content = normalize_keymap_key_names(content);
    match KeymapFile::load(&content, cx) {
        KeymapFileLoadResult::Success { .. } => Ok(()),
        KeymapFileLoadResult::SomeFailedToLoad { error_message, .. } => {
            anyhow::bail!("some key bindings are invalid: {error_message}")
        }
        KeymapFileLoadResult::JsonParseFailure { error } => {
            Err(error).context("parsing keymap JSON")
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn open_zetta_window(
    config: Config,
    configuration_error: Option<String>,
    initial_profile: Option<Profile>,
    initial_project: Option<ProjectConfig>,
    launch_theme_override: Option<(String, String)>,
    launch_split: Option<String>,
    enable_performance_overlay: bool,
    performance_report: Option<(PerformanceReportOptions, PerformanceReportStatus)>,
    profile_pane_stress: bool,
    no_mux: bool,
    initial_command: Option<Vec<String>>,
    initial_working_directory: Option<PathBuf>,
    initial_launch: Option<crate::app::TerminalLaunch>,
    activation_token: Option<String>,
    cx: &mut App,
) -> Result<()> {
    let options = zetta_window_options(cx);
    let window_handle = cx
        .open_window(options, move |window, cx| {
            window.set_window_title("Zetta");
            let zetta = cx.new(|cx| {
                Zetta::new(
                    config,
                    configuration_error,
                    ZettaLaunchOptions {
                        initial_profile,
                        initial_project,
                        launch_theme_override,
                        no_mux,
                        initial_command,
                        initial_working_directory,
                        initial_launch,
                    },
                    window,
                    cx,
                )
            });
            track_zetta_window(&zetta, window, cx);
            prepare_background_tabs_before_window_close(&zetta, window, cx);
            if let Some(name) = launch_split {
                zetta.update(cx, |zetta, cx| {
                    zetta.apply_pane_split_template(&ApplyPaneSplitTemplate { name }, window, cx)
                });
            }
            if profile_pane_stress {
                zetta.update(cx, |zetta, cx| {
                    zetta.configure_pane_profile_stress(window, cx)
                });
            }
            if enable_performance_overlay {
                zetta.update(cx, |zetta, cx| {
                    zetta.toggle_performance_overlay(&TogglePerformanceOverlay, window, cx)
                });
            }
            if let Some((options, status)) = performance_report {
                zetta.update(cx, |zetta, cx| {
                    zetta.start_performance_report(options, status, cx)
                });
            }
            zetta
        })
        .context("opening Zetta window")?;
    if let Some(activation_token) = activation_token {
        window_handle.update(cx, |_, window, _| {
            gpui_platform::activate_window_with_token(window, &activation_token)
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
                zetta.prepare_for_background_window_close(cx)
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

/// How often the configuration file is checked for changes made outside the
/// settings UI. The check is metadata-only while the file is unchanged, so it
/// does not add work to rendering or input handling.
const CONFIGURATION_FILE_POLL: Duration = Duration::from_secs(1);

fn config_file_stamp(path: &Path) -> ConfigFileStamp {
    let Ok(metadata) = fs::metadata(path) else {
        return ConfigFileStamp {
            modified: None,
            len: 0,
        };
    };
    ConfigFileStamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    }
}

fn reload_process_configuration(cx: &mut App) -> Result<()> {
    let (config_path, keymap_override) = {
        let process = cx.global::<ZettaProcessState>();
        (
            process.config.config_path.clone(),
            process.config.keymap_override.clone(),
        )
    };
    let config_stamp = config_file_stamp(&config_path);
    let config = Config::load(Some(&config_path), keymap_override)?;
    let entities = process_zetta_entities(cx);
    let has_entities = !entities.is_empty();
    for entity in entities {
        entity
            .update(cx, |zetta, cx| {
                zetta.reload_configuration_from_process(config.clone(), cx)
            })
            .with_context(|| {
                format!("applying reloaded configuration {}", config_path.display())
            })?;
    }
    // A process can receive a request before its first window has been
    // attached. Keep launcher integrations correct in that small window too;
    // normal entities update them as part of their reload path.
    if !has_entities {
        #[cfg(windows)]
        windows_integration::update_profile_jump_list(
            config.profiles.clone(),
            config.hidden_profiles.clone(),
        );
        #[cfg(target_os = "linux")]
        if linux_desktop::update_profile_actions(&config.profiles, &config.hidden_profiles)
            .log_err()
            .unwrap_or(false)
        {
            schedule_linux_desktop_window_reassociation(cx);
        }
        #[cfg(target_os = "macos")]
        update_native_macos_dock_menu(cx, &config.profiles, &config.hidden_profiles);
    }
    let process = cx.global_mut::<ZettaProcessState>();
    process.config = config;
    process.config_file_stamp = config_stamp;
    process.configuration_error = None;
    Ok(())
}

fn reload_process_configuration_if_changed(cx: &mut App) -> Result<bool> {
    let (config_path, last_stamp) = {
        let process = cx.global::<ZettaProcessState>();
        (
            process.config.config_path.clone(),
            process.config_file_stamp,
        )
    };
    if config_file_stamp(&config_path) == last_stamp {
        return Ok(false);
    }
    reload_process_configuration(cx)?;
    Ok(true)
}

/// Keeps every open window, native launcher, and the process-wide launch
/// configuration in sync with edits made directly to config.json. Profile
/// lists are read during this idle watcher rather than during rendering.
fn start_configuration_watcher(cx: &mut App) {
    let (config_path, mut last_seen) = {
        let process = cx.global::<ZettaProcessState>();
        (
            process.config.config_path.clone(),
            process.config_file_stamp,
        )
    };
    #[cfg(target_os = "linux")]
    let mut desktop_entry_stamp = linux_desktop::desktop_entry_stamp();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(CONFIGURATION_FILE_POLL)
                .await;
            let changed = config_file_stamp(&config_path);
            if changed != last_seen {
                last_seen = changed;
                if let Err(error) = cx.update(reload_process_configuration) {
                    eprintln!(
                        "Could not reload {} after it changed: {error:#}",
                        config_path.display()
                    );
                }
                #[cfg(target_os = "linux")]
                {
                    // A configuration reload can update the desktop entry
                    // itself. Absorb that write so the desktop poll below
                    // does not schedule a second repair for the same change.
                    desktop_entry_stamp = linux_desktop::desktop_entry_stamp();
                }
            }

            #[cfg(target_os = "linux")]
            {
                let current_stamp = linux_desktop::desktop_entry_stamp();
                if current_stamp != desktop_entry_stamp {
                    desktop_entry_stamp = current_stamp;
                    // An installer may atomically replace the entry with
                    // byte-for-byte identical content. That still causes
                    // GNOME Shell to refresh its app cache, so repair any
                    // managed entry replacement rather than relying on a
                    // content diff.
                    if linux_desktop::is_managed_user_desktop_entry() {
                        cx.update(schedule_linux_desktop_window_reassociation);
                    }
                }
            }
        }
    })
    .detach();
}

/// How often the multiplexer's published catalog is checked for changes.
const MULTIPLEXER_CATALOG_POLL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SessionCatalogFileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SessionCatalogStamp {
    catalog: Option<SessionCatalogFileStamp>,
    persistence_manifest: Option<SessionCatalogFileStamp>,
}

fn session_catalog_file_stamp(path: &Path) -> Option<SessionCatalogFileStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(SessionCatalogFileStamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

fn session_catalog_stamp(directory: &Path) -> SessionCatalogStamp {
    SessionCatalogStamp {
        catalog: session_catalog_file_stamp(directory),
        persistence_manifest: session_catalog_file_stamp(
            &directory.join("persistence").join("manifest.json"),
        ),
    }
}

/// Notices sessions the multiplexer is holding.
///
/// The reconnect list used to be refreshed only when *this* process published
/// its own catalog. Once the multiplexer owns the sessions that stopped
/// happening, so a window that had not detached anything itself never learned
/// that anything was there — no reconnect button, and the action finding
/// nothing to offer.
///
/// The catalog is a file the multiplexer replaces atomically, so this watches
/// the directory's modification time and the persistence manifest's
/// modification time, and only re-reads when either changes. The manifest is
/// nested below the catalog directory, so watching the directory alone misses
/// a disk record being consumed by `resume`. That keeps an idle process from
/// parsing the catalog and scanning the process table once a second for no
/// reason while still invalidating both live-session and disk-session entries.
fn start_multiplexer_session_watcher(cx: &mut App) {
    let directory = crate::background_sessions::session_catalog_dir();
    let mut last_seen: Option<SessionCatalogStamp> = None;
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(MULTIPLEXER_CATALOG_POLL)
                .await;
            let changed = session_catalog_stamp(&directory);
            // A first look always refreshes: the catalog may already describe
            // sessions from before this process started.
            if last_seen.is_some_and(|last_seen| changed == last_seen) {
                continue;
            }
            last_seen = Some(changed);
            cx.update(refresh_process_background_sessions);
        }
    })
    .detach();
}

pub(crate) fn refresh_process_background_sessions(cx: &mut App) {
    let entities = process_zetta_entities(cx);
    let mut entries = Vec::new();
    for zetta in &entities {
        let zetta = zetta.read(cx);
        let runner_id = zetta.background_sessions.runner_id();
        entries.extend(zetta.background_session_picker_entries.iter().map(
            |(session_id, title, details)| (runner_id, *session_id, title.clone(), details.clone()),
        ));
    }
    let no_mux = cx.has_global::<ZettaProcessState>() && cx.global::<ZettaProcessState>().no_mux;
    if !no_mux {
        entries.extend(multiplexer_session_entries());
    }
    if cx.has_global::<ZettaProcessState>() {
        cx.global_mut::<ZettaProcessState>()
            .background_session_entries = entries.into();
    }
    for zetta in entities {
        zetta.update(cx, |_, cx| cx.notify());
    }
}

pub(crate) fn prune_empty_dormant_runners(cx: &mut App) {
    if !cx.has_global::<ZettaProcessState>() {
        return;
    }
    let dormant = std::mem::take(&mut cx.global_mut::<ZettaProcessState>().dormant);
    let mut retained = Vec::with_capacity(dormant.len());
    let mut removed_runner_ids = Vec::new();
    for zetta in dormant {
        let (is_empty, runner_id) = {
            let state = zetta.read(cx);
            (
                state.background_sessions.is_empty(),
                state.background_sessions.runner_id(),
            )
        };
        if is_empty {
            removed_runner_ids.push(runner_id);
        } else {
            retained.push(zetta);
        }
    }
    let process = cx.global_mut::<ZettaProcessState>();
    process.dormant = retained;
    for runner_id in removed_runner_ids {
        process.runners.remove(&runner_id);
    }
    if should_quit_after_window_closed(process.windows.len(), process.dormant.len()) {
        quit_zetta_process(cx);
    }
}

fn should_quit_after_window_closed(window_count: usize, dormant_runner_count: usize) -> bool {
    window_count == 0 && dormant_runner_count == 0
}

fn zetta_quit_mode() -> gpui::QuitMode {
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
fn shutdown_multiplexer_if_idle(cx: &App) {
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
            zetta.resume_hidden_window(window, cx)
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
            None,
            None,
            None,
            None,
            false,
            None,
            false,
            no_mux,
            None,
            None,
            None,
            None,
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

fn open_fresh_zetta_window_with_profile_and_activation_token(
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
        initial_profile,
        None,
        None,
        None,
        false,
        None,
        false,
        no_mux,
        None,
        None,
        None,
        activation_token,
        cx,
    )
}

#[cfg(windows)]
fn open_windows_handoff_window(
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
        None,
        None,
        None,
        None,
        false,
        None,
        false,
        no_mux,
        None,
        None,
        Some(crate::app::TerminalLaunch::Handoff(request)),
        None,
        cx,
    )
}

fn handle_zetta_window_closed(cx: &mut App, window_id: WindowId) {
    let entity = cx
        .global_mut::<ZettaProcessState>()
        .windows
        .remove(&window_id);
    if let Some(entity) = entity {
        entity.update(cx, |zetta, cx| {
            zetta.prepare_for_background_window_close(cx)
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

fn terminal_rendering_profile_config(executable: &Path, workload: PerformanceWorkload) -> Config {
    let mut config = Config::defaults(None, None);
    let workload_argument = match workload {
        PerformanceWorkload::Standard => "--terminal-render-workload",
        PerformanceWorkload::CheckerboardBackground => "--terminal-checkerboard-workload",
        PerformanceWorkload::SparseUpdates => "--terminal-sparse-update-workload",
        PerformanceWorkload::AltScreenScroll => "--terminal-alt-screen-scroll-workload",
    };
    config.profiles = vec![Profile {
        name: "Terminal rendering profiler".to_owned(),
        command: Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec!["benchmark".to_owned(), workload_argument.to_owned()],
            title_override: Some("Terminal rendering profiler".to_owned()),
        },
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    }];
    config.default_profile = 0;
    config
}

fn focus_visible_tab_by_attention_id(cx: &mut App, attention_id: u64) -> bool {
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

fn reconnect_window_id(runner_id: u64, attention_id: Option<u64>, cx: &App) -> Option<WindowId> {
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

trait ApplicationOpenUrlsExt {
    fn with_open_url_handler(
        self,
        control_tx: futures::channel::mpsc::UnboundedSender<ProcessControlCommand>,
    ) -> Self;
}

impl ApplicationOpenUrlsExt for gpui::Application {
    fn with_open_url_handler(
        self,
        control_tx: futures::channel::mpsc::UnboundedSender<ProcessControlCommand>,
    ) -> Self {
        #[cfg(target_os = "macos")]
        self.on_open_urls(move |urls| {
            let _ = control_tx.unbounded_send(ProcessControlCommand::OpenUrls(urls));
        });
        #[cfg(not(target_os = "macos"))]
        let _ = control_tx;
        self
    }
}

pub(crate) fn run() -> Result<()> {
    let args = parse_args()?;
    // `zetta tftp get|put` parses as `StartupMode::Application` carrying a
    // client command rather than as a mode of its own, so it is dispatched
    // ahead of the mode match. It used to be dispatched after the
    // handoff-to-a-running-process checks below, where an Application-mode
    // launch beside a running Zetta was satisfied by raising that process's
    // window and the transfer never ran.
    if let Some(command) = &args.tftp_command {
        return command.run();
    }
    match dispatch_startup_mode(&args) {
        Some(result) => result,
        None => run_application(args),
    }
}

/// Runs the subcommand `args.mode` names, or returns `None` for the modes that
/// end in a window this process owns.
///
/// The match is exhaustive on purpose. This replaced a sequence of 36
/// `if let StartupMode::X(..)` and `args.mode == StartupMode::X` tests, in
/// which a new variant matched none of them and silently fell through to a GUI
/// launch; now it has to name the function that handles it, or say explicitly
/// that it launches a window.
fn dispatch_startup_mode(args: &StartupArgs) -> Option<Result<()>> {
    Some(match &args.mode {
        StartupMode::Application
        | StartupMode::NewWindow
        | StartupMode::Command(_)
        | StartupMode::Project(crate::project_cli::ProjectCommand::Open { .. })
        | StartupMode::TerminalRenderingProfile => return None,
        #[cfg(windows)]
        StartupMode::WindowsEmbedding => return None,
        StartupMode::PaneWait(command) => cli_modes::run_wait_command(command.clone()),
        StartupMode::ProjectCommand(invocation) => {
            cli_modes::run_registered_project_command(invocation)
        }
        StartupMode::Project(command) => cli_modes::run_project_registry_command(command),
        StartupMode::Edit {
            arguments,
            delete_after,
        } => cli_modes::run_editor(arguments, *delete_after),
        StartupMode::Vi(arguments) => cli_modes::run_vi(arguments.clone()),
        StartupMode::OutputBenchmark {
            size_mib,
            output_type,
        } => run_output_benchmark(*size_mib, *output_type),
        StartupMode::Pane(request) => cli_modes::run_pane_command(request),
        StartupMode::Attention(command) => cli_modes::run_attention_command(command),
        #[cfg(feature = "worktree")]
        StartupMode::Worktree(command) => run_worktree(command, WorktreeInvocation::Zetta),
        StartupMode::Profile(command) => {
            cli_modes::run_profile_command(command.clone(), args.config_path.as_deref())
        }
        StartupMode::PrintTerminalSize { json, resize } => {
            cli_modes::run_terminal_size_command(*json, *resize)
        }
        StartupMode::PrintShellIntegration(shell) => {
            print!("{}", shell.script());
            Ok(())
        }
        StartupMode::ConfigureCurrentShellIntegration => cli_modes::configure_shell_integration(),
        StartupMode::ListTabIcons => cli_modes::list_tab_icons(),
        StartupMode::SetTabIcon { icon } => cli_modes::set_tab_icon(*icon),
        StartupMode::SetTheme { scope, theme } => cli_modes::set_theme(*scope, theme.clone()),
        StartupMode::ListThemes => cli_modes::list_themes(),
        StartupMode::ListPaneSplits => cli_modes::list_pane_splits(),
        StartupMode::SetPaneOverlay(request) => cli_modes::set_pane_overlay(request.clone()),
        StartupMode::Mux(arguments) => {
            cli_modes::run_mux_command(arguments, args.config_path.clone())
        }
        #[cfg(cli_services)]
        StartupMode::CliService(command) => command.run(),
        #[cfg(windows)]
        StartupMode::RegisterWindowsShell(shortcut_path) => cli_modes::register_windows_shell(
            shortcut_path,
            args.config_path.as_deref(),
            args.keymap_path.clone(),
        ),
        #[cfg(windows)]
        StartupMode::UnregisterWindowsShell => windows_integration::unregister_shell_integration(),
        StartupMode::TerminalRenderingWorkload => {
            workload::run_terminal_rendering_workload(PerformanceWorkload::Standard, None)
        }
        StartupMode::TerminalCheckerboardWorkload => workload::run_terminal_rendering_workload(
            PerformanceWorkload::CheckerboardBackground,
            None,
        ),
        StartupMode::TerminalSparseUpdateWorkload => {
            workload::run_terminal_rendering_workload(PerformanceWorkload::SparseUpdates, None)
        }
        StartupMode::TerminalAltScreenScrollWorkload => {
            workload::run_terminal_rendering_workload(PerformanceWorkload::AltScreenScroll, None)
        }
    })
}

/// The project a GUI launch opens: named by `zetta project open`, or detected
/// from the directory a plain launch was started in.
#[derive(Debug, Default)]
struct StartupProject {
    root: Option<PathBuf>,
    working_directory: Option<PathBuf>,
}

/// A launch that ends in a window: the handoffs a running process can take,
/// then the GUI.
fn run_application(args: StartupArgs) -> Result<()> {
    let activation_token = (args.mode == StartupMode::NewWindow)
        .then(|| env::var("XDG_ACTIVATION_TOKEN").ok())
        .flatten();
    if args.mode == StartupMode::NewWindow
        && should_handoff_to_existing_process(&args)
        && request_existing_process_new_window(
            args.profile.as_deref(),
            activation_token.as_deref(),
        )?
    {
        return Ok(());
    }
    let Some(project) = resolve_startup_project(&args)? else {
        return Ok(());
    };
    if args.mode == StartupMode::Application
        || matches!(
            args.mode,
            StartupMode::Project(crate::project_cli::ProjectCommand::Open { .. })
        )
    {
        terminal_view::start_scrollback_cleanup_monitor();
    }
    if handed_off_to_existing_process(&args, project.root.as_deref())? {
        return Ok(());
    }
    launch_gui(args, project, activation_token)
}

/// Resolves the project a GUI launch opens, offering it to a running Zetta
/// process first.
///
/// `None` means a running process took the launch and this one has nothing
/// left to do. This runs before [`handed_off_to_existing_process`] because a
/// resolved project makes the plain window handoff there the wrong answer:
/// raising a window would drop the project, so that handoff is skipped once a
/// project resolves.
fn resolve_startup_project(args: &StartupArgs) -> Result<Option<StartupProject>> {
    if let StartupMode::Project(crate::project_cli::ProjectCommand::Open { path }) = &args.mode {
        let target = crate::project_cli::resolve_open_target(path.as_deref())?;
        if request_existing_process_project_with_working_directory(
            &target.root,
            target.working_directory.as_deref(),
        )? {
            return Ok(None);
        }
        return Ok(Some(StartupProject {
            root: Some(target.root),
            working_directory: target.working_directory,
        }));
    }
    // A plain launch adopts the project of the directory it started in, but
    // not when it carries configuration of its own or is replacing a pane in
    // another window.
    if args.mode != StartupMode::Application
        || args.config_path.is_some()
        || args.keymap_path.is_some()
        || args.replace_pane
    {
        return Ok(Some(StartupProject::default()));
    }
    let mut project = StartupProject::default();
    match crate::project_cli::current_project_target() {
        Ok(Some(target)) => {
            project.working_directory = target.working_directory;
            project.root = Some(target.root);
        }
        Ok(None) => {}
        Err(error) => eprintln!("Could not load the Zetta project registry: {error:#}"),
    }
    if let Some(root) = project.root.as_ref()
        && should_handoff_to_existing_process(args)
        && request_existing_process_project_with_working_directory(
            root,
            project.working_directory.as_deref(),
        )?
    {
        return Ok(None);
    }
    Ok(Some(project))
}

/// Offers this launch to a running Zetta process, returning whether one took
/// it.
///
/// The three are disjoint rather than ordered by precedence:
/// `should_replace_pane_in_existing_process` requires `--replace-pane`, the
/// command handoff requires [`StartupMode::Command`], and the window handoff
/// requires [`StartupMode::Application`] *without* `--replace-pane` — which
/// `should_handoff_to_existing_process` also demands. So at most one of them
/// can apply to any launch.
fn handed_off_to_existing_process(args: &StartupArgs, project_root: Option<&Path>) -> Result<bool> {
    if should_replace_pane_in_existing_process(args) {
        let request = ReplacePaneRequest {
            split: args.split.clone(),
            profile: args.profile.clone(),
            theme: args.theme_override.clone(),
        };
        if request_existing_process_replace_pane(request)? {
            return Ok(true);
        }
    }
    if should_handoff_to_existing_process(args)
        && let StartupMode::Command(command) = &args.mode
    {
        let request = crate::command_panes::PaneCommand {
            direction: None,
            label: None,
            pane: None,
            overlay: None,
            stack: false,
            list: false,
            command: command.clone(),
        };
        if request_existing_process_command(request, Some(env::current_dir()?))? {
            return Ok(true);
        }
    }
    if project_root.is_none()
        && args.mode == StartupMode::Application
        && should_handoff_to_existing_process(args)
        && request_existing_process_window()?
    {
        return Ok(true);
    }
    Ok(false)
}

/// Everything a GUI launch resolves before the application loop starts.
///
/// Built by [`resolve_application_launch`] and consumed inside the application
/// closure, which keeps that closure a list of initialization steps rather
/// than a closure over two dozen locals.
struct ApplicationLaunch {
    config: Config,
    configuration_error: Option<String>,
    keymap_path: PathBuf,
    profile_count: usize,
    no_mux: bool,
    http_client: Arc<reqwest_client::ReqwestClient>,
    /// The terminal-rendering profiler, which supplies its own configuration
    /// and opens its window with the performance overlay on.
    profiling: bool,
    performance_report: Option<(PerformanceReportOptions, PerformanceReportStatus)>,
    /// Windows only: this process embeds a handed-off console, so it opens no
    /// window until the handoff arrives over the control channel.
    embedding: bool,
    /// `zetta --new-window` that no running process took. It opens a window
    /// from process configuration and resolves its own profile, so the
    /// resolved project, profile, command, split, and theme override below do
    /// not apply to it.
    fresh_window: bool,
    fresh_window_profile: Option<String>,
    activation_token: Option<String>,
    initial_profile: Option<Profile>,
    initial_project: Option<ProjectConfig>,
    launch_theme_override: Option<(String, String)>,
    launch_split: Option<String>,
    profile_pane_stress: bool,
    initial_command: Option<Vec<String>>,
    initial_working_directory: Option<PathBuf>,
}

fn launch_gui(
    args: StartupArgs,
    project: StartupProject,
    activation_token: Option<String>,
) -> Result<()> {
    let profiling = args.mode == StartupMode::TerminalRenderingProfile;
    if profiling && args.profile_external_terminal {
        return workload::run_terminal_rendering_workload(
            args.profile_workload,
            args.profile_duration,
        );
    }
    let report_status: PerformanceReportStatus = Arc::new(Mutex::new(None));
    let launch = resolve_application_launch(args, project, activation_token, &report_status)?;
    let report_requested = launch.performance_report.is_some();
    start_application(launch);
    // `Zetta::start_performance_report` writes the outcome into `report_status`
    // as the profiling window finishes, so a failed report becomes this
    // process's exit status rather than only a log line.
    if report_requested {
        let result = report_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .context("profiling window closed before the performance report completed")?;
        result.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

fn resolve_application_launch(
    args: StartupArgs,
    project: StartupProject,
    activation_token: Option<String>,
    report_status: &PerformanceReportStatus,
) -> Result<ApplicationLaunch> {
    let profiling = args.mode == StartupMode::TerminalRenderingProfile;
    let workload = args.profile_workload;
    let performance_report =
        args.profile_report
            .zip(args.profile_duration)
            .map(|(path, duration)| {
                (
                    PerformanceReportOptions {
                        path,
                        duration,
                        workload,
                    },
                    report_status.clone(),
                )
            });
    let (config, configuration_error) = if profiling {
        (
            terminal_rendering_profile_config(&env::current_exe()?, workload),
            None,
        )
    } else {
        load_startup_config(args.config_path.as_deref(), args.keymap_path)
    };
    let mut initial_project = project
        .root
        .as_deref()
        .map(|root| ProjectConfig::load(root, &config))
        .transpose()?;
    // An explicit `--split` replaces the project's own initial layout rather
    // than being applied on top of it.
    if args.split.is_some()
        && let Some(project) = initial_project.as_mut()
    {
        project.initial_split = None;
    }
    let effective_launch_config = initial_project
        .as_ref()
        .map(|project| &project.effective)
        .unwrap_or(&config);
    validate_launch_split(effective_launch_config, args.split.as_deref())?;
    let initial_profile = select_launch_profile(effective_launch_config, args.profile.as_deref())?;
    // Keyed by profile name (case-insensitive) rather than baked into
    // `initial_profile.theme`, so every tab opened with this profile for the
    // rest of the process gets the override too, not just the first one.
    // Applied in `Zetta::open_tab_with_profile`; never written back to
    // `config.profiles` or the settings UI.
    let launch_theme_override = initial_profile
        .as_ref()
        .zip(args.theme_override.as_ref())
        .map(|(profile, theme)| (profile.name.to_lowercase(), theme.clone()));
    let initial_command = match &args.mode {
        StartupMode::Command(command) => Some(command.clone()),
        _ => None,
    };
    let initial_working_directory = match project.working_directory {
        Some(directory) => Some(directory),
        None => initial_command
            .as_ref()
            .map(|_| env::current_dir())
            .transpose()?,
    };
    let profile_count = visible_profile_count(
        &effective_launch_config.profiles,
        &effective_launch_config.hidden_profiles,
    );
    #[cfg(windows)]
    let embedding = args.mode == StartupMode::WindowsEmbedding;
    #[cfg(not(windows))]
    let embedding = false;
    Ok(ApplicationLaunch {
        keymap_path: config.keymap_path.clone(),
        config,
        configuration_error,
        profile_count,
        no_mux: args.no_mux,
        http_client: Arc::new(
            reqwest_client::ReqwestClient::user_agent(concat!("Zetta/", env!("CARGO_PKG_VERSION")))
                .context("initializing HTTP client")?,
        ),
        profiling,
        performance_report,
        embedding,
        fresh_window: args.mode == StartupMode::NewWindow,
        fresh_window_profile: args.profile,
        activation_token,
        initial_profile,
        initial_project,
        launch_theme_override,
        launch_split: args.split,
        profile_pane_stress: args.profile_pane_stress,
        initial_command,
        initial_working_directory,
    })
}

/// Runs the GPUI application loop until the last window closes.
fn start_application(launch: ApplicationLaunch) {
    let (control_tx, control_rx) = futures::channel::mpsc::unbounded();
    gpui_platform::application()
        .with_quit_mode(zetta_quit_mode())
        .with_assets(ZettaAssets)
        .with_open_url_handler(control_tx.clone())
        .run(move |cx: &mut App| {
            #[cfg(windows)]
            {
                cx.set_app_identity(ZETTA_APP_ID, "Zetta");
                windows_integration::update_profile_jump_list(
                    launch.config.profiles.clone(),
                    launch.config.hidden_profiles.clone(),
                );
            }
            #[cfg(target_os = "linux")]
            let linux_desktop_entry_managed = {
                linux_desktop::update_profile_actions(
                    &launch.config.profiles,
                    &launch.config.hidden_profiles,
                )
                .log_err();
                // The desktop entry may have been replaced by the installer
                // immediately before this process started. In that case its
                // contents already match the current configuration and the
                // updater reports no change, but GNOME Shell may still be
                // holding the old cached app record.
                linux_desktop::is_managed_user_desktop_entry()
            };
            initialize_zetta_settings(&launch, cx);
            initialize_process_state(&launch, control_tx, cx);
            install_process_observers(&launch, cx);
            cx.spawn(async move |cx| process_control_loop::serve(control_rx, cx).await)
                .detach();
            open_launch_window(launch, cx);
            #[cfg(target_os = "linux")]
            if linux_desktop_entry_managed {
                schedule_linux_desktop_window_reassociation(cx);
            }
        });
}

/// Brings up the settings, theme, font, and keybinding state every window
/// reads from.
fn initialize_zetta_settings(launch: &ApplicationLaunch, cx: &mut App) {
    cx.set_http_client(launch.http_client.clone());
    menu::init();
    zed_actions::init();
    release_channel::init(semver::Version::new(0, 1, 0), cx);
    settings::init(cx);
    theme_settings::init(theme::LoadThemes::All(Box::new(ZettaAssets)), cx);
    load_user_themes(cx).log_err();
    ZettaAssets.load_fonts(cx).log_err();
    apply_config_settings(&launch.config, cx).expect("failed to apply Zetta configuration");
    load_keybindings(&launch.keymap_path, launch.profile_count, launch.no_mux, cx);
    #[cfg(target_os = "macos")]
    install_native_macos_menus(
        cx,
        &launch.config.profiles,
        &launch.config.hidden_profiles,
        launch.config.default_profile,
    );
    #[cfg(target_os = "macos")]
    update_native_macos_dock_menu(cx, &launch.config.profiles, &launch.config.hidden_profiles);
}

/// Starts the control server, publishes [`ZettaProcessState`], and starts the
/// watchers that keep it current.
fn initialize_process_state(
    launch: &ApplicationLaunch,
    control_tx: futures::channel::mpsc::UnboundedSender<ProcessControlCommand>,
    cx: &mut App,
) {
    #[cfg(windows)]
    let handoff_control_tx = control_tx.clone();
    let control_server =
        ProcessControlServer::start(control_tx).expect("failed to start Zetta process control");
    #[cfg(windows)]
    if launch.embedding {
        windows_integration::start_handoff_server(handoff_control_tx)
            .expect("failed to start the Windows terminal handoff server");
    }
    let quit_subscription = cx.on_app_quit(|cx| {
        if cx.has_global::<ZettaProcessState>() {
            cx.global::<ZettaProcessState>()
                .control_server
                .begin_shutdown();
        }
        shutdown_multiplexer_if_idle(cx);
        async {}
    });
    cx.set_global(ZettaProcessState {
        windows: HashMap::new(),
        dormant: Vec::new(),
        runners: HashMap::new(),
        next_attention_id: 1,
        silent_mode: SilentModeState::default(),
        background_session_entries: Arc::from([]),
        config: launch.config.clone(),
        config_file_stamp: config_file_stamp(&launch.config.config_path),
        configuration_error: launch.configuration_error.clone(),
        no_mux: launch.no_mux,
        #[cfg(target_os = "linux")]
        linux_desktop_reassociation_generation: 0,
        control_server,
        _quit_subscription: quit_subscription,
    });
    start_configuration_watcher(cx);
    silent_mode::start_observer(cx);
    if !launch.no_mux {
        start_multiplexer_session_watcher(cx);
    }
}

/// Installs the process-wide observers: notification responses, the tab
/// overflow menu's keystroke interception, the macOS window actions, keyboard
/// layout changes, and window teardown.
fn install_process_observers(launch: &ApplicationLaunch, cx: &mut App) {
    #[cfg(all(target_os = "macos", feature = "notifications"))]
    cx.on_system_notification_response(|response, cx| {
        let target = macos_notification_target_for_response(
            response.tag.as_ref(),
            response.action_id.as_deref(),
        );
        if let Some(target) = target {
            focus_visible_tab_by_attention_id(cx, target.attention_id);
        }
    });
    install_tab_overflow_keystroke_interception(cx);
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &NewWindow, cx| {
        open_fresh_zetta_window(cx).log_err();
    });
    #[cfg(target_os = "macos")]
    cx.on_action(|action: &OpenProfileWindow, cx| {
        open_fresh_zetta_window_with_profile_and_activation_token(
            cx,
            Some(action.profile.clone()),
            None,
        )
        .log_err();
    });
    let keymap_path = launch.keymap_path.clone();
    let profile_count = launch.profile_count;
    let no_mux = launch.no_mux;
    cx.on_keyboard_layout_change(move |cx| {
        let keymap_path = keymap_path.clone();
        cx.defer(move |cx| {
            load_keybindings(&keymap_path, profile_count, no_mux, cx);
        });
    })
    .detach();
    cx.on_window_closed(handle_zetta_window_closed).detach();
}

/// Lets the tab overflow menu cycle with tab and page keys.
///
/// `intercept_keystrokes` fires before action dispatch, which is what lets
/// `stop_propagation` here claim keys a keybinding would otherwise take. The
/// handler returns early unless the menu is open, so those keys keep their
/// normal meaning the rest of the time.
fn install_tab_overflow_keystroke_interception(cx: &mut App) {
    cx.intercept_keystrokes(|event, _window, cx| {
        let reverse = match event.keystroke.key.as_str() {
            "tab" => event.keystroke.modifiers.shift,
            "pageup" => false,
            "pagedown" => true,
            _ => return,
        };
        let Some(window_handle) = cx.active_window() else {
            return;
        };
        let should_cycle = window_handle
            .update(cx, |view, _window, cx| {
                view.downcast::<Zetta>()
                    .is_ok_and(|zetta| zetta.read(cx).tab_overflow_keyboard_menu_edge.is_some())
            })
            .unwrap_or(false);
        if !should_cycle {
            return;
        }

        cx.stop_propagation();
        cx.defer(move |cx| {
            let _ = window_handle.update(cx, |view, window, cx| {
                let Ok(zetta) = view.downcast::<Zetta>() else {
                    return;
                };
                zetta.update(cx, |zetta, cx| {
                    if reverse {
                        zetta.previous_tab(&PreviousTab, window, cx);
                    } else {
                        zetta.next_tab(&NextTab, window, cx);
                    }
                });
            });
        });
    })
    .detach();
}

/// Opens the window this launch asked for, if it asks for one at all.
fn open_launch_window(launch: ApplicationLaunch, cx: &mut App) {
    if launch.embedding {
        return;
    }
    let opened = if launch.fresh_window {
        open_fresh_zetta_window_with_profile_and_activation_token(
            cx,
            launch.fresh_window_profile,
            launch.activation_token,
        )
    } else {
        open_zetta_window(
            launch.config,
            launch.configuration_error,
            launch.initial_profile,
            launch.initial_project,
            launch.launch_theme_override,
            launch.launch_split,
            launch.profiling,
            launch.performance_report,
            launch.profile_pane_stress,
            launch.no_mux,
            launch.initial_command,
            launch.initial_working_directory,
            None,
            None,
            cx,
        )
    };
    opened.expect("failed to open Zetta window");
}

#[cfg(test)]
#[path = "tests/startup.rs"]
mod tests;

/// The sessions the multiplexer is holding, as reconnect entries.
///
/// Read from the published catalog rather than by asking the multiplexer,
/// because this runs whenever the session list might have changed and must not
/// cost a round trip. Catalogs published by *this* process are skipped: those
/// describe sessions kept in memory here because the multiplexer was
/// unreachable, and they are already in the list.
fn multiplexer_session_entries() -> Vec<ProcessBackgroundSessionEntry> {
    let catalogs = match crate::background_sessions::read_session_catalogs(
        &crate::background_sessions::session_catalog_dir(),
    ) {
        Ok(catalogs) => catalogs,
        Err(error) => {
            log::debug!("could not read the session catalog: {error:#}");
            return Vec::new();
        }
    };
    // Only the multiplexer's own catalog counts: a Zetta process that kept a
    // session in memory because the multiplexer was unreachable publishes one
    // too, and those sessions are this process's to transfer, not the daemon's
    // to attach.
    let entries = crate::background_sessions::multiplexer_held_catalog_sessions(
        &catalogs,
        crate::background_sessions::process_is_zetta,
        std::process::id(),
    )
    .map(|(catalog, session)| {
        let runner_id = catalog.runner_id;
        let details = if session.authentication_required {
            format!("Session {} · protected", session.id)
        } else {
            let applications = session
                .panes
                .iter()
                .map(|pane| pane.application.as_str())
                .collect::<Vec<_>>();
            let panes = session.panes.len();
            let mut details = format!(
                "Session {} · {panes} pane{}",
                session.id,
                if panes == 1 { "" } else { "s" }
            );
            if !applications.is_empty() {
                details.push_str(" · ");
                details.push_str(&applications.join(", "));
            }
            details
        };
        (runner_id, session.id, session.title.clone(), details)
    })
    .collect::<Vec<_>>();
    #[cfg(feature = "session-persistence")]
    let mut entries = entries;
    #[cfg(feature = "session-persistence")]
    if let Ok(records) =
        zmux::persistence::read_opaque_records(&crate::background_sessions::session_catalog_dir())
    {
        let live_ids = entries.iter().map(|(_, session_id, _, _)| *session_id);
        let live_ids = live_ids.collect::<std::collections::HashSet<_>>();
        entries.extend(
            records
                .into_iter()
                .filter(|record| record.restorable && !live_ids.contains(&record.id))
                .map(|record| {
                    (
                        crate::background_sessions::RESTORABLE_RUNNER_ID,
                        record.id,
                        "Restorable session".to_owned(),
                        format!(
                            "Session {} · encrypted disk record{}",
                            record.id,
                            if record.protected {
                                " · protected"
                            } else {
                                ""
                            }
                        ),
                    )
                }),
        );
    }
    entries
}
