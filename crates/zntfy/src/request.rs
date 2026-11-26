//! What a notification request is, and how a command line spells one.
//!
//! Zetta's own `zetta notify` parses the same options before handing the
//! request to this crate's executable. On Linux and Windows it links this
//! crate without the `backends` feature, which leaves only this module, so the
//! module depends on nothing but `std`, `anyhow` and `zetta_cli_help`, and
//! takes the program name and help text from its caller.
//! `zetta attention --notify` reads its notification options through
//! [`NotificationOptions`] for the same reason.

use std::ffi::{OsStr, OsString};

use anyhow::{Context as _, Result};
#[cfg(notify_cleanup_enabled)]
use zetta_cli_help::set_flag;
use zetta_cli_help::set_once;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotificationTimeout {
    #[default]
    Default,
    Never,
    Milliseconds(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationRequest {
    pub summary: String,
    pub body: Option<String>,
    pub app_name: Option<String>,
    pub icon: Option<String>,
    pub sound: Option<String>,
    pub timeout: Option<NotificationTimeout>,
}

/// What a notification carries besides its text, as every command that sends
/// one spells it: `--app-name`, `--icon`, `--sound` and `--timeout`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotificationOptions {
    pub app_name: Option<String>,
    pub icon: Option<String>,
    pub sound: Option<String>,
    pub timeout: Option<NotificationTimeout>,
}

impl NotificationOptions {
    /// Reads `option`, and its value from `arguments`, when it is one of the
    /// notification options, and says whether it was.
    pub fn parse<T: AsRef<OsStr>>(
        &mut self,
        option: &str,
        arguments: &mut impl Iterator<Item = T>,
    ) -> Result<bool> {
        let mut value = |option: &str, requirement: &str| {
            arguments
                .next()
                .map(|value| value.as_ref().to_string_lossy().into_owned())
                .with_context(|| format!("{option} requires {requirement}"))
        };
        match option {
            "--app-name" | "-a" => {
                set_once(&mut self.app_name, "--app-name", || {
                    value("--app-name", "a name")
                })?;
            }
            "--icon" | "-i" => {
                set_once(&mut self.icon, "--icon", || value("--icon", "a path"))?;
            }
            "--sound" | "-s" => {
                set_once(&mut self.sound, "--sound", || value("--sound", "a name"))?;
            }
            "--timeout" | "-t" => {
                set_once(&mut self.timeout, "--timeout", || {
                    let requirement = "default, never, or a number of milliseconds";
                    parse_notification_timeout(&value("--timeout", requirement)?)
                })?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Whether none of the options was given.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The request that shows `summary` and `body` with these options.
    pub fn request(self, summary: String, body: Option<String>) -> NotificationRequest {
        NotificationRequest {
            summary,
            body,
            app_name: self.app_name,
            icon: self.icon,
            sound: self.sound,
            timeout: self.timeout,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotificationTarget {
    pub process_id: u32,
    pub attention_id: u64,
}

#[cfg(notify_cleanup_enabled)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NotifyCleanupCommand {
    pub dry_run: bool,
}

pub fn parse_notification_timeout(value: &str) -> Result<NotificationTimeout> {
    match value {
        "default" => Ok(NotificationTimeout::Default),
        "never" => Ok(NotificationTimeout::Never),
        value => value
            .parse::<u32>()
            .map(NotificationTimeout::Milliseconds)
            .map_err(|_| {
                anyhow::anyhow!(
                    "--timeout must be default, never, or a whole number of milliseconds, got {value:?}"
                )
            }),
    }
}

pub fn parse_notification_target(
    process_id: &str,
    attention_id: &str,
) -> Option<NotificationTarget> {
    let process_id = process_id.parse::<u32>().ok().filter(|id| *id != 0)?;
    let attention_id = attention_id.parse::<u64>().ok().filter(|id| *id != 0)?;
    Some(NotificationTarget {
        process_id,
        attention_id,
    })
}

/// The tab a notification sent from inside a Zetta pane belongs to.
pub fn notification_target_from_environment() -> Option<NotificationTarget> {
    parse_notification_target(
        &std::env::var("ZETTA_PROCESS_ID").ok()?,
        &std::env::var("ZETTA_ATTENTION_ID").ok()?,
    )
}

/// Whether `--help` appears as an option: after `--` it is notification text.
pub fn requests_help(arguments: &[OsString]) -> bool {
    arguments
        .iter()
        .take_while(|argument| *argument != "--")
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
}

/// Parses `[OPTIONS] [--] SUMMARY [BODY]`. `program` names the command in the
/// usage error, and `help` is what `--help` reports.
pub fn parse_notify_args(
    args: impl IntoIterator<Item = OsString>,
    program: &str,
    help: impl Fn() -> String,
) -> Result<NotificationRequest> {
    let mut options = NotificationOptions::default();
    let mut positional = Vec::new();
    let mut arguments = args.into_iter();
    while let Some(argument) = arguments.next() {
        let text = argument.to_string_lossy().into_owned();
        if options.parse(&text, &mut arguments)? {
            continue;
        }
        match text.as_str() {
            "--help" | "-h" => anyhow::bail!("{}", help()),
            "--" => {
                positional.extend(&mut arguments);
                break;
            }
            option if option.starts_with('-') => anyhow::bail!("unknown notify option {option:?}"),
            _ => positional.push(argument),
        }
    }
    anyhow::ensure!(
        (1..=2).contains(&positional.len()),
        "usage: {program} [OPTIONS] [--] SUMMARY [BODY]; run `{program} --help` for details"
    );
    let summary = positional[0].to_string_lossy().into_owned();
    anyhow::ensure!(!summary.is_empty(), "SUMMARY must not be empty");
    let body = positional
        .get(1)
        .map(|value| value.to_string_lossy().into_owned());
    Ok(options.request(summary, body))
}

#[cfg(notify_cleanup_enabled)]
pub fn parse_notify_cleanup_args(
    args: impl IntoIterator<Item = OsString>,
    help: impl Fn() -> String,
) -> Result<NotifyCleanupCommand> {
    let mut dry_run = false;
    for argument in args {
        match argument.to_string_lossy().as_ref() {
            "--dry-run" | "-n" => set_flag(&mut dry_run, "--dry-run")?,
            "--help" | "-h" => anyhow::bail!("{}", help()),
            option => anyhow::bail!("unknown notify cleanup option {option:?}"),
        }
    }
    Ok(NotifyCleanupCommand { dry_run })
}

/// The arguments that re-enter notification-only startup in a child process.
/// In particular, an `attention --notify` request must not be replayed as
/// `attention`, or the child would route the badge a second time.
pub fn notification_reexec_args(command: &NotificationRequest) -> Vec<OsString> {
    let mut args = Vec::new();
    for (option, value) in [
        ("--app-name", command.app_name.as_deref()),
        ("--icon", command.icon.as_deref()),
        ("--sound", command.sound.as_deref()),
    ] {
        if let Some(value) = value {
            args.push(OsString::from(option));
            args.push(OsString::from(value));
        }
    }
    if let Some(timeout) = command.timeout {
        args.push(OsString::from("--timeout"));
        args.push(OsString::from(match timeout {
            NotificationTimeout::Default => "default".to_owned(),
            NotificationTimeout::Never => "never".to_owned(),
            NotificationTimeout::Milliseconds(milliseconds) => milliseconds.to_string(),
        }));
    }
    // The text is often a program's, so it must never be read as an option.
    args.push(OsString::from("--"));
    args.push(OsString::from(&command.summary));
    if let Some(body) = &command.body {
        args.push(OsString::from(body));
    }
    args
}
