//! `zntfy cleanup`: reaping notification workers that outlived their
//! notification. Only Linux and BSD need it, because only their notification
//! servers fail to report an expiry; `build.rs` sets `notify_cleanup_enabled`.

use std::ffi::OsString;

use anyhow::Result;

use zetta_cli_help::format_help_table;

use crate::notify::{NOTIFICATION_WORKER_ENV, notification_expiry_duration, parse_notify_args};
use crate::request::{self, NotificationTimeout, NotifyCleanupCommand};

pub fn notify_cleanup_help() -> String {
    format!(
        "Reap stale desktop notification worker processes\n\nUsage: zntfy cleanup [OPTIONS]\n\nA `zntfy` invocation that targets a pane spawns a detached worker that waits for the notification to be clicked or dismissed, so it can focus the originating tab. Some notification servers (GNOME Shell in particular) do not reliably signal when a notification expires, which can leave a worker running indefinitely with nothing left to click. This command finds Zetta notification workers that have outlived their notification's own timeout and terminates them.\n\nOptions:\n{}",
        format_help_table([
            (
                "-n, --dry-run",
                "List stale workers without terminating them"
            ),
            ("-h, --help", "Print help"),
        ])
    )
}

pub fn parse_notify_cleanup_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<NotifyCleanupCommand> {
    request::parse_notify_cleanup_args(args, notify_cleanup_help)
}

struct StaleNotificationWorker {
    process_id: sysinfo::Pid,
    age: std::time::Duration,
    expiry: std::time::Duration,
}

/// Recovers the `--timeout` a notification worker was launched with by
/// reparsing its argv the same way it parsed it at startup. The previous
/// `zetta notify` worker had an extra `notify` argument; accept that spelling
/// as well so the standalone cleanup command can reap old workers.
pub(crate) fn worker_notification_timeout(cmd: &[OsString]) -> Option<Option<NotificationTimeout>> {
    let executable = std::path::Path::new(cmd.first()?).file_stem()?.to_str()?;
    let arguments = match executable {
        "zntfy" => &cmd[1..],
        "zetta" if cmd.get(1)?.to_str()? == "notify" => &cmd[2..],
        _ => return None,
    };
    let command = parse_notify_args(arguments.iter().cloned()).ok()?;
    Some(command.timeout)
}

fn stale_notification_workers(system: &sysinfo::System) -> Vec<StaleNotificationWorker> {
    let worker_marker = OsString::from(format!("{NOTIFICATION_WORKER_ENV}=1"));
    system
        .processes()
        .values()
        .filter(|process| process.environ().contains(&worker_marker))
        .filter_map(|process| {
            let timeout = worker_notification_timeout(process.cmd())?;
            let expiry = notification_expiry_duration(timeout)?;
            let age = std::time::Duration::from_secs(process.run_time());
            (age > expiry).then_some(StaleNotificationWorker {
                process_id: process.pid(),
                age,
                expiry,
            })
        })
        .collect()
}

pub fn run_notify_cleanup(command: &NotifyCleanupCommand) -> Result<()> {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing()
            .with_environ(sysinfo::UpdateKind::Always)
            .with_cmd(sysinfo::UpdateKind::Always),
    );

    let stale = stale_notification_workers(&system);
    if stale.is_empty() {
        println!("No stale desktop notification workers found.");
        return Ok(());
    }

    for worker in &stale {
        let verb = if command.dry_run {
            "would reap"
        } else {
            "reaping"
        };
        println!(
            "{verb} pid {} (running {}s, past its {}s notification timeout)",
            worker.process_id,
            worker.age.as_secs(),
            worker.expiry.as_secs(),
        );
    }
    if command.dry_run {
        return Ok(());
    }

    let mut reaped = 0;
    for worker in &stale {
        match system.process(worker.process_id) {
            Some(process) if process.kill_with(sysinfo::Signal::Term) == Some(true) => {
                reaped += 1;
            }
            _ => eprintln!(
                "zetta: failed to terminate notification worker pid {}",
                worker.process_id
            ),
        }
    }
    println!(
        "Reaped {reaped} of {} stale desktop notification worker(s).",
        stale.len()
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/cleanup.rs"]
mod tests;
