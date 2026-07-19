use super::*;

const DEFAULT_PERFORMANCE_REPORT_DURATION: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StartupMode {
    Application,
    TerminalRenderingProfile,
    TerminalRenderingWorkload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartupArgs {
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) keymap_path: Option<PathBuf>,
    pub(crate) mode: StartupMode,
    pub(crate) profile_report: Option<PathBuf>,
    pub(crate) profile_duration: Option<Duration>,
    pub(crate) profile_pane_stress: bool,
    pub(crate) tftp_command: Option<TftpCommand>,
}

pub(crate) fn version_text() -> String {
    format!("Zetta {}", env!("CARGO_PKG_VERSION"))
}

fn is_version_argument(argument: &str) -> bool {
    matches!(argument, "--version" | "-v")
}

pub(crate) fn parse_args_from(args: impl IntoIterator<Item = OsString>) -> Result<StartupArgs> {
    let arguments = args.into_iter().collect::<Vec<_>>();
    if arguments.first().is_some_and(|argument| argument == "tftp") {
        let tftp_arguments = &arguments[1..];
        if tftp_arguments
            .iter()
            .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
        {
            println!("{}", tftp_help());
            std::process::exit(0);
        }
        return Ok(StartupArgs {
            config_path: None,
            keymap_path: None,
            mode: StartupMode::Application,
            profile_report: None,
            profile_duration: None,
            profile_pane_stress: false,
            tftp_command: Some(parse_tftp_args(tftp_arguments.iter().cloned())?),
        });
    }
    let mut config = None;
    let mut keymap = None;
    let mut mode = StartupMode::Application;
    let mut profile_report = None;
    let mut profile_duration = None;
    let mut profile_pane_stress = false;
    let mut args = arguments.into_iter();
    while let Some(argument) = args.next() {
        let argument = argument.to_string_lossy();
        if is_version_argument(&argument) {
            println!("{}", version_text());
            std::process::exit(0);
        }
        match argument.as_ref() {
            "--config" | "-c" => {
                config = Some(args.next().context("--config requires a path")?.into())
            }
            "--keymap" | "-k" => {
                keymap = Some(args.next().context("--keymap requires a path")?.into())
            }
            "--profile-terminal-rendering" | "-p" => mode = StartupMode::TerminalRenderingProfile,
            "--profile-pane-stress" | "-s" => profile_pane_stress = true,
            "--terminal-render-workload" => mode = StartupMode::TerminalRenderingWorkload,
            "--profile-report" | "-r" => {
                profile_report = Some(
                    args.next()
                        .context("--profile-report requires a path")?
                        .into(),
                )
            }
            "--profile-duration" | "-d" => {
                let seconds = args
                    .next()
                    .context("--profile-duration requires seconds")?
                    .to_string_lossy()
                    .parse::<f64>()
                    .context("--profile-duration must be a number of seconds")?;
                anyhow::ensure!(
                    seconds.is_finite() && seconds > 0.0,
                    "--profile-duration must be greater than zero"
                );
                profile_duration = Some(Duration::from_secs_f64(seconds));
            }
            "--help" | "-h" => {
                println!(
                    "Zetta terminal\n\nUsage: zetta [OPTIONS]\n       zetta tftp <COMMAND> [OPTIONS]\n\nCommands:\n  tftp                                Transfer a file with TFTP\n\nOptions:\n  -h, --help                          Print help\n  -v, --version                       Print version\n  -c, --config PATH                   Use a configuration file\n  -k, --keymap PATH                   Use a keymap file\n  -p, --profile-terminal-rendering    Profile terminal rendering\n  -s, --profile-pane-stress           Use 64 panes with 63 minimized\n  -r, --profile-report PATH           Write a profiling report\n  -d, --profile-duration SECONDS      Set the profiling duration"
                );
                std::process::exit(0);
            }
            unknown => anyhow::bail!("unknown argument {unknown:?}"),
        }
    }
    anyhow::ensure!(
        mode == StartupMode::Application || (config.is_none() && keymap.is_none()),
        "profiling modes cannot be combined with --config or --keymap"
    );
    anyhow::ensure!(
        (profile_report.is_none() && profile_duration.is_none())
            || mode == StartupMode::TerminalRenderingProfile,
        "--profile-report and --profile-duration require --profile-terminal-rendering"
    );
    anyhow::ensure!(
        profile_duration.is_none() || profile_report.is_some(),
        "--profile-duration requires --profile-report"
    );
    anyhow::ensure!(
        !profile_pane_stress || mode == StartupMode::TerminalRenderingProfile,
        "--profile-pane-stress requires --profile-terminal-rendering"
    );
    if profile_report.is_some() && profile_duration.is_none() {
        profile_duration = Some(DEFAULT_PERFORMANCE_REPORT_DURATION);
    }
    Ok(StartupArgs {
        config_path: config,
        keymap_path: keymap,
        mode,
        profile_report,
        profile_duration,
        profile_pane_stress,
        tftp_command: None,
    })
}

pub(crate) fn parse_args() -> Result<StartupArgs> {
    parse_args_from(env::args_os().skip(1))
}

#[cfg(windows)]
fn path_with_entry_first(path: Option<&std::ffi::OsStr>, entry: &Path) -> Option<OsString> {
    let inherited = path.map(env::split_paths).into_iter().flatten();
    let entries = inherited.collect::<Vec<_>>();
    let entry_text = entry.to_string_lossy();
    if entries.iter().any(|candidate| {
        candidate
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .eq_ignore_ascii_case(entry_text.trim_end_matches(['\\', '/']))
    }) {
        return None;
    }
    env::join_paths(std::iter::once(entry.to_path_buf()).chain(entries)).ok()
}

#[cfg(windows)]
pub(crate) fn native_terminal_environment() -> Vec<(String, String)> {
    let Some(executable_directory) = env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
    else {
        return Vec::new();
    };
    let Some(path) = path_with_entry_first(env::var_os("PATH").as_deref(), &executable_directory)
    else {
        return Vec::new();
    };
    vec![("PATH".to_owned(), path.to_string_lossy().into_owned())]
}

#[cfg(not(windows))]
pub(crate) fn native_terminal_environment() -> Vec<(String, String)> {
    Vec::new()
}

pub(crate) fn load_startup_config(
    config_path: Option<&Path>,
    keymap_path: Option<PathBuf>,
) -> (Config, Option<String>) {
    match Config::load(config_path, keymap_path.clone()) {
        Ok(config) => (config, None),
        Err(error) => (
            Config::defaults(config_path, keymap_path),
            Some(format!("Could not load configuration: {error:#}")),
        ),
    }
}

pub(crate) fn profile_keybindings(
    slot: usize,
    keyboard_mapper: &dyn PlatformKeyboardMapper,
) -> [KeyBinding; 1] {
    const SHIFTED_DIGITS: [&str; 9] = ["!", "@", "#", "$", "%", "^", "&", "*", "("];
    let action = OpenProfile { slot };
    let context = Some(
        KeyBindingContextPredicate::parse("Zetta > Terminal")
            .expect("built-in keybinding context must be valid")
            .into(),
    );
    let binding = |keystroke: &str, action: OpenProfile| {
        KeyBinding::load(
            keystroke,
            Box::new(action),
            context.clone(),
            true,
            None,
            keyboard_mapper,
        )
        .expect("built-in profile keystroke must be valid")
    };
    [binding(
        &format!("ctrl-{}", SHIFTED_DIGITS[slot - 1]),
        action,
    )]
}

pub(crate) fn pane_template_keybindings() -> [KeyBinding; 2] {
    [
        KeyBinding::new(
            "alt-shift-o",
            ApplyPaneSplitTemplate {
                name: "three-right".to_owned(),
            },
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "alt-shift-e",
            ApplyPaneSplitTemplate {
                name: "quarters".to_owned(),
            },
            Some("Zetta > Terminal"),
        ),
    ]
}

pub(crate) fn profile_shortcut_label(slot: usize) -> Option<String> {
    (1..=9)
        .contains(&slot)
        .then(|| format!("Ctrl+Shift+{slot}"))
}

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

pub(crate) fn is_wsl_shell(shell: &Shell) -> bool {
    let program = match shell {
        Shell::System => return false,
        Shell::Program(program) | Shell::WithArguments { program, .. } => program,
    };
    program
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("wsl.exe"))
}

pub(crate) fn launch_working_directory(
    profile: &Profile,
    inherited: Option<PathBuf>,
    inherited_wsl: Option<String>,
    fallback: Option<PathBuf>,
    fallback_is_configured: bool,
) -> (Option<PathBuf>, Option<String>) {
    // Windows process inspection sees the cwd of wsl.exe, not of its Linux shell.
    // Passing that value to a new WSL session leaks Zetta's own launch directory.
    let is_wsl = is_wsl_shell(&profile.command);
    let has_inherited_wsl = inherited_wsl.is_some();
    let working_directory = if is_wsl && has_inherited_wsl {
        None
    } else if is_wsl {
        fallback_is_configured.then_some(fallback).flatten()
    } else {
        inherited.or(fallback)
    };
    let wsl_directory = if is_wsl && has_inherited_wsl {
        inherited_wsl
    } else {
        (is_wsl && !fallback_is_configured).then(|| "~".to_owned())
    };
    (working_directory, wsl_directory)
}

pub(crate) fn wsl_cwd_tracking_file(profile: &Profile, pane_id: u64) -> Option<PathBuf> {
    (cfg!(windows) && is_wsl_shell(&profile.command)).then(|| {
        let path = env::temp_dir().join(format!("zetta-wsl-cwd-{}-{pane_id}", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    })
}

pub(crate) const WSL_CWD_TRACKER: &str = r#"marker="$(wslpath -u "$1" 2>/dev/null || true)"
shell="${SHELL:-}"
if [ ! -x "$shell" ]; then
    shell="$(getent passwd "$(id -u)" 2>/dev/null | cut -d: -f7)"
fi
[ -x "$shell" ] || shell=/bin/sh

cwd_command='case "$PWD" in /*) printf "\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\" "$PWD" "$PWD";; esac'
case "${shell##*/}" in
    bash)
        PROMPT_COMMAND="${cwd_command}${PROMPT_COMMAND:+;${PROMPT_COMMAND}}"
        export PROMPT_COMMAND
        exec "$shell" -l
        ;;
    fish)
        exec "$shell" -l -C 'function __zetta_report_cwd --on-event fish_prompt; if string match -qr "^/" -- "$PWD"; printf "\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\" "$PWD" "$PWD"; end; end'
        ;;
    zsh)
        integration_zdotdir="$(mktemp -d "${TMPDIR:-/tmp}/zetta-zsh-XXXXXX" 2>/dev/null || true)"
        if [ -n "$integration_zdotdir" ]; then
            export ZETTA_ORIGINAL_ZDOTDIR="${ZDOTDIR:-$HOME}"
            export ZETTA_INTEGRATION_ZDOTDIR="$integration_zdotdir"
            cat > "$integration_zdotdir/.zshenv" <<'ZETTA_ZSHENV'
ZDOTDIR="$ZETTA_ORIGINAL_ZDOTDIR"
[[ -r "$ZDOTDIR/.zshenv" ]] && source "$ZDOTDIR/.zshenv"

function __zetta_report_cwd() {
    [[ "$PWD" == /* ]] && printf '\033]7;file://localhost%s\033\\\033]2;zetta-cwd:%s\033\\' "$PWD" "$PWD"
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __zetta_report_cwd
command rm -rf -- "$ZETTA_INTEGRATION_ZDOTDIR"
unset ZETTA_ORIGINAL_ZDOTDIR ZETTA_INTEGRATION_ZDOTDIR
ZETTA_ZSHENV
            ZDOTDIR="$integration_zdotdir"
            export ZDOTDIR
            exec "$shell" -l
        fi
        ;;
esac

# Shells without an injection mechanism retain the legacy tracker.
parent=$$
if [ -n "$marker" ]; then
    (
        previous=
        while kill -0 "$parent" 2>/dev/null; do
            cwd="$(readlink "/proc/$parent/cwd" 2>/dev/null)" || break
            if [ "$cwd" != "$previous" ]; then
                printf '%s\n' "$cwd" > "${marker}.tmp" && mv -f "${marker}.tmp" "$marker"
                previous="$cwd"
            fi
            sleep 0.1
        done
        rm -f "$marker" "${marker}.tmp"
    ) </dev/null >/dev/null 2>&1 &
fi
exec "$shell" -l"#;

pub(crate) fn wsl_shell_with_tracking(
    shell: Shell,
    directory: Option<&str>,
    cwd_file: Option<&Path>,
) -> Shell {
    match shell {
        Shell::Program(program) => {
            wsl_command_with_tracking(program, Vec::new(), None, directory, cwd_file)
        }
        Shell::WithArguments {
            program,
            args,
            title_override,
        } => wsl_command_with_tracking(program, args, title_override, directory, cwd_file),
        Shell::System => Shell::System,
    }
}

pub(crate) fn wsl_command_with_tracking(
    program: String,
    mut args: Vec<String>,
    title_override: Option<String>,
    directory: Option<&str>,
    cwd_file: Option<&Path>,
) -> Shell {
    let exec_index = args.iter().position(|arg| arg == "--exec" || arg == "-e");
    if let Some(directory) = directory
        && !args
            .iter()
            .take(exec_index.unwrap_or(args.len()))
            .any(|arg| arg == "--cd" || arg.starts_with("--cd="))
    {
        args.splice(
            exec_index.unwrap_or(args.len())..exec_index.unwrap_or(args.len()),
            ["--cd".to_owned(), directory.to_owned()],
        );
    }
    if exec_index.is_none()
        && let Some(cwd_file) = cwd_file
    {
        args.extend([
            "--exec".to_owned(),
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            WSL_CWD_TRACKER.to_owned(),
            "zetta-wsl-cwd".to_owned(),
            cwd_file.to_string_lossy().into_owned(),
        ]);
    }
    Shell::WithArguments {
        program,
        args,
        title_override,
    }
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
    content
        .replace("page-up", "pageup")
        .replace("page-down", "pagedown")
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

pub(crate) const RENAME_TAB_KEYBINDING: &str = "ctrl-alt-r";
pub(crate) const RENAME_PANE_KEYBINDING: &str = "ctrl-alt-l";
pub(crate) const CLOSE_PANE_KEYBINDING: &str = "ctrl-shift-x";
pub(crate) const SAVE_PANE_OUTPUT_KEYBINDING: &str = "ctrl-shift-s";
pub(crate) const SERIAL_CONSOLE_KEYBINDING: &str = "ctrl-shift-d";

pub(crate) fn pane_output_keybinding() -> KeyBinding {
    KeyBinding::new(
        SAVE_PANE_OUTPUT_KEYBINDING,
        SavePaneOutput,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn close_pane_keybinding() -> KeyBinding {
    KeyBinding::new(CLOSE_PANE_KEYBINDING, ClosePane, Some("Zetta > Terminal"))
}

pub(crate) fn serial_console_keybinding() -> KeyBinding {
    KeyBinding::new(
        SERIAL_CONSOLE_KEYBINDING,
        ToggleSerialConsole,
        Some("Zetta > Terminal"),
    )
}

pub(crate) fn minimized_pane_keybindings() -> [KeyBinding; 4] {
    [
        KeyBinding::new("alt-shift-down", MinimizePane, Some("Zetta > Terminal")),
        KeyBinding::new(
            "alt-shift-up",
            RestoreMinimizedPane,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "alt-shift-left",
            SelectPreviousMinimizedPane,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "alt-shift-right",
            SelectNextMinimizedPane,
            Some("Zetta > Terminal"),
        ),
    ]
}

pub(crate) fn scrollback_page_keybindings() -> [KeyBinding; 2] {
    const NORMAL_TERMINAL_SCREEN: &str = "Zetta > Terminal && screen == normal";
    [
        KeyBinding::new("pageup", ScrollPageUp, Some(NORMAL_TERMINAL_SCREEN)),
        KeyBinding::new("pagedown", ScrollPageDown, Some(NORMAL_TERMINAL_SCREEN)),
    ]
}

pub(crate) fn load_keybindings(path: &PathBuf, profile_count: usize, cx: &mut App) {
    cx.clear_key_bindings();
    match KeymapFile::load_asset_allow_partial_failure(settings::DEFAULT_KEYMAP_PATH, cx) {
        Ok(bindings) => cx.bind_keys(bindings),
        Err(error) => eprintln!("Could not load the default terminal keymap: {error:#}"),
    }
    let mut bindings = vec![
        KeyBinding::new("ctrl-shift-t", NewTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-n", NewWindow, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-w", CloseTab, Some("Zetta > Terminal")),
        close_pane_keybinding(),
        KeyBinding::new("ctrl-shift-o", SplitHorizontal, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-e", SplitVertical, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-a", SelectAll, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-backspace",
            ClearClipboard,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("alt-left", FocusPaneLeft, Some("Zetta > Terminal")),
        KeyBinding::new("alt-right", FocusPaneRight, Some("Zetta > Terminal")),
        KeyBinding::new("alt-up", FocusPaneUp, Some("Zetta > Terminal")),
        KeyBinding::new("alt-down", FocusPaneDown, Some("Zetta > Terminal")),
        KeyBinding::new("shift-escape", ToggleMaximizePane, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-i",
            ToggleBroadcastInput,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("ctrl-shift-m", ToggleMultiCommand, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-tab", NextTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-pageup", NextTab, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-pagedown", PreviousTab, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-c",
            CopyAndClearSelection,
            Some("Zetta > Terminal && selection"),
        ),
        KeyBinding::new("ctrl-v", Paste, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-shift-f", SearchScrollback, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-alt-f", SearchTabScrollback, Some("Zetta > Terminal")),
        KeyBinding::new(
            "enter",
            SearchNextMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "shift-enter",
            SearchPreviousMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "f3",
            SearchNextMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "shift-f3",
            SearchPreviousMatch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "escape",
            DismissSearch,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new(
            "ctrl-a",
            SelectAllSearchText,
            Some("Zetta > Terminal && scrollback_search"),
        ),
        KeyBinding::new("ctrl-alt-v", PasteTrimmed, Some("Zetta > Terminal")),
        pane_output_keybinding(),
        KeyBinding::new(
            "ctrl-shift-p",
            ToggleCommandPalette,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new("ctrl-,", ToggleSettings, Some("Zetta > Terminal")),
        serial_console_keybinding(),
        KeyBinding::new(RENAME_TAB_KEYBINDING, RenameTab, Some("Zetta > Terminal")),
        KeyBinding::new(RENAME_PANE_KEYBINDING, RenamePane, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-=", IncreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-+", IncreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl--", DecreaseTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-0", ResetTerminalFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-alt-=", IncreasePaneFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-alt-+", IncreasePaneFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-alt--", DecreasePaneFontSize, Some("Zetta > Terminal")),
        KeyBinding::new("ctrl-alt-0", ResetPaneFontSize, Some("Zetta > Terminal")),
        KeyBinding::new(
            "ctrl-shift-r",
            ReloadConfiguration,
            Some("Zetta > Terminal"),
        ),
        KeyBinding::new(
            "ctrl-shift-f12",
            TogglePerformanceOverlay,
            Some("Zetta > Terminal"),
        ),
        // Override Zed's inherited `pane::CloseActiveItem` binding in terminal focus.
        KeyBinding::new("ctrl-shift-w", CloseTab, Some("Terminal")),
    ];
    bindings.extend(minimized_pane_keybindings());
    bindings.extend(scrollback_page_keybindings());
    bindings.extend(pane_template_keybindings());
    let keyboard_mapper = cx.keyboard_mapper().clone();
    bindings.extend(
        (1..=profile_count.min(9))
            .flat_map(|slot| profile_keybindings(slot, keyboard_mapper.as_ref())),
    );
    cx.bind_keys(bindings);
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let content = normalize_keymap_key_names(&content);
    match KeymapFile::load(&content, cx) {
        KeymapFileLoadResult::Success { key_bindings } => cx.bind_keys(key_bindings),
        KeymapFileLoadResult::SomeFailedToLoad {
            key_bindings,
            error_message,
        } => {
            eprintln!(
                "Some key bindings in {} were ignored: {error_message}",
                path.display()
            );
            cx.bind_keys(key_bindings);
        }
        KeymapFileLoadResult::JsonParseFailure { error } => {
            eprintln!("Could not load {}: {error:#}", path.display());
        }
    }
}

pub(crate) fn open_zetta_window(
    config: Config,
    configuration_error: Option<String>,
    enable_performance_overlay: bool,
    performance_report: Option<(PerformanceReportOptions, PerformanceReportStatus)>,
    profile_pane_stress: bool,
    cx: &mut App,
) -> Result<()> {
    let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(520.), px(320.))),
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
        },
        move |window, cx| {
            window.set_window_title("Zetta");
            let zetta = cx.new(|cx| Zetta::new(config, configuration_error, window, cx));
            if profile_pane_stress {
                zetta.update(cx, |zetta, cx| zetta.configure_pane_profile_stress(cx));
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
        },
    )
    .context("opening Zetta window")?;
    cx.activate(true);
    Ok(())
}

fn terminal_rendering_profile_config(executable: &Path) -> Config {
    let mut config = Config::defaults(None, None);
    config.profiles = vec![Profile {
        name: "Terminal rendering profiler".to_owned(),
        command: Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec!["--terminal-render-workload".to_owned()],
            title_override: Some("Terminal rendering profiler".to_owned()),
        },
        theme: None,
    }];
    config.default_profile = 0;
    config
}

fn run_terminal_rendering_workload() -> Result<()> {
    const FRAME_INTERVAL: Duration = Duration::from_nanos(4_166_667);
    const ROW: &str = "0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ │─╭╮╰╯ ✓ rendered cell workload";

    let stdout = std::io::stdout();
    let mut output = std::io::BufWriter::new(stdout.lock());
    output.write_all(b"\x1b[2J\x1b[?25l")?;
    let mut frame = 0_u64;
    let mut next_frame = Instant::now();
    loop {
        if write!(
            output,
            "\x1b[H\x1b[1;36mZetta terminal rendering profiler\x1b[0m\r\n\
             240 Hz producer · frame {frame:010}\r\n\
             This deterministic workload is identical on Linux, macOS, and Windows.\r\n\r\n"
        )
        .is_err()
        {
            return Ok(());
        }
        for row in 0..34 {
            writeln!(output, "{row:02} {ROW} {frame:010}\r")?;
        }
        output.flush()?;
        frame = frame.wrapping_add(1);

        next_frame += FRAME_INTERVAL;
        let now = Instant::now();
        if next_frame > now {
            std::thread::sleep(next_frame - now);
        } else {
            next_frame = now;
        }
    }
}

pub(crate) fn run() -> Result<()> {
    let args = parse_args()?;
    if let Some(command) = &args.tftp_command {
        return command.run();
    }
    if args.mode == StartupMode::TerminalRenderingWorkload {
        return run_terminal_rendering_workload();
    }

    let profiling = args.mode == StartupMode::TerminalRenderingProfile;
    let report_options = args
        .profile_report
        .zip(args.profile_duration)
        .map(|(path, duration)| PerformanceReportOptions { path, duration });
    let report_requested = report_options.is_some();
    let report_status = Arc::new(Mutex::new(None));
    let (config, configuration_error) = if profiling {
        (
            terminal_rendering_profile_config(&env::current_exe()?),
            None,
        )
    } else {
        load_startup_config(args.config_path.as_deref(), args.keymap_path)
    };
    let keymap_path = config.keymap_path.clone();
    let profile_count = config.profiles.len();
    let http_client = Arc::new(
        reqwest_client::ReqwestClient::user_agent(concat!("Zetta/", env!("CARGO_PKG_VERSION")))
            .context("initializing HTTP client")?,
    );
    let report_status_for_app = report_status.clone();
    gpui_platform::application()
        .with_assets(ZettaAssets)
        .run(move |cx: &mut App| {
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
            let layout_keymap_path = keymap_path.clone();
            cx.on_keyboard_layout_change(move |cx| {
                load_keybindings(&layout_keymap_path, profile_count, cx);
            })
            .detach();
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            open_zetta_window(
                config,
                configuration_error,
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

#[cfg(test)]
#[path = "tests/startup.rs"]
mod tests;
