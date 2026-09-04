//! One parser per `zetta` subcommand.
//!
//! Each takes the arguments *after* its own name and returns the launch it
//! describes, so `super::parse_subcommand`'s match stays a table of names. A
//! subcommand whose feature is disabled fails here with the reason rather than
//! falling through to the plain launch, which would report its name as an
//! unknown argument.

use super::*;

pub(crate) fn parse_pane_wait_args(args: &[OsString]) -> Result<PaneWaitCommand> {
    let mut dependencies = None;
    let mut allow_failure = false;
    let mut after_delimiter = false;
    let mut command = Vec::new();
    let arguments = args.iter();

    for argument in arguments {
        if after_delimiter {
            command.push(argument.to_string_lossy().into_owned());
            continue;
        }
        match argument.to_string_lossy().as_ref() {
            "--" => after_delimiter = true,
            "--help" | "-h" => {
                println!("{}", pane_help());
                std::process::exit(0);
            }
            "--allow-failure" | "-a" => {
                anyhow::ensure!(!allow_failure, "--allow-failure may only be specified once");
                allow_failure = true;
            }
            option if option.starts_with('-') => {
                anyhow::bail!("unknown pane wait option {option:?}")
            }
            value => {
                anyhow::ensure!(
                    dependencies.is_none(),
                    "pane wait accepts one comma-separated dependency list"
                );
                let mut parsed_dependencies = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for dependency in value.split(',') {
                    anyhow::ensure!(!dependency.is_empty(), "pane wait labels must not be empty");
                    anyhow::ensure!(
                        seen.insert(dependency),
                        "pane wait cannot list the same pane label more than once"
                    );
                    parsed_dependencies.push(dependency.to_owned());
                }
                anyhow::ensure!(
                    !parsed_dependencies.is_empty(),
                    "pane wait requires a pane label"
                );
                dependencies = Some(parsed_dependencies);
            }
        }
    }

    let dependencies = dependencies.context("zetta pane wait requires dependency labels")?;
    anyhow::ensure!(
        after_delimiter,
        "zetta pane wait requires -- before COMMAND"
    );
    anyhow::ensure!(
        !command.is_empty(),
        "zetta pane wait requires a command after --"
    );
    anyhow::ensure!(
        command.first().is_none_or(|program| !program.is_empty()),
        "zetta pane wait requires a non-empty command name"
    );
    Ok(PaneWaitCommand {
        dependencies,
        allow_failure,
        command,
    })
}

pub(crate) fn parse_attention_args(args: &[OsString]) -> Result<StartupArgs> {
    let mut notify = false;
    let mut app_name = None;
    let mut icon = None;
    let mut sound = None;
    let mut timeout = None;
    let mut positional = Vec::new();
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--notify" | "-n" => {
                anyhow::ensure!(!notify, "--notify may only be specified once");
                notify = true;
            }
            "--app-name" | "-a" => {
                anyhow::ensure!(app_name.is_none(), "--app-name may only be specified once");
                app_name = Some(
                    arguments
                        .next()
                        .context("--app-name requires a name")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--icon" | "-i" => {
                anyhow::ensure!(icon.is_none(), "--icon may only be specified once");
                icon = Some(
                    arguments
                        .next()
                        .context("--icon requires a path")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--sound" | "-s" => {
                anyhow::ensure!(sound.is_none(), "--sound may only be specified once");
                sound = Some(
                    arguments
                        .next()
                        .context("--sound requires a name")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--timeout" | "-t" => {
                anyhow::ensure!(timeout.is_none(), "--timeout may only be specified once");
                let value = arguments
                    .next()
                    .context("--timeout requires default, never, or a number of milliseconds")?
                    .to_string_lossy()
                    .into_owned();
                timeout = Some(parse_notification_timeout(&value)?);
            }
            "--help" | "-h" => anyhow::bail!("{}", attention_help()),
            option if option.starts_with('-') => {
                anyhow::bail!("unknown attention option {option:?}")
            }
            _ => positional.push(argument),
        }
    }

    anyhow::ensure!(
        (0..=2).contains(&positional.len()),
        "usage: zetta attention [OPTIONS] [SUMMARY] [BODY]; run `zetta attention --help` for details"
    );
    anyhow::ensure!(
        notify || (app_name.is_none() && icon.is_none() && sound.is_none() && timeout.is_none()),
        "--app-name, --icon, --sound, and --timeout require --notify"
    );
    if notify && !cfg!(feature = "notifications") {
        anyhow::bail!("desktop notifications are disabled in this build")
    }

    let summary = positional.first().map_or_else(
        || "Attention required".to_owned(),
        |value| value.to_string_lossy().into_owned(),
    );
    anyhow::ensure!(!summary.is_empty(), "SUMMARY must not be empty");
    let body = positional
        .get(1)
        .map(|value| value.to_string_lossy().into_owned());

    Ok(StartupArgs::for_mode(StartupMode::Attention(
        AttentionCommand {
            notify,
            notification: NotificationRequest {
                summary,
                body,
                app_name,
                icon,
                sound,
                timeout,
            },
        },
    )))
}

pub(crate) fn parse_attention_target(process_id: &str, attention_id: &str) -> Result<(u32, u64)> {
    let process_id = process_id
        .parse::<u32>()
        .context("ZETTA_PROCESS_ID must be a positive process ID")?;
    anyhow::ensure!(
        process_id != 0,
        "ZETTA_PROCESS_ID must be a positive process ID"
    );
    let attention_id = attention_id
        .parse::<u64>()
        .context("ZETTA_ATTENTION_ID must be a positive attention ID")?;
    anyhow::ensure!(
        attention_id != 0,
        "ZETTA_ATTENTION_ID must be a positive attention ID"
    );
    Ok((process_id, attention_id))
}

pub(crate) fn parse_pane_args(args: &[OsString]) -> Result<PaneCommand> {
    let mut direction = None;
    let mut label = None;
    let mut pane = None;
    let mut overlay = None;
    let mut stack = false;
    let mut list = false;
    let mut command = Vec::new();
    let mut after_delimiter = false;
    let mut arguments = args.iter();

    while let Some(argument) = arguments.next() {
        let value = argument.to_string_lossy();
        if after_delimiter {
            command.push(value.into_owned());
            continue;
        }
        match value.as_ref() {
            "--" => after_delimiter = true,
            "--help" | "-h" => {
                println!("{}", pane_help());
                std::process::exit(0);
            }
            "--direction" | "-d" => {
                anyhow::ensure!(
                    direction.is_none(),
                    "--direction may only be specified once"
                );
                let value = arguments
                    .next()
                    .context("--direction requires left, right, up, or down")?
                    .to_string_lossy()
                    .into_owned();
                direction = Some(parse_pane_direction(&value).with_context(|| {
                    format!("unknown pane direction {value:?}; use left, right, up, or down")
                })?);
            }
            "--label" | "-l" => {
                anyhow::ensure!(label.is_none(), "--label may only be specified once");
                let value = arguments
                    .next()
                    .context("--label requires a non-empty pane label")?
                    .to_string_lossy()
                    .into_owned();
                anyhow::ensure!(!value.is_empty(), "--label requires a non-empty pane label");
                label = Some(value);
            }
            "--pane" | "-p" => {
                anyhow::ensure!(pane.is_none(), "--pane may only be specified once");
                let value = arguments
                    .next()
                    .context("--pane requires an existing pane label")?
                    .to_string_lossy()
                    .into_owned();
                anyhow::ensure!(!value.is_empty(), "--pane requires a non-empty pane label");
                pane = Some(value);
            }
            "--overlay" | "-o" | "--overlay-size" | "-S" | "--overlay-opacity" | "-O"
            | "--overlay-color" | "-c" => {
                parse_pane_overlay_arg(&value, &mut overlay, &mut arguments)?;
            }
            "--stack" | "-s" => {
                anyhow::ensure!(!stack, "--stack may only be specified once");
                stack = true;
            }
            "--list" | "-L" => {
                anyhow::ensure!(!list, "--list may only be specified once");
                list = true;
            }
            option if option.starts_with('-') => anyhow::bail!("unknown pane option {option:?}"),
            positional => anyhow::bail!(
                "pane command arguments must follow --; found positional argument {positional:?}"
            ),
        }
    }

    if list {
        anyhow::ensure!(
            direction.is_none(),
            "--list cannot be combined with --direction"
        );
        anyhow::ensure!(label.is_none(), "--list cannot be combined with --label");
        anyhow::ensure!(pane.is_none(), "--list cannot be combined with --pane");
        anyhow::ensure!(!stack, "--list cannot be combined with --stack");
        anyhow::ensure!(
            !after_delimiter && command.is_empty(),
            "--list cannot be combined with a command"
        );
    } else {
        anyhow::ensure!(after_delimiter, "pane commands require arguments after --");
        anyhow::ensure!(
            !command.is_empty(),
            "pane commands require a command after --"
        );
        anyhow::ensure!(
            pane_command_byte_len(&command) <= MAX_PANE_COMMAND_BYTES,
            "pane command exceeds the {} KiB limit",
            MAX_PANE_COMMAND_BYTES / 1024
        );
        anyhow::ensure!(
            label.is_none() || direction.is_some(),
            "--label requires --direction"
        );
        anyhow::ensure!(
            direction.is_none() || (pane.is_none() && !stack),
            "--direction cannot be combined with --pane or --stack"
        );
        anyhow::ensure!(
            overlay.is_none() || direction.is_some(),
            "--overlay options require --direction"
        );
        anyhow::ensure!(
            overlay
                .as_ref()
                .is_none_or(|overlay| overlay.text.is_some()),
            "an overlay requires --overlay TEXT"
        );
    }

    Ok(PaneCommand {
        direction,
        label,
        pane,
        overlay,
        stack,
        list,
        command,
    })
}

/// Parses a full command line into the launch it describes.
///
/// `zetta pane [wait] …`
pub(super) fn parse_pane_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    let mode = if arguments.first().is_some_and(|argument| argument == "wait") {
        StartupMode::PaneWait(parse_pane_wait_args(&arguments[1..])?)
    } else {
        StartupMode::Pane(parse_pane_args(arguments)?)
    };
    Ok(StartupArgs::for_mode(mode))
}

/// `zetta theme pane|tab …`
pub(super) fn parse_theme_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    if arguments
        .first()
        .is_some_and(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        println!("{}", theme_help(None));
        std::process::exit(0);
    }
    let scope = match arguments.first().map(|scope| scope.to_string_lossy()) {
        Some(scope) if scope == "pane" => ThemeScope::Pane,
        Some(scope) if scope == "tab" => ThemeScope::Tab,
        Some(scope) => anyhow::bail!("unknown theme scope {scope:?}; expected pane or tab"),
        None => anyhow::bail!(
            "zetta theme requires a scope (pane or tab); run zetta theme --help for usage"
        ),
    };
    Ok(StartupArgs::for_mode(parse_theme_args(
        scope,
        &arguments[1..],
    )?))
}

/// `zetta splits`
pub(super) fn parse_splits_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    if arguments
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        println!("{}", pane_splits_help());
        std::process::exit(0);
    }
    if let Some(argument) = arguments.first() {
        anyhow::bail!("unknown splits argument {argument:?}");
    }
    Ok(StartupArgs::for_mode(StartupMode::ListPaneSplits))
}

/// `zetta attention …`
pub(super) fn parse_attention_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    if arguments
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        println!("{}", attention_help());
        std::process::exit(0);
    }
    parse_attention_args(arguments)
}

/// `zetta terminal-size …`
pub(super) fn parse_terminal_size_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    let mut json = false;
    let mut resize = false;
    let mut columns = None;
    let mut rows = None;
    let mut terminal_size_arguments = arguments.iter();
    while let Some(argument) = terminal_size_arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--json" | "-j" => json = true,
            "--resize" | "-r" => resize = true,
            "--columns" | "-c" => {
                anyhow::ensure!(columns.is_none(), "--columns may only be specified once");
                columns = Some(parse_terminal_resize_dimension(
                    terminal_size_arguments
                        .next()
                        .context("--columns requires a positive whole number")?,
                    "--columns",
                )?);
            }
            "--rows" | "-R" => {
                anyhow::ensure!(rows.is_none(), "--rows may only be specified once");
                rows = Some(parse_terminal_resize_dimension(
                    terminal_size_arguments
                        .next()
                        .context("--rows requires a positive whole number")?,
                    "--rows",
                )?);
            }
            "--help" | "-h" => {
                println!("{}", terminal_size_help());
                std::process::exit(0);
            }
            unknown => anyhow::bail!("unknown terminal-size argument {unknown:?}"),
        }
    }
    anyhow::ensure!(!json || !resize, "--json cannot be used with --resize");
    anyhow::ensure!(
        resize || (columns.is_none() && rows.is_none()),
        "--columns and --rows require --resize"
    );
    anyhow::ensure!(
        !resize || columns.is_some() || rows.is_some(),
        "--resize requires --columns and/or --rows"
    );
    Ok(StartupArgs::for_mode(StartupMode::PrintTerminalSize {
        json,
        resize: resize.then_some(TerminalResize { columns, rows }),
    }))
}

/// `zetta edit …`
pub(super) fn parse_edit_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    let mut delete_after = false;
    let mut paths = Vec::new();
    let mut options = true;
    for argument in arguments {
        match argument.to_string_lossy().as_ref() {
            "--" if options => options = false,
            "--help" | "-h" if options => {
                println!("{}", edit_help());
                std::process::exit(0);
            }
            "--delete-after" | "-d" if options => {
                anyhow::ensure!(!delete_after, "--delete-after may only be specified once");
                delete_after = true;
            }
            option if options && option.starts_with('-') => {
                anyhow::bail!("unknown edit option {option:?}")
            }
            _ => paths.push(argument.to_string_lossy().into_owned()),
        }
    }
    anyhow::ensure!(!paths.is_empty(), "zetta edit requires at least one file");
    anyhow::ensure!(
        !delete_after || paths.len() == 1,
        "--delete-after requires exactly one managed scrollback file"
    );
    Ok(StartupArgs::for_mode(StartupMode::Edit {
        arguments: paths,
        delete_after,
    }))
}

/// `zetta init [SHELL]`
pub(super) fn parse_shell_integration_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    if arguments
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        println!("{}", shell_integration_help());
        std::process::exit(0);
    }
    anyhow::ensure!(
        arguments.len() <= 1,
        "usage: zetta init [SHELL]; run `zetta init --help` for supported shells"
    );
    let Some(shell) = arguments.first() else {
        return Ok(StartupArgs::for_mode(
            StartupMode::ConfigureCurrentShellIntegration,
        ));
    };
    let shell = shell.to_str().context("SHELL must be valid UTF-8")?;
    Ok(StartupArgs::for_mode(StartupMode::PrintShellIntegration(
        ShellIntegration::parse(shell)?,
    )))
}

/// `zetta serial …`
pub(super) fn parse_serial_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    #[cfg(feature = "serial-console")]
    {
        if arguments
            .iter()
            .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
        {
            println!("{}", serial_help());
            std::process::exit(0);
        }
        Ok(StartupArgs::for_mode(StartupMode::CliService(
            parse_serial_args(arguments.iter().cloned())?,
        )))
    }
    #[cfg(not(feature = "serial-console"))]
    {
        let _ = arguments;
        anyhow::bail!("Serial console support is disabled in this build")
    }
}

/// `zetta http …`
pub(super) fn parse_http_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    #[cfg(feature = "http-server")]
    {
        if arguments
            .iter()
            .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
        {
            println!("{}", http_server_help());
            std::process::exit(0);
        }
        Ok(StartupArgs::for_mode(StartupMode::CliService(
            parse_http_args(arguments.iter().cloned())?,
        )))
    }
    #[cfg(not(feature = "http-server"))]
    {
        let _ = arguments;
        anyhow::bail!("HTTP server support is disabled in this build")
    }
}

/// `zetta tftp server …`, and the `get`/`put` transfers a window runs.
pub(super) fn parse_tftp_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    if arguments
        .first()
        .is_some_and(|argument| argument == "server")
    {
        #[cfg(feature = "tftp-server")]
        {
            let server_arguments = &arguments[1..];
            if server_arguments
                .iter()
                .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
            {
                println!("{}", tftp_server_help());
                std::process::exit(0);
            }
            return Ok(StartupArgs::for_mode(StartupMode::CliService(
                parse_tftp_server_args(server_arguments.iter().cloned())?,
            )));
        }
        #[cfg(not(feature = "tftp-server"))]
        anyhow::bail!("TFTP server support is disabled in this build");
    }
    if arguments
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        println!("{}", tftp_help());
        std::process::exit(0);
    }
    // A transfer opens a window to run in, so it rides along with an ordinary
    // application launch rather than being a mode of its own.
    Ok(StartupArgs {
        tftp_command: Some(parse_tftp_args(arguments.iter().cloned())?),
        ..StartupArgs::for_mode(StartupMode::Application)
    })
}

/// `zetta notify [cleanup] …`
pub(super) fn parse_notify_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    if arguments
        .first()
        .is_some_and(|argument| argument == "cleanup")
    {
        #[cfg(notify_cleanup_enabled)]
        {
            let cleanup_arguments = &arguments[1..];
            if cleanup_arguments
                .iter()
                .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
            {
                println!("{}", notify_cleanup_help());
                std::process::exit(0);
            }
            return Ok(StartupArgs::for_mode(StartupMode::CliService(
                parse_notify_cleanup_args(cleanup_arguments.iter().cloned())?,
            )));
        }
        #[cfg(not(notify_cleanup_enabled))]
        anyhow::bail!(
            "zetta notify cleanup requires desktop notifications and is only needed on Linux and BSD"
        );
    }
    #[cfg(feature = "notifications")]
    {
        if arguments
            .iter()
            .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
        {
            println!("{}", notify_help());
            std::process::exit(0);
        }
        Ok(StartupArgs::for_mode(StartupMode::CliService(
            parse_notify_args(arguments.iter().cloned())?,
        )))
    }
    #[cfg(not(feature = "notifications"))]
    anyhow::bail!("Desktop notification support is disabled in this build")
}

/// `zetta copy …`
pub(super) fn parse_copy_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    #[cfg(feature = "clipboard")]
    {
        if arguments.iter().any(|argument| {
            matches!(
                argument.to_string_lossy().as_ref(),
                "--help" | "-h" | "-help"
            )
        }) {
            println!("{}", copy_help());
            std::process::exit(0);
        }
        Ok(StartupArgs::for_mode(StartupMode::CliService(
            parse_copy_args(arguments.iter().cloned())?,
        )))
    }
    #[cfg(not(feature = "clipboard"))]
    {
        let _ = arguments;
        anyhow::bail!("Clipboard support is disabled in this build")
    }
}

/// `zetta paste …`
pub(super) fn parse_paste_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    #[cfg(feature = "clipboard")]
    {
        if arguments.iter().any(|argument| {
            matches!(
                argument.to_string_lossy().as_ref(),
                "--help" | "-h" | "-help"
            )
        }) {
            println!("{}", paste_help());
            std::process::exit(0);
        }
        Ok(StartupArgs::for_mode(StartupMode::CliService(
            parse_paste_args(arguments.iter().cloned())?,
        )))
    }
    #[cfg(not(feature = "clipboard"))]
    {
        let _ = arguments;
        anyhow::bail!("Clipboard support is disabled in this build")
    }
}

#[cfg(test)]
#[path = "../../tests/startup/arg_parsing/subcommands.rs"]
mod tests;

/// The four `--overlay*` options, which all fill in one
/// [`PaneOverlayRequest`].
///
/// Together rather than as four arms of the caller's match: each of them starts
/// by materialising the same empty request, and each has to refuse a second
/// spelling of itself.
fn parse_pane_overlay_arg(
    option: &str,
    overlay: &mut Option<PaneOverlayRequest>,
    arguments: &mut std::slice::Iter<'_, OsString>,
) -> Result<()> {
    let overlay = overlay.get_or_insert(PaneOverlayRequest {
        text: None,
        font_size: None,
        opacity: None,
        color: None,
    });
    match option {
        "--overlay" | "-o" => {
            anyhow::ensure!(
                overlay.text.is_none(),
                "--overlay may only be specified once"
            );
            overlay.text = Some(
                arguments
                    .next()
                    .context("--overlay requires overlay text")?
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        "--overlay-size" | "-S" => {
            anyhow::ensure!(
                overlay.font_size.is_none(),
                "--overlay-size may only be specified once"
            );
            let value = arguments
                .next()
                .context("--overlay-size requires sm, base, lg, xl, 2xl, or 3xl")?
                .to_string_lossy()
                .into_owned();
            overlay.font_size = Some(OverlayFontSize::parse(&value).with_context(|| {
                format!(
                    "unknown overlay size {value:?}; expected one of {}",
                    OverlayFontSize::CLI_NAMES.join(", ")
                )
            })?);
        }
        "--overlay-opacity" | "-O" => {
            anyhow::ensure!(
                overlay.opacity.is_none(),
                "--overlay-opacity may only be specified once"
            );
            let value = arguments
                .next()
                .context("--overlay-opacity requires a percentage from 0 to 100")?
                .to_string_lossy()
                .into_owned();
            let percent = value
                .parse::<u8>()
                .with_context(|| format!("--overlay-opacity {value:?} must be a whole number"))?;
            anyhow::ensure!(
                percent <= 100,
                "--overlay-opacity must be between 0 and 100"
            );
            overlay.opacity = Some(percent);
        }
        "--overlay-color" | "-c" => {
            anyhow::ensure!(
                overlay.color.is_none(),
                "--overlay-color may only be specified once"
            );
            let value = arguments
                .next()
                .context("--overlay-color requires a color name or hex color")?
                .to_string_lossy()
                .into_owned();
            anyhow::ensure!(
                overlay_color_from_value(&value).is_some(),
                "invalid overlay color {value:?}"
            );
            overlay.color = Some(value);
        }
        _ => unreachable!("parse_pane_args routes only overlay options here"),
    }
    Ok(())
}
