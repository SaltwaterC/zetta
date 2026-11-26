//! `zetta notify` parsing and forwarding to the sibling `zntfy` executable.
//!
//! This module contains no notification backend. `zetta attention --notify`
//! also enters here after setting the tab badge, carrying its exact tab target
//! to the standalone process.

use std::{ffi::OsString, path::PathBuf, process::Command};

use anyhow::{Context as _, Result};

use super::{NotificationRequest, NotificationTarget};
use crate::startup::format_help_table;
#[cfg(notify_cleanup_enabled)]
use zntfy::request::NotifyCleanupCommand;
use zntfy::request::{self, notification_reexec_args};

pub(crate) fn notify_help() -> String {
    format!(
        "Show a desktop notification\n\nUsage: zetta notify [OPTIONS] [--] SUMMARY [BODY]\n\nSUMMARY is the notification's title; BODY is optional additional text. Put\n`--` before them when either may begin with `-`; nothing after it is an option.\n\nOptions:\n{}\n\nThe notification is delivered by the sibling `zntfy` executable. It uses D-Bus\non Linux and BSD, Notification Center on macOS, and toast notifications on\nWindows. Without --icon, Zetta's bundled icon is shown. --app-name has no\neffect on macOS and --timeout is ignored by some macOS notification centers.\n\n--sound zetta-default, zetta-ok, zetta-alarm, and zetta-gong are bundled tones.\nOther values are passed through as platform-specific system sound names.",
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

/// Whether `--help` appears as an option: after `--` it is notification text.
pub(crate) fn requests_notify_help(arguments: &[OsString]) -> bool {
    request::requests_help(arguments)
}

pub(crate) fn parse_notify_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<NotificationRequest> {
    request::parse_notify_args(args, "zetta notify", notify_help)
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
    request::parse_notify_cleanup_args(args, notify_cleanup_help)
}

fn notification_helper_executable() -> Result<PathBuf> {
    Ok(std::env::current_exe()
        .context("locating the Zetta executable")?
        .with_file_name(format!("zntfy{}", std::env::consts::EXE_SUFFIX)))
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
        return zntfy::run_macos_hosted_notification(&command, target);
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
