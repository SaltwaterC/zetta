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
    format!("Zetta {}", env!("CARGO_PKG_VERSION"))
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
    ];

    let serial_usage = if cfg!(feature = "serial-console") {
        "\n       zetta serial <COMMAND>"
    } else {
        ""
    };
    let http_usage = if cfg!(feature = "http-server") {
        "\n       zetta http server [OPTIONS]"
    } else {
        ""
    };
    let tftp_usage = if cfg!(tftp_enabled) {
        "\n       zetta tftp <COMMAND>"
    } else {
        ""
    };
    let notify_usage = if cfg!(feature = "notifications") {
        "\n       zetta notify [OPTIONS] SUMMARY [BODY]"
    } else {
        ""
    };
    let clipboard_usage = if cfg!(feature = "clipboard") {
        "\n       zetta copy [OPTIONS]\n       zetta paste [OPTIONS]"
    } else {
        ""
    };
    let serial_command = if cfg!(feature = "serial-console") {
        "\n  serial                              List or connect to serial devices"
    } else {
        ""
    };
    let http_command = if cfg!(feature = "http-server") {
        "\n  http server                         Serve static files over HTTP"
    } else {
        ""
    };
    let tftp_command = if cfg!(tftp_enabled) {
        "\n  tftp                                Transfer files or serve them with TFTP"
    } else {
        ""
    };
    let notify_command = if cfg!(feature = "notifications") {
        "\n  notify                              Show a desktop notification"
    } else {
        ""
    };
    let clipboard_command = if cfg!(feature = "clipboard") {
        "\n  copy                                Copy standard input to the clipboard\n  paste                               Print the clipboard's contents"
    } else {
        ""
    };
    let profiles = profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<Vec<_>>()
        .join("\n  ");
    let help = format!(
        "Zetta Terminal\n\nUsage: zetta [OPTIONS]\n       zetta benchmark [OPTIONS]\n       zetta benchmark-output [OPTIONS]\n       zetta terminal-size [--json | --resize [--columns COLUMNS] [--rows ROWS]]\n       zetta sessions [--json]\n       zetta profile <COMMAND>\n       zetta init [SHELL]{serial_usage}{http_usage}{tftp_usage}{notify_usage}{clipboard_usage}\n\nCommands:\n  benchmark                           Profile terminal rendering\n  benchmark-output                    Write and time a text payload (default: 10 MiB)\n  terminal-size                       Print or resize the current terminal pane\n  sessions                            List detached background sessions\n  profile                             List and manage profiles\n  init                                Configure or generate shell integration{serial_command}{http_command}{tftp_command}{notify_command}{clipboard_command}\n\nBuilt-in features:\n  {}\n\nProfiles accepted by --profile NAME (case-insensitive):\n  {profiles}\n\nOptions:\n  -h, --help                          Print help\n  -v, --version                       Print version\n  -c, --config PATH                   Use a configuration file\n  -k, --keymap PATH                   Use a keymap file\n  -p, --profile NAME                  Select one of the profiles listed above\n  -s, --split NAME                    Apply a configured pane split template; run `zetta splits` to list available names\n  -t, --theme NAME                    Non-persistently override --profile's theme for this launch",
        features.join("\n  "),
    );
    help
    .replace(
        "  -s, --split NAME                    Apply a configured pane split template; run `zetta splits` to list available names\n  -t, --theme NAME                    Non-persistently override --profile's theme for this launch",
        "  -s, --split NAME                    Apply a configured pane split template; run `zetta splits` to list available names\n  -r, --replace-pane                  Replace the active pane in a running process; requires --split or --profile\n  -t, --theme NAME                    Non-persistently override --profile's theme for this launch",
    )
    .replace(
        "       zetta sessions [--json]",
        "       zetta sessions [--json]\n       zetta sessions reconnect SESSION_ID\n       zetta splits\n       zetta tabicon [OPTIONS] ICON\n       zetta tabicon --list\n       zetta panetheme [OPTIONS] THEME\n       zetta panetheme --reset\n       zetta panetheme --list\n       zetta overlay [OPTIONS] TEXT\n       zetta overlay --reset\n       zetta edit [OPTIONS] [--] FILE ...\n       zetta vi [OPTIONS] [FILE ...]",
    )
    .replace(
        "sessions                            List detached background sessions",
        "sessions                            List or reconnect detached background sessions\n  splits                              List configured pane split templates\n  tabicon                             Set the active tab icon\n  panetheme                           Non-persistently change the active pane's theme\n  overlay                             Non-persistently show text over the active pane\n  edit                                Edit files with $EDITOR, falling back to Zetta vi\n  vi                                  Edit files with Zetta's built-in vi",
    )
}

pub(crate) fn is_version_argument(argument: &str) -> bool {
    matches!(argument, "--version" | "-v")
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

pub(crate) fn tab_icon_help() -> &'static str {
    "Set the active tab icon through the running Zetta process\n\nUsage: zetta tabicon [OPTIONS] ICON\n       zetta tabicon --list\n\nICON is a built-in icon name. Use none to hide the icon. The icon list is fetched dynamically with --list.\n\nOptions:\n  -i, --icon NAME  Set the icon by option instead of as a positional argument\n  -l, --list       Print built-in icon names, including none\n  -h, --help       Print help"
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

pub(crate) fn pane_theme_help() -> &'static str {
    "Non-persistently change the active pane's theme through the running Zetta process\n\nUsage: zetta panetheme [OPTIONS] THEME\n       zetta panetheme --reset\n       zetta panetheme --list\n\nTHEME is a theme name registered in the running Zetta process (built-in or user-installed). The theme list is fetched dynamically with --list. The change is never written to the configuration file: it is lost when the pane closes or the configuration reloads.\n\nOptions:\n  -t, --theme NAME  Set the theme by option instead of as a positional argument\n  -r, --reset       Restore the active pane's profile-configured theme\n  -l, --list        Print the running process's registered theme names\n  -h, --help        Print help"
}

pub(crate) fn pane_splits_help() -> &'static str {
    "List configured pane split templates\n\nUsage: zetta splits\n\nPrints one configured pane split template name per line. Pass a listed name to the root --split or -s option, or to --replace-pane --split when replacing the active pane in a running process.\n\nOptions:\n  -h, --help  Print help"
}

pub(crate) fn parse_pane_theme_args(args: &[OsString]) -> Result<StartupMode> {
    let mut theme_name = None;
    let mut reset = false;
    let mut list = false;
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", pane_theme_help());
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
                anyhow::bail!("unknown panetheme option {value:?}")
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
        return Ok(StartupMode::ListPaneThemes);
    }
    if reset {
        anyhow::ensure!(
            theme_name.is_none(),
            "--reset cannot be combined with a theme name"
        );
        return Ok(StartupMode::SetPaneTheme { theme: None });
    }
    let theme_name = theme_name
        .context("zetta panetheme requires a theme name; run zetta panetheme --help for usage")?;
    Ok(StartupMode::SetPaneTheme {
        theme: Some(theme_name),
    })
}

pub(crate) fn overlay_help() -> String {
    let preset_names = OVERLAY_COLOR_PRESETS
        .iter()
        .map(|preset| preset.name)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Non-persistently show text over the active pane's terminal content through the running Zetta process\n\nUsage: zetta overlay [OPTIONS] TEXT\n       zetta overlay --reset\n\nTEXT is free-form text, shown over the top-right corner of the active pane. The change is never written to the configuration file: it is lost when the pane closes or the configuration reloads.\n\nOptions:\n  -t, --text TEXT        Set the overlay text by option instead of as a positional argument\n  -s, --size SIZE        Set the font size: sm, base, lg, xl (default), 2xl, or 3xl\n  -o, --opacity PERCENT  Set the opacity as a percentage from 0 to 100 (default: 85)\n  -c, --color COLOR      Set the text color as a named preset ({preset_names}) or an rgb, rgba, rrggbb, or rrggbbaa hex value (no leading #)\n  -r, --reset            Clear the active pane's overlay\n  -h, --help             Print help"
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
