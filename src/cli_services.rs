#[cfg(feature = "serial-console")]
mod raw_terminal;
#[cfg(feature = "serial-console")]
mod serial;
#[cfg(all(test, feature = "serial-console"))]
pub(crate) use serial::SerialCommand;
#[cfg(feature = "serial-console")]
pub(crate) use serial::{parse_serial_args, serial_help};

#[cfg(servers_enabled)]
mod servers;
#[cfg(feature = "http-server")]
pub(crate) use servers::{http_server_help, parse_http_args};
#[cfg(feature = "tftp-server")]
pub(crate) use servers::{parse_tftp_server_args, tftp_server_help};

#[cfg(feature = "notifications")]
mod notify;
#[cfg(all(target_os = "macos", feature = "notifications"))]
pub(crate) use notify::macos_notification_target_for_response;
#[cfg(feature = "notifications")]
pub(crate) use notify::{notify_help, parse_notify_args, run_notification};

#[cfg(feature = "clipboard")]
mod clipboard;
#[cfg(feature = "clipboard")]
pub(crate) use clipboard::{copy_help, parse_copy_args, parse_paste_args, paste_help};

use anyhow::Result;

/// The timeout values accepted by both `notify` and `attention --notify`.
/// Keeping this type independent of notify-rust lets badge-only builds parse
/// and reject notification options without pulling in the optional backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum NotificationTimeout {
    #[default]
    Default,
    Never,
    Milliseconds(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NotificationRequest {
    pub(crate) summary: String,
    pub(crate) body: Option<String>,
    pub(crate) app_name: Option<String>,
    pub(crate) icon: Option<String>,
    pub(crate) sound: Option<String>,
    pub(crate) timeout: Option<NotificationTimeout>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NotificationTarget {
    pub(crate) process_id: u32,
    pub(crate) attention_id: u64,
}

pub(crate) fn parse_notification_timeout(value: &str) -> Result<NotificationTimeout> {
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

#[cfg(cli_services)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CliServiceCommand {
    #[cfg(feature = "serial-console")]
    Serial(serial::SerialCommand),
    #[cfg(feature = "http-server")]
    Http(servers::HttpServerCommand),
    #[cfg(feature = "tftp-server")]
    Tftp(servers::TftpServerCommand),
    #[cfg(feature = "notifications")]
    Notify(notify::NotifyCommand),
    #[cfg(feature = "clipboard")]
    Copy(clipboard::CopyCommand),
    #[cfg(feature = "clipboard")]
    Paste(clipboard::PasteCommand),
    #[cfg(feature = "clipboard")]
    CopyDaemon,
}

#[cfg(cli_services)]
impl CliServiceCommand {
    pub(crate) fn run(&self) -> Result<()> {
        match self {
            #[cfg(feature = "serial-console")]
            Self::Serial(command) => command.run(),
            #[cfg(feature = "http-server")]
            Self::Http(command) => command.run(),
            #[cfg(feature = "tftp-server")]
            Self::Tftp(command) => command.run(),
            #[cfg(feature = "notifications")]
            Self::Notify(command) => {
                notify::run_notification(command, notify::notification_target_from_environment())
            }
            #[cfg(feature = "clipboard")]
            Self::Copy(command) => command.run(),
            #[cfg(feature = "clipboard")]
            Self::Paste(command) => command.run(),
            #[cfg(feature = "clipboard")]
            Self::CopyDaemon => clipboard::run_clipboard_copy_daemon(),
        }
    }
}
