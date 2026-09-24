//! `zetta notify` parsing and forwarding to the sibling `zntfy` executable.
//!
//! This module contains no notification backend. `zetta attention --notify`
//! also enters here after setting the tab badge, carrying its exact tab target
//! to the standalone process.

use std::{ffi::OsString, path::PathBuf, process::Command};

use anyhow::{Context as _, Result};

use super::{
    NotificationRequest, NotificationTarget, NotificationTimeout, parse_notification_timeout,
};
use crate::startup::format_help_table;

pub(crate) fn notify_help() -> String {
    format!(
        "Show a desktop notification\n\nUsage: zetta notify [OPTIONS] SUMMARY [BODY]\n\nSUMMARY is the notification's title; BODY is optional additional text.\n\nOptions:\n{}\n\nThe notification is delivered by the sibling `zntfy` executable. It uses D-Bus\non Linux and BSD, Notification Center on macOS, and toast notifications on\nWindows. Without --icon, Zetta's bundled icon is shown. --app-name has no\neffect on macOS and --timeout is ignored by some macOS notification centers.\n\n--sound zetta-default, zetta-ok, zetta-alarm, and zetta-gong are bundled tones.\nOther values are passed through as platform-specific system sound names.",
        format_help_table([
            (
                "-a, --app-name NAME",
                "Set the notification's application name"
            ),
            (
                "-i, --icon PATH",
                "Show an image from PATH (default: Zetta's icon)"
            ),
            (
                "-s, --sound NAME",
                "Built-in tone or platform-specific system sound name"
            ),
            (
                "-t, --timeout WHEN",
                "default, never, or milliseconds (default: default)"
            ),
            ("-h, --help", "Print help"),
        ])
    )
}

pub(crate) fn parse_notify_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<NotificationRequest> {
    let mut app_name = None;
    let mut icon = None;
    let mut sound = None;
    let mut timeout = None;
    let mut positional = Vec::new();
    let mut arguments = args.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
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
            "--help" | "-h" => anyhow::bail!("{}", notify_help()),
            option if option.starts_with('-') => anyhow::bail!("unknown notify option {option:?}"),
            _ => positional.push(argument),
        }
    }
    anyhow::ensure!(
        (1..=2).contains(&positional.len()),
        "usage: zetta notify [OPTIONS] SUMMARY [BODY]; run `zetta notify --help` for details"
    );
    let summary = positional[0].to_string_lossy().into_owned();
    anyhow::ensure!(!summary.is_empty(), "SUMMARY must not be empty");
    Ok(NotificationRequest {
        summary,
        body: positional
            .get(1)
            .map(|value| value.to_string_lossy().into_owned()),
        app_name,
        icon,
        sound,
        timeout,
    })
}

#[cfg(notify_cleanup_enabled)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NotifyCleanupCommand {
    pub(crate) dry_run: bool,
}

#[cfg(notify_cleanup_enabled)]
pub(crate) fn notify_cleanup_help() -> String {
    format!(
        "Reap stale desktop notification workers\n\nUsage: zetta notify cleanup [OPTIONS]\n\nA targeted notification starts a detached worker to wait for its click. Some\nnotification servers do not signal when a notification expires. This command\nfinds workers past their notification timeout and terminates them.\n\nOptions:\n{}",
        format_help_table([
            (
                "-n, --dry-run",
                "List stale workers without terminating them"
            ),
            ("-h, --help", "Print help"),
        ])
    )
}

#[cfg(notify_cleanup_enabled)]
pub(crate) fn parse_notify_cleanup_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<NotifyCleanupCommand> {
    let mut dry_run = false;
    for argument in args {
        match argument.to_string_lossy().as_ref() {
            "--dry-run" | "-n" => {
                anyhow::ensure!(!dry_run, "--dry-run may only be specified once");
                dry_run = true;
            }
            "--help" | "-h" => anyhow::bail!("{}", notify_cleanup_help()),
            option => anyhow::bail!("unknown notify cleanup option {option:?}"),
        }
    }
    Ok(NotifyCleanupCommand { dry_run })
}

pub(super) fn notification_target_from_environment() -> Option<NotificationTarget> {
    let process_id = std::env::var("ZETTA_PROCESS_ID")
        .ok()?
        .parse::<u32>()
        .ok()
        .filter(|id| *id != 0)?;
    let attention_id = std::env::var("ZETTA_ATTENTION_ID")
        .ok()?
        .parse::<u64>()
        .ok()
        .filter(|id| *id != 0)?;
    Some(NotificationTarget {
        process_id,
        attention_id,
    })
}

fn notification_helper_executable() -> Result<PathBuf> {
    Ok(std::env::current_exe()
        .context("locating the Zetta executable")?
        .with_file_name(format!("zntfy{}", std::env::consts::EXE_SUFFIX)))
}

pub(crate) fn notification_reexec_args(notification: &NotificationRequest) -> Vec<OsString> {
    let mut args = Vec::new();
    for (option, value) in [
        ("--app-name", notification.app_name.as_deref()),
        ("--icon", notification.icon.as_deref()),
        ("--sound", notification.sound.as_deref()),
    ] {
        if let Some(value) = value {
            args.push(OsString::from(option));
            args.push(OsString::from(value));
        }
    }
    if let Some(timeout) = notification.timeout {
        args.push(OsString::from("--timeout"));
        args.push(OsString::from(match timeout {
            NotificationTimeout::Default => "default".to_owned(),
            NotificationTimeout::Never => "never".to_owned(),
            NotificationTimeout::Milliseconds(milliseconds) => milliseconds.to_string(),
        }));
    }
    args.push(OsString::from(&notification.summary));
    if let Some(body) = &notification.body {
        args.push(OsString::from(body));
    }
    args
}

pub(crate) fn run_notification_proxy(
    notification: &NotificationRequest,
    target: Option<NotificationTarget>,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    if std::env::var_os("ZNTFY_INTERNAL_ZETTA_NOTIFICATION_HOST").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        let command = zntfy::parse_notify_args(notification_reexec_args(notification))?;
        return zntfy::run_macos_hosted_notification(
            &command,
            target.map(|target| zntfy::NotificationTarget {
                process_id: target.process_id,
                attention_id: target.attention_id,
            }),
        );
    }
    let executable = notification_helper_executable()?;
    let mut command = Command::new(&executable);
    command.args(notification_reexec_args(notification));
    if let Some(target) = target {
        command
            .env("ZETTA_PROCESS_ID", target.process_id.to_string())
            .env("ZETTA_ATTENTION_ID", target.attention_id.to_string());
    } else {
        command
            .env_remove("ZETTA_PROCESS_ID")
            .env_remove("ZETTA_ATTENTION_ID")
            .env(
                "ZNTFY_SILENT",
                u8::from(crate::silent_mode::system_silence_active_non_prompting()).to_string(),
            );
    }
    let status = command
        .status()
        .with_context(|| format!("starting notification helper {}", executable.display()))?;
    anyhow::ensure!(status.success(), "notification helper exited with {status}");
    Ok(())
}

#[cfg(notify_cleanup_enabled)]
pub(super) fn run_notification_cleanup_proxy(command: &NotifyCleanupCommand) -> Result<()> {
    let executable = notification_helper_executable()?;
    let mut child = Command::new(&executable);
    child.arg("cleanup");
    if command.dry_run {
        child.arg("--dry-run");
    }
    let status = child
        .status()
        .with_context(|| format!("starting notification helper {}", executable.display()))?;
    anyhow::ensure!(status.success(), "notification helper exited with {status}");
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_notification_target_for_response(
    tag: &str,
    action_id: Option<&str>,
) -> Option<NotificationTarget> {
    if action_id.is_some() {
        return None;
    }
    let mut parts = tag.split(':');
    (parts.next() == Some("zetta-target")).then_some(())?;
    let process_id = parts.next()?.parse::<u32>().ok().filter(|id| *id != 0)?;
    let attention_id = parts.next()?.parse::<u64>().ok().filter(|id| *id != 0)?;
    let mut suffix = parts.next()?.split('-');
    suffix.next()?.parse::<u32>().ok().filter(|id| *id != 0)?;
    suffix.next()?.parse::<u128>().ok()?;
    suffix.next()?.parse::<u64>().ok().filter(|id| *id != 0)?;
    (suffix.next().is_none() && parts.next().is_none() && process_id == std::process::id())
        .then_some(NotificationTarget {
            process_id,
            attention_id,
        })
}

#[cfg(test)]
#[path = "../tests/cli_services/notify_proxy.rs"]
mod tests;
