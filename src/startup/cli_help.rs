use super::*;

#[cfg(not(feature = "tftp-client"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TftpCommand;

#[cfg(not(feature = "tftp-client"))]
impl TftpCommand {
    pub(crate) fn run(&self) -> Result<()> {
        anyhow::bail!("TFTP support is disabled in this build")
    }
}

#[cfg(not(feature = "tftp-client"))]
pub(crate) fn tftp_help() -> &'static str {
    #[cfg(feature = "tftp-server")]
    {
        "Zetta TFTP server\n\nUsage: zetta tftp server [OPTIONS]\n\nRun `zetta tftp server --help` for server options."
    }
    #[cfg(not(feature = "tftp-server"))]
    {
        "TFTP support is disabled in this build"
    }
}

#[cfg(not(feature = "tftp-client"))]
pub(crate) fn parse_tftp_args(_: impl IntoIterator<Item = OsString>) -> Result<TftpCommand> {
    anyhow::bail!("TFTP support is disabled in this build")
}

pub(crate) fn version_text() -> String {
    format!(
        "Zetta {}\nCONTROL_VERSION={}\nCATALOG_VERSION={}\nZMUX_PROTOCOL_VERSION={}",
        env!("CARGO_PKG_VERSION"),
        crate::process_control::CONTROL_VERSION,
        zmux::protocol::CATALOG_VERSION,
        zmux::messages::PROTOCOL_VERSION,
    )
}

pub(crate) fn format_help_table<'a>(rows: impl AsRef<[(&'a str, &'a str)]>) -> String {
    let rows = rows
        .as_ref()
        .iter()
        .map(|&(label, description)| (label.trim_end(), description))
        .collect::<Vec<_>>();
    let label_width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    rows.into_iter()
        .map(|(label, description)| {
            let mut lines = description.split('\n');
            let first_line = lines.next().unwrap_or("").trim_end();
            let mut formatted = String::new();
            formatted.push_str("  ");
            formatted.push_str(label);
            formatted.push_str(&" ".repeat(label_width - label.chars().count()));
            if !first_line.is_empty() {
                formatted.push_str("  ");
                formatted.push_str(first_line);
            }
            for line in lines {
                formatted.push('\n');
                let line = line.trim_end();
                if !line.is_empty() {
                    formatted.push_str("  ");
                    formatted.push_str(&" ".repeat(label_width));
                    formatted.push_str("  ");
                    formatted.push_str(line);
                }
            }
            formatted
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn help_text(profiles: &[Profile]) -> String {
    let features = [
        "Terminal emulator",
        #[cfg(feature = "syntax-highlighting")]
        "Vi syntax highlighting",
        #[cfg(all(feature = "wayland", linux_like))]
        "Wayland backend",
        #[cfg(all(feature = "x11", linux_like))]
        "X11 backend",
        #[cfg(feature = "serial-console")]
        "Serial console",
        #[cfg(feature = "http-server")]
        "HTTP server",
        #[cfg(feature = "tftp-server")]
        "TFTP server",
        #[cfg(feature = "tftp-client")]
        "TFTP client",
        #[cfg(feature = "notifications")]
        "Desktop notifications",
        #[cfg(feature = "clipboard")]
        "Clipboard access",
        #[cfg(feature = "session-persistence")]
        "Encrypted session retention",
        #[cfg(feature = "worktree")]
        "Git worktree workflow",
    ];

    let mut usage = vec![
        "zetta [OPTIONS]",
        "zetta benchmark [OPTIONS]",
        "zetta benchmark output [OPTIONS]",
        "zetta terminal-size [--json | --resize [--columns COLUMNS] [--rows ROWS]]",
        "zetta mux [COMMAND]",
        #[cfg(feature = "worktree")]
        "zetta wt <COMMAND>",
        "zetta mux reconnect SESSION_ID",
        "zetta mux resume SESSION [-i PATH]",
        "zetta splits",
        "zetta tabicon [OPTIONS] ICON",
        "zetta tabicon --list",
        "zetta theme pane [OPTIONS] THEME",
        "zetta theme tab [OPTIONS] THEME",
        "zetta theme pane --reset",
        "zetta theme tab --reset",
        "zetta theme pane --list",
        "zetta theme tab --list",
        "zetta overlay [OPTIONS] TEXT",
        "zetta overlay --reset",
        "zetta edit [OPTIONS] [--] FILE ...",
        "zetta vi [OPTIONS] [FILE ...]",
        "zetta pane [OPTIONS] -- COMMAND [ARGUMENT ...]",
        "zetta pane --list",
        "zetta profile <COMMAND>",
        "zetta project <COMMAND>",
        "zetta attention [OPTIONS] [SUMMARY] [BODY]",
        "zetta init [SHELL]",
    ];
    if cfg!(feature = "serial-console") {
        usage.push("zetta serial <COMMAND>");
    }
    if cfg!(feature = "http-server") {
        usage.push("zetta http server [OPTIONS]");
    }
    if cfg!(tftp_enabled) {
        usage.push("zetta tftp <COMMAND>");
    }
    if cfg!(feature = "notifications") {
        usage.push("zetta notify [OPTIONS] SUMMARY [BODY]");
    }
    if cfg!(notify_cleanup_enabled) {
        usage.push("zetta notify cleanup [OPTIONS]");
    }
    if cfg!(feature = "clipboard") {
        usage.extend(["zetta copy [OPTIONS]", "zetta paste [OPTIONS]"]);
    }

    let mut commands = vec![
        ("benchmark", "Profile terminal rendering"),
        (
            "benchmark output",
            "Write and time a text payload (default: 10 MiB)",
        ),
        ("terminal-size", "Print or resize the current terminal pane"),
        (
            "mux",
            "Control, list, reconnect, and resume background sessions",
        ),
        #[cfg(feature = "worktree")]
        ("wt", "Create and integrate Git worktrees"),
        ("splits", "List configured pane split templates"),
        ("tabicon", "Set the active tab's icon override"),
        (
            "theme",
            "Non-persistently change the active pane or tab's theme",
        ),
        ("overlay", "Non-persistently show text over the active pane"),
        ("edit", "Edit files with $EDITOR, falling back to Zetta vi"),
        ("vi", "Edit files with Zetta's built-in vi"),
        ("pane", "Run a command in an existing or new pane"),
        ("profile", "List and manage profiles"),
        ("attention", "Mark the originating tab as needing attention"),
        ("project", "List, add, remove, or open projects"),
        ("init", "Configure or generate shell integration"),
    ];
    if cfg!(feature = "serial-console") {
        commands.push(("serial", "List or connect to serial devices"));
    }
    if cfg!(feature = "http-server") {
        commands.push(("http server", "Serve static files over HTTP"));
    }
    if cfg!(tftp_enabled) {
        commands.push(("tftp", "Transfer files or serve them with TFTP"));
    }
    if cfg!(feature = "notifications") {
        commands.push(("notify", "Show a desktop notification"));
    }
    if cfg!(notify_cleanup_enabled) {
        commands.push((
            "notify cleanup",
            "Reap stale desktop notification worker processes",
        ));
    }
    if cfg!(feature = "clipboard") {
        commands.extend([
            ("copy", "Copy standard input to the clipboard"),
            ("paste", "Print the clipboard's contents"),
        ]);
    }
    let commands = format_help_table(commands);

    let options = format_help_table([
        ("-h, --help", "Print help"),
        ("-v, --version", "Print version and compatibility versions"),
        ("-c, --config PATH", "Use a configuration file"),
        ("-k, --keymap PATH", "Use a keymap file"),
        (
            "-p, --profile NAME",
            "Select one of the profiles listed above",
        ),
        (
            "-s, --split NAME",
            "Apply a configured pane split template; run `zetta splits` to list available names",
        ),
        (
            "-r, --replace-pane",
            "Replace the active pane in a running process; requires --split or --profile",
        ),
        (
            "-t, --theme NAME",
            "Non-persistently override --profile's theme for this launch",
        ),
        (
            "-n, --no-mux",
            "Keep background sessions in this process for this launch; sharing unavailable",
        ),
        (
            "-w, --new-window",
            "Open a fresh OS window without resuming a dormant session",
        ),
        (
            "-e, --command COMMAND [ARGUMENT ...]",
            "Open a tab and run COMMAND",
        ),
    ]);
    let profiles = profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<Vec<_>>()
        .join("\n  ");
    let usage = format!("Usage: {}", usage.join("\n       "));
    format!(
        "Zetta Terminal\n\n{usage}\n\nCommands:\n{commands}\n\nBuilt-in features:\n  {}\n\nProfiles accepted by --profile NAME (case-insensitive):\n  {profiles}\n\nOptions:\n{options}",
        features.join("\n  "),
    )
}

pub(crate) fn benchmark_output_help() -> String {
    format!(
        "Benchmark terminal output throughput\n\nUsage: zetta benchmark output [OPTIONS]\n\nWrites deterministic text to standard output and prints the elapsed time to standard error.\n\nOptions:\n{}",
        format_help_table([
            ("-s, --size MIB", "Set the output size in MiB [default: 10]",),
            (
                "-t, --output-type TYPE",
                "Select repeated or unique lines [default: repeated]",
            ),
            ("-h, --help", "Print help"),
        ])
    )
}

pub(crate) fn terminal_size_help() -> String {
    format!(
        "Print or resize the current terminal pane\n\nUsage: zetta terminal-size [--json | --resize [--columns COLUMNS] [--rows ROWS]]\n\nWithout --resize, prints the terminal width in columns and height in rows. With --resize, emits the xterm CSI 8 resize request for the current pane; an omitted dimension is kept unchanged.\n\nOptions:\n{}",
        format_help_table([
            ("-j, --json", "Print machine-readable JSON"),
            ("-r, --resize", "Resize the current pane"),
            ("-c, --columns COLUMNS", "Set the pane width in columns",),
            ("-R, --rows ROWS", "Set the pane height in rows"),
            ("-h, --help", "Print help"),
        ])
    )
}

pub(crate) fn edit_help() -> String {
    format!(
        "Edit files with the pane's configured editor\n\nUsage: zetta edit [OPTIONS] [--] FILE ...\n\nUses EDITOR from the current environment. If EDITOR is unset or empty, Zetta's built-in vi is used.\n\nOptions:\n{}",
        format_help_table([
            (
                "-d, --delete-after",
                "Delete a managed scrollback file after editing",
            ),
            ("-h, --help", "Print help"),
        ])
    )
}

pub(crate) fn benchmark_help() -> String {
    format!(
        "Benchmark terminal rendering\n\nUsage: zetta benchmark [OPTIONS]\n\nThe workload options select one producer pattern and cannot be combined with\neach other. Without one, the standard text and line-drawing workload runs.\n\nOptions:\n{}",
        format_help_table([
            (
                "-s, --profile-pane-stress",
                "Use four visible producer panes",
            ),
            (
                "-b, --profile-background-stress",
                "Render alternating cell backgrounds",
            ),
            (
                "-u, --profile-sparse-updates",
                "Update a dense terminal at 40 Hz",
            ),
            (
                "-a, --profile-alt-screen-scroll",
                "Scroll a colourised diff on the alternate screen",
            ),
            (
                "-x, --profile-external-terminal",
                "Run the workload in the current terminal",
            ),
            ("-r, --profile-report PATH", "Write a profiling report"),
            (
                "-d, --profile-duration SECONDS",
                "Set the profiling duration",
            ),
            ("-h, --help", "Print help"),
        ])
    )
}

pub(crate) fn is_version_argument(argument: &str) -> bool {
    matches!(argument, "--version" | "-v")
}

pub(crate) fn attention_help() -> String {
    let options = format_help_table([
        ("-n, --notify", "Also show a desktop notification"),
        (
            "-a, --app-name NAME",
            "Set the notification's application name",
        ),
        (
            "-i, --icon PATH",
            "Show an image with the notification (default: Zetta's icon)",
        ),
        (
            "-s, --sound NAME",
            "zetta-default, zetta-ok, zetta-alarm, zetta-gong, or a platform-specific system sound name",
        ),
        (
            "-t, --timeout WHEN",
            "default, never, or a number of milliseconds (default: default)",
        ),
        ("-h, --help", "Print help"),
    ]);
    format!(
        "Mark the originating Zetta tab as needing attention\n\nUsage: zetta attention [OPTIONS] [SUMMARY] [BODY]\n\nSUMMARY defaults to `Attention required`; BODY is optional additional text. The command must run inside a terminal launched by Zetta. The badge is cleared when that tab becomes active and genuinely focused.\n\nOptions:\n{options}\n\nNotification options require --notify. Without --notify, attention is an in-memory tab badge only."
    )
}

pub(crate) fn parse_terminal_resize_dimension(argument: &OsString, option: &str) -> Result<usize> {
    let value = argument
        .to_string_lossy()
        .parse::<usize>()
        .with_context(|| format!("{option} must be a positive whole number"))?;
    anyhow::ensure!(value > 0, "{option} must be greater than zero");
    anyhow::ensure!(
        value <= usize::from(u16::MAX),
        "{option} must not exceed {}",
        u16::MAX
    );
    Ok(value)
}

pub(crate) fn tab_icon_help() -> String {
    let options = format_help_table([
        (
            "-i, --icon NAME",
            "Set the icon by option instead of as a positional argument",
        ),
        ("-l, --list", "Print built-in icon names, including none"),
        ("-h, --help", "Print help"),
    ]);
    format!(
        "Set the active tab's per-tab icon override through the running Zetta process\n\nUsage: zetta tabicon [OPTIONS] ICON\n       zetta tabicon --list\n\nICON is a built-in icon name. Use none to explicitly hide the icon. The choice remains with the logical tab across project changes and background/shared-session handoffs, and is never written to user or project configuration. The icon list is fetched dynamically with --list.\n\nOptions:\n{options}"
    )
}

pub(crate) fn parse_tab_icon_args(args: &[OsString]) -> Result<StartupMode> {
    let mut icon_name = None;
    let mut list = false;
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", tab_icon_help());
                std::process::exit(0);
            }
            "--list" | "-l" => {
                anyhow::ensure!(!list, "--list may only be specified once");
                list = true;
            }
            "--icon" | "-i" => {
                anyhow::ensure!(icon_name.is_none(), "--icon may only be specified once");
                icon_name = Some(
                    arguments
                        .next()
                        .context("--icon requires an icon name")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown tabicon option {value:?}")
            }
            value => {
                anyhow::ensure!(icon_name.is_none(), "only one tab icon may be specified");
                icon_name = Some(value.to_owned());
            }
        }
    }
    if list {
        anyhow::ensure!(
            icon_name.is_none(),
            "--list cannot be combined with an icon name"
        );
        return Ok(StartupMode::ListTabIcons);
    }
    let icon_name = icon_name
        .context("zetta tabicon requires an icon name; run zetta tabicon --help for usage")?;
    let icon = if icon_name.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(parse_tab_icon_name(&icon_name).with_context(|| {
            format!("unknown tab icon {icon_name:?}; run zetta tabicon --list for available icons")
        })?)
    };
    Ok(StartupMode::SetTabIcon { icon })
}

pub(crate) fn theme_help(scope: Option<ThemeScope>) -> String {
    let usage = match scope {
        Some(scope) => format!(
            "Usage: zetta theme {} [OPTIONS] THEME\n       zetta theme {} --reset\n       zetta theme {} --list",
            scope.name(),
            scope.name(),
            scope.name()
        ),
        None => "Usage: zetta theme pane [OPTIONS] THEME\n       zetta theme tab [OPTIONS] THEME\n       zetta theme pane --reset\n       zetta theme tab --reset\n       zetta theme pane --list\n       zetta theme tab --list".to_owned(),
    };
    let target = scope
        .map(|scope| format!("active {}", scope.name()))
        .unwrap_or_else(|| "active pane or tab".to_owned());
    let options = format_help_table([
        (
            "-t, --theme NAME",
            "Set the theme by option instead of as a positional argument",
        ),
        (
            "-r, --reset",
            "Restore the configured theme (or the tab theme for a pane)",
        ),
        (
            "-l, --list",
            "Print the running process's registered theme names",
        ),
        ("-h, --help", "Print help"),
    ]);
    format!(
        "Non-persistently change the {target}'s theme through the running Zetta process\n\n{usage}\n\nTHEME is a theme name registered in the running Zetta process (built-in or user-installed). The theme list is fetched dynamically with --list. The session-scoped change is never written to configuration: it is preserved across backgrounding, reconnect, and encrypted disk resume, and is lost when the pane or tab closes or configuration reloads.\n\nOptions:\n{options}"
    )
}

pub(crate) fn pane_splits_help() -> String {
    format!(
        "List configured pane split templates\n\nUsage: zetta splits\n\nPrints one configured pane split template name per line. Pass a listed name to the root --split or -s option, or to --replace-pane --split when replacing the active pane in a running process.\n\nOptions:\n{}",
        format_help_table([("-h, --help", "Print help")]),
    )
}

pub(crate) fn pane_help() -> String {
    let options = format_help_table([
        (
            "-d, --direction DIRECTION",
            "Create a split to the left, right, up, or down of the active pane",
        ),
        (
            "-l, --label LABEL",
            "Assign a generated label to a newly created split pane",
        ),
        (
            "-p, --pane LABEL",
            "Target an existing pane by its exact, case-sensitive label",
        ),
        (
            "-o, --overlay TEXT",
            "Show TEXT over a newly created split pane",
        ),
        (
            "-S, --overlay-size SIZE",
            "Set overlay size: sm, base, lg, xl, 2xl, or 3xl",
        ),
        (
            "-O, --overlay-opacity PCT",
            "Set overlay opacity from 0 to 100",
        ),
        (
            "-c, --overlay-color COLOR",
            "Set overlay color by name or hex value",
        ),
        ("-s, --stack", "Run the command in a stacked task terminal"),
        ("-L, --list", "Print labels for panes in the active tab"),
        ("-h, --help", "Print help"),
    ]);
    format!(
        "Run a command in an existing or newly created pane\n\nUsage: zetta pane [OPTIONS] -- COMMAND [ARGUMENT ...]\n       zetta pane --list\n\nWithout --direction, the command is sent to the selected pane's existing base shell. With --stack, it runs in a task-backed stacked terminal. With --direction, a new split is created relative to the active pane; up and down split horizontally, while left and right split vertically. Commands are passed as exact argv values, and must follow --. An overlay can be shown on a newly created split with --overlay.\n\nOptions:\n{options}\n\nExamples:\n  zetta pane --direction right --label api --overlay API -- npm run dev\n  zetta pane --direction up --overlay TESTS --overlay-color cyan -- cargo test\n  zetta pane --pane api -- make test\n  zetta pane --pane api --stack -- tail -f server.log"
    )
}

pub(crate) fn parse_theme_args(scope: ThemeScope, args: &[OsString]) -> Result<StartupMode> {
    let mut theme_name = None;
    let mut reset = false;
    let mut list = false;
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", theme_help(Some(scope)));
                std::process::exit(0);
            }
            "--reset" | "-r" => {
                anyhow::ensure!(!reset, "--reset may only be specified once");
                reset = true;
            }
            "--list" | "-l" => {
                anyhow::ensure!(!list, "--list may only be specified once");
                list = true;
            }
            "--theme" | "-t" => {
                anyhow::ensure!(theme_name.is_none(), "--theme may only be specified once");
                theme_name = Some(
                    arguments
                        .next()
                        .context("--theme requires a theme name")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown theme option {value:?}")
            }
            value => {
                anyhow::ensure!(theme_name.is_none(), "only one theme may be specified");
                theme_name = Some(value.to_owned());
            }
        }
    }
    if list {
        anyhow::ensure!(
            theme_name.is_none() && !reset,
            "--list cannot be combined with a theme name or --reset"
        );
        return Ok(StartupMode::ListThemes);
    }
    if reset {
        anyhow::ensure!(
            theme_name.is_none(),
            "--reset cannot be combined with a theme name"
        );
        return Ok(StartupMode::SetTheme { scope, theme: None });
    }
    let theme_name = theme_name.with_context(|| {
        format!(
            "zetta theme {} requires a theme name; run zetta theme {} --help for usage",
            scope.name(),
            scope.name()
        )
    })?;
    Ok(StartupMode::SetTheme {
        scope,
        theme: Some(theme_name),
    })
}

pub(crate) fn overlay_help() -> String {
    let preset_names = OVERLAY_COLOR_PRESETS
        .iter()
        .map(|preset| preset.name)
        .collect::<Vec<_>>()
        .join(", ");
    let color_description = format!(
        "Set the text color as a named preset ({preset_names}) or an rgb, rgba, rrggbb, or rrggbbaa hex value (no leading #)"
    );
    let options = format_help_table([
        (
            "-t, --text TEXT",
            "Set the overlay text by option instead of as a positional argument",
        ),
        (
            "-s, --size SIZE",
            "Set the font size: sm, base, lg, xl (default), 2xl, or 3xl",
        ),
        (
            "-o, --opacity PERCENT",
            "Set the opacity as a percentage from 0 to 100 (default: 85)",
        ),
        ("-c, --color COLOR", color_description.as_str()),
        ("-r, --reset", "Clear the active pane's overlay"),
        ("-h, --help", "Print help"),
    ]);
    format!(
        "Non-persistently show text over the active pane's terminal content through the running Zetta process\n\nUsage: zetta overlay [OPTIONS] TEXT\n       zetta overlay --reset\n\nTEXT is free-form text, shown over the top-right corner of the active pane. The change is never written to the configuration file: it is lost when the pane closes or the configuration reloads.\n\nOptions:\n{options}"
    )
}

pub(crate) fn parse_overlay_args(args: &[OsString]) -> Result<StartupMode> {
    let mut text = None;
    let mut reset = false;
    let mut font_size = None;
    let mut opacity = None;
    let mut color = None;
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", overlay_help());
                std::process::exit(0);
            }
            "--reset" | "-r" => {
                anyhow::ensure!(!reset, "--reset may only be specified once");
                reset = true;
            }
            "--text" | "-t" => {
                anyhow::ensure!(text.is_none(), "--text may only be specified once");
                text = Some(
                    arguments
                        .next()
                        .context("--text requires overlay text")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--size" | "-s" => {
                anyhow::ensure!(font_size.is_none(), "--size may only be specified once");
                let value = arguments
                    .next()
                    .context("--size requires a font size")?
                    .to_string_lossy()
                    .into_owned();
                font_size = Some(OverlayFontSize::parse(&value).with_context(|| {
                    format!(
                        "unknown overlay size {value:?}; expected one of {}",
                        OverlayFontSize::CLI_NAMES.join(", ")
                    )
                })?);
            }
            "--opacity" | "-o" => {
                anyhow::ensure!(opacity.is_none(), "--opacity may only be specified once");
                let value = arguments
                    .next()
                    .context("--opacity requires a percentage from 0 to 100")?
                    .to_string_lossy()
                    .into_owned();
                let percent = value
                    .parse::<u8>()
                    .with_context(|| format!("--opacity {value:?} must be a whole number"))?;
                anyhow::ensure!(percent <= 100, "--opacity must be between 0 and 100");
                opacity = Some(percent);
            }
            "--color" | "-c" => {
                anyhow::ensure!(color.is_none(), "--color may only be specified once");
                let value = arguments
                    .next()
                    .context("--color requires a color name or hex color")?
                    .to_string_lossy()
                    .into_owned();
                anyhow::ensure!(
                    overlay_color_from_value(&value).is_some(),
                    "invalid overlay color {value:?}"
                );
                color = Some(value);
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown overlay option {value:?}")
            }
            value => {
                anyhow::ensure!(text.is_none(), "only one overlay text may be specified");
                text = Some(value.to_owned());
            }
        }
    }
    if reset {
        anyhow::ensure!(
            text.is_none() && font_size.is_none() && opacity.is_none() && color.is_none(),
            "--reset cannot be combined with overlay text or a style option"
        );
        return Ok(StartupMode::SetPaneOverlay(PaneOverlayRequest {
            text: None,
            font_size: None,
            opacity: None,
            color: None,
        }));
    }
    let text =
        text.context("zetta overlay requires overlay text; run zetta overlay --help for usage")?;
    Ok(StartupMode::SetPaneOverlay(PaneOverlayRequest {
        text: Some(text),
        font_size,
        opacity,
        color,
    }))
}

#[cfg(test)]
#[path = "../tests/startup/cli_help.rs"]
mod tests;
