use super::*;
#[cfg(cli_services)]
use crate::cli_services::CliServiceCommand;
use crate::cli_services::NotificationTarget;
#[cfg(feature = "clipboard")]
use crate::cli_services::{copy_help, parse_copy_args, parse_paste_args, paste_help};
#[cfg(feature = "http-server")]
use crate::cli_services::{http_server_help, parse_http_args};
#[cfg(feature = "notifications")]
use crate::cli_services::{notify_help, parse_notify_args, run_notification};
#[cfg(feature = "serial-console")]
use crate::cli_services::{parse_serial_args, serial_help};
#[cfg(feature = "tftp-server")]
use crate::cli_services::{parse_tftp_server_args, tftp_server_help};
use crate::process_control::{
    ReplacePaneRequest, TabAttentionRequest, request_existing_process_pane_overlay,
    request_existing_process_pane_theme, request_existing_process_pane_theme_list,
    request_existing_process_replace_pane, request_existing_process_tab_icon,
    request_process_tab_attention,
};
use crate::worktree_cli;

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
mod keybindings;
mod wsl;

pub(crate) use arg_parsing::{
    StartupArgs, StartupMode, load_startup_config, native_terminal_environment, parse_args,
};
use arg_parsing::{
    configured_split_names, parse_attention_target, select_launch_profile,
    should_handoff_to_existing_process, should_replace_pane_in_existing_process,
    validate_launch_split,
};
#[cfg(not(feature = "tftp-client"))]
pub(crate) use cli_help::{TftpCommand, parse_tftp_args, tftp_help};
pub(crate) use keybindings::{
    PROFILE_SHORTCUT_KEYS, keymap_keystroke_display, keymap_keystroke_storage, load_keybindings,
    profile_keybindings, profile_shortcut_label,
};
#[cfg(test)]
pub(crate) use keybindings::{RENAME_TAB_KEYBINDING, keymap_keystroke_alias};
#[cfg(target_os = "macos")]
pub(crate) use keybindings::{install_native_macos_menus, update_native_macos_menus};
use wsl::paths_for_external_editor;
pub(crate) use wsl::{
    add_wsl_environment_variables, is_wsl_shell, launch_working_directory,
    msys2_cwd_tracking_environment, msys2_path_to_windows, msys2_profile, wsl_cwd_tracking_file,
    wsl_shell_with_tracking,
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

pub(crate) fn with_zetta_theme_overrides(theme: Arc<Theme>) -> Arc<Theme> {
    let mut theme = theme.as_ref().clone();
    let colors = &mut theme.styles.colors;
    colors.scrollbar_thumb_background = colors.text_muted.opacity(0.7);
    colors.scrollbar_thumb_hover_background = colors.text.opacity(0.85);
    colors.scrollbar_thumb_active_background = colors.text_accent.opacity(0.95);
    Arc::new(theme)
}

pub(crate) fn apply_zetta_theme_overrides(cx: &mut App) {
    GlobalTheme::update_theme(cx, with_zetta_theme_overrides(cx.theme().clone()));
}

pub(crate) fn resolve_profile_theme(profile: &Profile, cx: &App) -> Result<Option<Arc<Theme>>> {
    profile
        .theme
        .as_deref()
        .map(|name| {
            ThemeRegistry::global(cx)
                .get(name)
                .map(with_zetta_theme_overrides)
                .with_context(|| format!("using theme {name:?} for profile {:?}", profile.name))
        })
        .transpose()
}

pub(crate) fn apply_config_settings(config: &Config, cx: &mut App) -> Result<()> {
    let theme_name = selected_theme_name(config.theme.as_deref());
    let theme = ThemeRegistry::global(cx)
        .get(theme_name)
        .with_context(|| format!("using Zed theme {theme_name:?}"))?;
    GlobalTheme::update_theme(cx, theme);
    apply_zetta_theme_overrides(cx);

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
    launch_theme_override: Option<(String, String)>,
    launch_split: Option<String>,
    enable_performance_overlay: bool,
    performance_report: Option<(PerformanceReportOptions, PerformanceReportStatus)>,
    profile_pane_stress: bool,
    cx: &mut App,
) -> Result<()> {
    let options = zetta_window_options(cx);
    cx.open_window(options, move |window, cx| {
        window.set_window_title("Zetta");
        let zetta = cx.new(|cx| {
            Zetta::new(
                config,
                configuration_error,
                initial_profile,
                launch_theme_override,
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

pub(crate) fn zetta_for_runner(runner_id: u64, cx: &App) -> Option<Entity<Zetta>> {
    if !cx.has_global::<ZettaProcessState>() {
        return None;
    }
    cx.global::<ZettaProcessState>()
        .runners
        .get(&runner_id)
        .cloned()
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
    cx.quit();
}

pub(crate) fn open_dormant_or_new_window(cx: &mut App) -> Result<()> {
    let (existing, dormant, config, configuration_error) = {
        let process = cx.global_mut::<ZettaProcessState>();
        (
            process
                .windows
                .iter()
                .next()
                .map(|(window_id, entity)| (*window_id, entity.clone())),
            process.dormant.pop(),
            process.config.clone(),
            process.configuration_error.clone(),
        )
    };
    if let Some((window_id, _)) = existing {
        gpui::WindowHandle::<Zetta>::new(window_id).update(cx, |zetta, window, cx| {
            zetta.resume_hidden_window(window, cx)
        })?;
        cx.activate(true);
        return Ok(());
    }
    if let Some(zetta) = dormant {
        let zetta_for_window = zetta.clone();
        cx.open_window(zetta_window_options(cx), move |window, cx| {
            window.set_window_title("Zetta");
            zetta_for_window.update(cx, |zetta, cx| zetta.attach_to_reopened_window(window, cx));
            track_zetta_window(&zetta_for_window, window, cx);
            prepare_background_tabs_before_window_close(&zetta_for_window, window, cx);
            zetta_for_window
        })?;
        cx.activate(true);
        Ok(())
    } else {
        open_zetta_window(
            config,
            configuration_error,
            None,
            None,
            None,
            false,
            None,
            false,
            cx,
        )
    }
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
    };
    config.profiles = vec![Profile {
        name: "Terminal rendering profiler".to_owned(),
        command: Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec!["benchmark".to_owned(), workload_argument.to_owned()],
            title_override: Some("Terminal rendering profiler".to_owned()),
        },
        theme: None,
    }];
    config.default_profile = 0;
    config
}

fn checkerboard_background(row: usize, column: usize, frame: u64) -> u8 {
    if (row + column + frame as usize).is_multiple_of(2) {
        41
    } else {
        44
    }
}

struct TerminalStateRestore;

impl Drop for TerminalStateRestore {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(b"\x1b[0m\x1b[?25h\r\n");
        let _ = stdout.flush();
    }
}

fn run_terminal_rendering_workload(
    workload: PerformanceWorkload,
    duration: Option<Duration>,
) -> Result<()> {
    const FRAME_INTERVAL: Duration = Duration::from_nanos(4_166_667);
    const SPARSE_UPDATE_INTERVAL: Duration = Duration::from_millis(25);
    const ROW: &str = "0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ │─╭╮╰╯ ✓ rendered cell workload";

    let _restore_terminal_state = TerminalStateRestore;
    let stdout = std::io::stdout();
    let mut output = std::io::BufWriter::new(stdout.lock());
    output.write_all(b"\x1b[2J\x1b[?25l")?;
    if workload == PerformanceWorkload::SparseUpdates {
        output.write_all(
            b"\x1b[H\x1b[1;36mZetta sparse terminal update profiler\x1b[0m\r\n\
              40 Hz producer updating only this status line\r\n\
              Dense unchanged content below models a full-screen TUI.\r\n\r\n",
        )?;
        for row in 0..34 {
            writeln!(output, "{row:02} {ROW}\r")?;
        }
        output.flush()?;
    }
    let mut frame = 0_u64;
    let mut next_frame = Instant::now();
    let deadline = duration.map(|duration| next_frame + duration);
    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        if workload == PerformanceWorkload::SparseUpdates {
            let spinner = ['|', '/', '-', '\\'][(frame as usize) % 4];
            write!(
                output,
                "\x1b[2;1H40 Hz sparse producer · processing {spinner} · frame {frame:010}"
            )?;
            output.flush()?;
            frame = frame.wrapping_add(1);

            next_frame += SPARSE_UPDATE_INTERVAL;
            let now = Instant::now();
            let wake_at = deadline.map_or(next_frame, |deadline| next_frame.min(deadline));
            if wake_at > now {
                std::thread::sleep(wake_at - now);
            } else {
                next_frame = now;
            }
            continue;
        }

        let workload_description = match workload {
            PerformanceWorkload::Standard => "text and line-drawing cells",
            PerformanceWorkload::CheckerboardBackground => "alternating cell backgrounds",
            PerformanceWorkload::SparseUpdates => unreachable!(),
        };
        if write!(
            output,
            "\x1b[H\x1b[1;36mZetta terminal rendering profiler\x1b[0m\r\n\
             240 Hz producer · {workload_description} · frame {frame:010}\r\n\
             This deterministic workload is identical on Linux, macOS, and Windows.\r\n\r\n"
        )
        .is_err()
        {
            return Ok(());
        }
        for row in 0..34 {
            match workload {
                PerformanceWorkload::Standard => {
                    writeln!(output, "{row:02} {ROW} {frame:010}\r")?;
                }
                PerformanceWorkload::CheckerboardBackground => {
                    write!(output, "{row:02} ")?;
                    for column in 0..96 {
                        let background = checkerboard_background(row, column, frame);
                        write!(output, "\x1b[{background}m ")?;
                    }
                    write!(output, "\x1b[0m\r\n")?;
                }
                PerformanceWorkload::SparseUpdates => unreachable!(),
            }
        }
        output.flush()?;
        frame = frame.wrapping_add(1);

        next_frame += FRAME_INTERVAL;
        let now = Instant::now();
        let wake_at = deadline.map_or(next_frame, |deadline| next_frame.min(deadline));
        if wake_at > now {
            std::thread::sleep(wake_at - now);
        } else {
            next_frame = now;
        }
    }
    Ok(())
}

fn selected_performance_workload(args: &StartupArgs) -> PerformanceWorkload {
    if args.profile_background_stress {
        PerformanceWorkload::CheckerboardBackground
    } else if args.profile_sparse_updates {
        PerformanceWorkload::SparseUpdates
    } else {
        PerformanceWorkload::Standard
    }
}

pub(crate) fn run() -> Result<()> {
    let args = parse_args()?;
    if args.mode == StartupMode::Application {
        terminal_view::start_scrollback_cleanup_monitor();
    }
    if let StartupMode::Edit {
        arguments,
        delete_after,
    } = &args.mode
    {
        let (arguments, cleanup_path) = if *delete_after {
            let path = terminal_view::claim_scrollback_for_editor(Path::new(&arguments[0]))
                .context("claiming the managed scrollback file")?;
            (vec![path.to_string_lossy().into_owned()], Some(path))
        } else {
            (arguments.clone(), None)
        };
        let editor = env::var("EDITOR")
            .ok()
            .filter(|editor| !editor.trim().is_empty());
        let result: Result<i32> = (|| {
            if let Some(editor) = editor {
                let mut editor_parts = task::ShellKind::system()
                    .split(&editor)
                    .filter(|parts| !parts.is_empty())
                    .context("EDITOR does not contain a command")?;
                let program = editor_parts.remove(0);
                std::process::Command::new(&program)
                    .args(editor_parts)
                    .args(paths_for_external_editor(&arguments))
                    .status()
                    .with_context(|| format!("failed to start editor {program:?}"))
                    .map(|status| status.code().unwrap_or(1))
            } else {
                #[cfg(feature = "syntax-highlighting")]
                {
                    Ok(vi_syntax::run(arguments))
                }
                #[cfg(not(feature = "syntax-highlighting"))]
                {
                    Ok(busy_v::run(arguments))
                }
            }
        })();
        if let Some(path) = cleanup_path {
            let _ = terminal_view::remove_scrollback_file(&path);
        }
        std::process::exit(result?);
    }
    if let StartupMode::Vi(arguments) = args.mode {
        #[cfg(feature = "syntax-highlighting")]
        {
            std::process::exit(vi_syntax::run(arguments));
        }
        #[cfg(not(feature = "syntax-highlighting"))]
        {
            std::process::exit(busy_v::run(arguments));
        }
    }
    if let StartupMode::OutputBenchmark {
        size_mib,
        output_type,
    } = &args.mode
    {
        return run_output_benchmark(*size_mib, *output_type);
    }
    if let StartupMode::Attention(command) = &args.mode {
        let inherited_process_id = env::var("ZETTA_PROCESS_ID")
            .context("zetta attention must run inside a Zetta terminal")?;
        let inherited_attention_id = env::var("ZETTA_ATTENTION_ID")
            .context("zetta attention must run inside a Zetta terminal")?;
        let (process_id, attention_id) =
            parse_attention_target(&inherited_process_id, &inherited_attention_id)?;
        let target = NotificationTarget {
            process_id,
            attention_id,
        };
        let accepted = request_process_tab_attention(
            target.process_id,
            TabAttentionRequest {
                attention_id: target.attention_id,
                summary: command.notification.summary.clone(),
                body: command.notification.body.clone(),
            },
        )?;
        anyhow::ensure!(accepted, "the originating Zetta tab is no longer available");
        #[cfg(feature = "notifications")]
        if command.notify {
            run_notification(&command.notification, Some(target))?;
        }
        #[cfg(not(feature = "notifications"))]
        if command.notify {
            anyhow::bail!("desktop notifications are disabled in this build");
        }
        return Ok(());
    }
    if let StartupMode::Worktree(command) = args.mode {
        return worktree_cli::run(&command);
    }
    if let StartupMode::Profile(command) = args.mode {
        let result = crate::profile_cli::run(command, args.config_path.as_deref())?;
        if result.changed {
            crate::profile_cli::reload_after_mutation(&result);
        }
        return Ok(());
    }
    if let StartupMode::PrintTerminalSize { json, resize } = args.mode {
        if let Some(resize) = resize {
            return request_terminal_resize(resize.columns, resize.rows);
        }
        print_terminal_size(json);
        return Ok(());
    }
    if let StartupMode::PrintShellIntegration(shell) = args.mode {
        let (config, _) = load_startup_config(None, None);
        print!("{}", shell.script(&config.profiles));
        return Ok(());
    }
    if args.mode == StartupMode::ConfigureCurrentShellIntegration {
        println!(
            "{}",
            shell_integration_configuration_message(&configure_current_shell_integration()?)
        );
        return Ok(());
    }
    if args.mode == StartupMode::ListTabIcons {
        for icon in tab_icon_completion_names() {
            println!("{icon}");
        }
        return Ok(());
    }
    if let StartupMode::SetTabIcon { icon } = args.mode {
        anyhow::ensure!(
            request_existing_process_tab_icon(icon)?,
            "no running Zetta process accepted the tab icon request"
        );
        return Ok(());
    }
    if let StartupMode::SetPaneTheme { theme } = args.mode {
        anyhow::ensure!(
            request_existing_process_pane_theme(theme)?,
            "no running Zetta process accepted the pane theme request"
        );
        return Ok(());
    }
    if args.mode == StartupMode::ListPaneThemes {
        let themes = request_existing_process_pane_theme_list()?
            .context("no running Zetta process accepted the pane theme list request")?;
        for theme in themes {
            println!("{theme}");
        }
        return Ok(());
    }
    if args.mode == StartupMode::ListPaneSplits {
        let (config, _) = load_startup_config(None, None);
        for name in configured_split_names(&config) {
            println!("{name}");
        }
        return Ok(());
    }
    if let StartupMode::SetPaneOverlay(request) = args.mode {
        anyhow::ensure!(
            request_existing_process_pane_overlay(request)?,
            "no running Zetta process accepted the pane overlay request"
        );
        return Ok(());
    }
    match &args.mode {
        StartupMode::ListBackgroundSessions { json } => return print_session_catalogs(*json),
        StartupMode::ReconnectBackgroundSession { identifier } => {
            return crate::session_cli::run_reconnect_session(identifier);
        }
        _ => {}
    }
    #[cfg(cli_services)]
    if let StartupMode::CliService(command) = &args.mode {
        return command.run();
    }
    if should_replace_pane_in_existing_process(&args) {
        let request = ReplacePaneRequest {
            split: args.split.clone(),
            profile: args.profile.clone(),
            theme: args.theme_override.clone(),
        };
        if request_existing_process_replace_pane(request)? {
            return Ok(());
        }
    }
    if should_handoff_to_existing_process(&args) && request_existing_process_window()? {
        return Ok(());
    }
    #[cfg(windows)]
    if let StartupMode::RegisterWindowsShell(shortcut_path) = &args.mode {
        let (config, _) =
            load_startup_config(args.config_path.as_deref(), args.keymap_path.clone());
        return windows_integration::register_shell_integration(shortcut_path, &config.profiles);
    }
    if let Some(command) = &args.tftp_command {
        return command.run();
    }
    if args.mode == StartupMode::TerminalRenderingWorkload {
        return run_terminal_rendering_workload(PerformanceWorkload::Standard, None);
    }
    if args.mode == StartupMode::TerminalCheckerboardWorkload {
        return run_terminal_rendering_workload(PerformanceWorkload::CheckerboardBackground, None);
    }
    if args.mode == StartupMode::TerminalSparseUpdateWorkload {
        return run_terminal_rendering_workload(PerformanceWorkload::SparseUpdates, None);
    }

    let profiling = args.mode == StartupMode::TerminalRenderingProfile;
    let workload = selected_performance_workload(&args);
    if profiling && args.profile_external_terminal {
        return run_terminal_rendering_workload(workload, args.profile_duration);
    }
    let report_options = args
        .profile_report
        .zip(args.profile_duration)
        .map(|(path, duration)| PerformanceReportOptions {
            path,
            duration,
            workload,
        });
    let report_requested = report_options.is_some();
    let report_status = Arc::new(Mutex::new(None));
    let (config, configuration_error) = if profiling {
        (
            terminal_rendering_profile_config(&env::current_exe()?, workload),
            None,
        )
    } else {
        load_startup_config(args.config_path.as_deref(), args.keymap_path)
    };
    validate_launch_split(&config, args.split.as_deref())?;
    let initial_profile = select_launch_profile(&config, args.profile.as_deref())?;
    // Keyed by profile name (case-insensitive) rather than baked into
    // `initial_profile.theme`, so every tab opened with this profile for the
    // rest of the process gets the override too, not just the first one.
    // Applied in `Zetta::open_tab_with_profile`; never written back to
    // `config.profiles` or the settings UI.
    let launch_theme_override = initial_profile
        .as_ref()
        .zip(args.theme_override.as_ref())
        .map(|(profile, theme)| (profile.name.to_lowercase(), theme.clone()));
    let keymap_path = config.keymap_path.clone();
    let profile_count = visible_profile_count(&config.profiles, &config.hidden_profiles);
    let http_client = Arc::new(
        reqwest_client::ReqwestClient::user_agent(concat!("Zetta/", env!("CARGO_PKG_VERSION")))
            .context("initializing HTTP client")?,
    );
    let report_status_for_app = report_status.clone();
    gpui_platform::application()
        .with_quit_mode(zetta_quit_mode())
        .with_assets(ZettaAssets)
        .run(move |cx: &mut App| {
            #[cfg(windows)]
            {
                cx.set_app_identity(ZETTA_APP_ID, "Zetta");
                windows_integration::update_profile_jump_list(config.profiles.clone());
            }
            cx.set_http_client(http_client);
            menu::init();
            zed_actions::init();
            release_channel::init(semver::Version::new(0, 1, 0), cx);
            settings::init(cx);
            theme_settings::init(theme::LoadThemes::All(Box::new(ZettaAssets)), cx);
            load_user_themes(cx).log_err();
            ZettaAssets.load_fonts(cx).log_err();
            apply_config_settings(&config, cx).expect("failed to apply Zetta configuration");
            load_keybindings(&keymap_path, profile_count, cx);
            #[cfg(target_os = "macos")]
            install_native_macos_menus(
                cx,
                &config.profiles,
                &config.hidden_profiles,
                config.default_profile,
            );
            let (control_tx, mut control_rx) = futures::channel::mpsc::unbounded();
            let control_server = ProcessControlServer::start(control_tx)
                .expect("failed to start Zetta process control");
            let quit_subscription = cx.on_app_quit(|cx| {
                if cx.has_global::<ZettaProcessState>() {
                    cx.global::<ZettaProcessState>()
                        .control_server
                        .begin_shutdown();
                }
                async {}
            });
            cx.set_global(ZettaProcessState {
                windows: HashMap::new(),
                dormant: Vec::new(),
                runners: HashMap::new(),
                next_attention_id: 1,
                silent_mode: SilentModeState::default(),
                background_session_entries: Arc::from([]),
                config: config.clone(),
                configuration_error: configuration_error.clone(),
                control_server,
                _quit_subscription: quit_subscription,
            });
            silent_mode::start_observer(cx);
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
                        view.downcast::<Zetta>().is_ok_and(|zetta| {
                            zetta.read(cx).tab_overflow_keyboard_menu_edge.is_some()
                        })
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
            #[cfg(target_os = "macos")]
            cx.on_action(|_: &NewWindow, cx| {
                open_dormant_or_new_window(cx).log_err();
            });
            cx.spawn(async move |cx| {
                while let Some(command) = control_rx.next().await {
                    match command {
                        ProcessControlCommand::ReloadConfiguration {
                            config_path,
                            completion,
                        } => {
                            let accepted = cx.update(|cx| {
                                if !cx
                                    .global::<ZettaProcessState>()
                                    .control_server
                                    .is_accepting()
                                {
                                    return false;
                                }
                                let process = cx.global::<ZettaProcessState>();
                                if crate::process_control::config_path_identity(
                                    &process.config.config_path,
                                ) != config_path
                                {
                                    return false;
                                }
                                let config_path = process.config.config_path.clone();
                                let keymap_override = process.config.keymap_override.clone();
                                let config = match Config::load(Some(&config_path), keymap_override)
                                {
                                    Ok(config) => config,
                                    Err(error) => {
                                        eprintln!(
                                            "Could not reload {}: {error:#}",
                                            config_path.display()
                                        );
                                        return false;
                                    }
                                };
                                let entities = process_zetta_entities(cx);
                                for entity in entities {
                                    if let Err(error) = entity.update(cx, |zetta, cx| {
                                        zetta.reload_configuration_from_process(config.clone(), cx)
                                    }) {
                                        eprintln!(
                                            "Could not apply reloaded configuration {}: {error:#}",
                                            config_path.display()
                                        );
                                        return false;
                                    }
                                }
                                let process = cx.global_mut::<ZettaProcessState>();
                                process.config = config;
                                process.configuration_error = None;
                                true
                            });
                            let _ = completion.send(accepted);
                        }
                        ProcessControlCommand::OpenWindow { completion } => {
                            let opened = cx.update(|cx| {
                                if !cx
                                    .global::<ZettaProcessState>()
                                    .control_server
                                    .is_accepting()
                                {
                                    return false;
                                }

                                match open_dormant_or_new_window(cx) {
                                    Ok(()) => true,
                                    Err(error) => {
                                        eprintln!(
                                            "Could not open the requested Zetta window: {error:#}"
                                        );
                                        false
                                    }
                                }
                            });
                            let _ = completion.send(opened);
                        }
                        ProcessControlCommand::ReplacePane {
                            request,
                            completion,
                        } => {
                            let replaced = cx.update(|cx| {
                                if !cx
                                    .global::<ZettaProcessState>()
                                    .control_server
                                    .is_accepting()
                                {
                                    return false;
                                }
                                let Some(window_id) =
                                    cx.active_window().map(|window| window.window_id())
                                else {
                                    return false;
                                };
                                gpui::WindowHandle::<Zetta>::new(window_id)
                                    .update(cx, |zetta, window, cx| {
                                        zetta.replace_active_pane_from_cli(request, window, cx)
                                    })
                                    .unwrap_or(false)
                            });
                            let _ = completion.send(replaced);
                        }
                        ProcessControlCommand::SetTabIcon { icon, completion } => {
                            let accepted = cx.update(|cx| {
                                if !cx
                                    .global::<ZettaProcessState>()
                                    .control_server
                                    .is_accepting()
                                {
                                    return false;
                                }
                                if cx.global::<ZettaProcessState>().windows.is_empty()
                                    && open_dormant_or_new_window(cx).is_err()
                                {
                                    return false;
                                }
                                let Some(window_id) = cx
                                    .global::<ZettaProcessState>()
                                    .windows
                                    .keys()
                                    .next()
                                    .copied()
                                else {
                                    return false;
                                };
                                gpui::WindowHandle::<Zetta>::new(window_id)
                                    .update(cx, |zetta, _, cx| {
                                        zetta.set_active_tab_icon_from_cli(icon, cx)
                                    })
                                    .unwrap_or(false)
                            });
                            let _ = completion.send(accepted);
                        }
                        ProcessControlCommand::SetPaneTheme { theme, completion } => {
                            let accepted = cx.update(|cx| {
                                if !cx
                                    .global::<ZettaProcessState>()
                                    .control_server
                                    .is_accepting()
                                {
                                    return false;
                                }
                                if cx.global::<ZettaProcessState>().windows.is_empty()
                                    && open_dormant_or_new_window(cx).is_err()
                                {
                                    return false;
                                }
                                let Some(window_id) = cx
                                    .global::<ZettaProcessState>()
                                    .windows
                                    .keys()
                                    .next()
                                    .copied()
                                else {
                                    return false;
                                };
                                gpui::WindowHandle::<Zetta>::new(window_id)
                                    .update(cx, |zetta, _, cx| {
                                        zetta.set_active_pane_theme(theme, cx)
                                    })
                                    .unwrap_or(false)
                            });
                            let _ = completion.send(accepted);
                        }
                        ProcessControlCommand::ListPaneThemes { completion } => {
                            let themes = cx.update(|cx| {
                                if !cx
                                    .global::<ZettaProcessState>()
                                    .control_server
                                    .is_accepting()
                                {
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
                            });
                            let _ = completion.send(themes);
                        }
                        ProcessControlCommand::SetPaneOverlay {
                            text,
                            font_size,
                            opacity,
                            color,
                            completion,
                        } => {
                            let accepted = cx.update(|cx| {
                                if !cx
                                    .global::<ZettaProcessState>()
                                    .control_server
                                    .is_accepting()
                                {
                                    return false;
                                }
                                if cx.global::<ZettaProcessState>().windows.is_empty()
                                    && open_dormant_or_new_window(cx).is_err()
                                {
                                    return false;
                                }
                                let Some(window_id) = cx
                                    .global::<ZettaProcessState>()
                                    .windows
                                    .keys()
                                    .next()
                                    .copied()
                                else {
                                    return false;
                                };
                                gpui::WindowHandle::<Zetta>::new(window_id)
                                    .update(cx, |zetta, _, cx| {
                                        zetta.set_active_pane_overlay(
                                            text, font_size, opacity, color, cx,
                                        )
                                    })
                                    .unwrap_or(false)
                            });
                            let _ = completion.send(accepted);
                        }
                        ProcessControlCommand::SetTabAttention {
                            request,
                            completion,
                        } => {
                            let accepted = cx.update(|cx| {
                                if !cx
                                    .global::<ZettaProcessState>()
                                    .control_server
                                    .is_accepting()
                                {
                                    return false;
                                }
                                let process = cx.global::<ZettaProcessState>();
                                let windows = process.windows.keys().copied().collect::<Vec<_>>();
                                let dormant = process.dormant.clone();
                                let mut accepted = false;
                                for window_id in windows {
                                    let found = gpui::WindowHandle::<Zetta>::new(window_id)
                                        .update(cx, |zetta, window, cx| {
                                            let found = zetta.set_tab_attention(
                                                request.clone(),
                                                Some(window),
                                                cx,
                                            );
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
                            });
                            let _ = completion.send(accepted);
                        }
                        ProcessControlCommand::FocusTab {
                            attention_id,
                            completion,
                        } => {
                            let focused = cx.update(|cx| {
                                if !cx
                                    .global::<ZettaProcessState>()
                                    .control_server
                                    .is_accepting()
                                {
                                    return false;
                                }
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
                                                zetta.focus_tab_by_attention_id(
                                                    attention_id,
                                                    window,
                                                    cx,
                                                )
                                            } else {
                                                false
                                            }
                                        })
                                        .unwrap_or(false)
                                })
                            });
                            let _ = completion.send(focused);
                        }
                        ProcessControlCommand::SetTabName {
                            request,
                            completion,
                        } => {
                            let accepted = cx.update(|cx| {
                                if !cx
                                    .global::<ZettaProcessState>()
                                    .control_server
                                    .is_accepting()
                                {
                                    return false;
                                }
                                process_zetta_entities(cx).into_iter().any(|zetta| {
                                    zetta.update(cx, |zetta, cx| {
                                        zetta.set_tab_name(request.clone(), cx)
                                    })
                                })
                            });
                            let _ = completion.send(accepted);
                        }
                        ProcessControlCommand::GetSilentMode { completion } => {
                            let silent_mode = cx.update(|cx| {
                                cx.has_global::<ZettaProcessState>()
                                    && cx
                                        .global::<ZettaProcessState>()
                                        .control_server
                                        .is_accepting()
                                    && cx.global::<ZettaProcessState>().silent_mode.effective()
                            });
                            let _ = completion.send(silent_mode);
                        }
                        ProcessControlCommand::ReconnectSession {
                            runner_id,
                            session_id,
                            secret,
                            completion,
                        } => {
                            let mut completion = Some(completion);
                            let dispatched = cx.update(|cx| {
                                if !cx
                                    .global::<ZettaProcessState>()
                                    .control_server
                                    .is_accepting()
                                {
                                    return false;
                                }
                                if cx.global::<ZettaProcessState>().windows.is_empty()
                                    && open_dormant_or_new_window(cx).is_err()
                                {
                                    return false;
                                }
                                let Some(window_id) = cx
                                    .global::<ZettaProcessState>()
                                    .windows
                                    .keys()
                                    .next()
                                    .copied()
                                else {
                                    return false;
                                };
                                gpui::WindowHandle::<Zetta>::new(window_id)
                                    .update(cx, |zetta, window, cx| {
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
                    }
                }
            })
            .detach();
            let layout_keymap_path = keymap_path.clone();
            cx.on_keyboard_layout_change(move |cx| {
                let layout_keymap_path = layout_keymap_path.clone();
                cx.defer(move |cx| {
                    load_keybindings(&layout_keymap_path, profile_count, cx);
                });
            })
            .detach();
            cx.on_window_closed(handle_zetta_window_closed).detach();

            open_zetta_window(
                config,
                configuration_error,
                initial_profile,
                launch_theme_override,
                args.split,
                profiling,
                report_options.map(|options| (options, report_status_for_app)),
                args.profile_pane_stress,
                cx,
            )
            .expect("failed to open Zetta window");
        });
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

fn shell_integration_configuration_message(
    configuration: &ShellIntegrationConfiguration,
) -> String {
    match configuration {
        ShellIntegrationConfiguration::Written(path) => format!(
            "Added Zetta shell integration to {}. Start a new shell or reload this file to enable it.",
            path.display()
        ),
        ShellIntegrationConfiguration::AlreadyPresent(path) => format!(
            "Zetta shell integration is already present in {}; no changes made.",
            path.display()
        ),
    }
}

#[cfg(test)]
#[path = "tests/startup.rs"]
mod tests;
