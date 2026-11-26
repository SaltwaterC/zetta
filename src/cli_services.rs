//! The command-line services the `zetta` binary runs itself, each behind its
//! feature: the serial console, the HTTP and TFTP servers, notifications and
//! the clipboard.

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

#[cfg(tftp_setcap)]
mod tftp_setcap;
#[cfg(tftp_setcap)]
pub(crate) use tftp_setcap::{TFTP_SETCAP_HINT, parse_tftp_setcap_args, tftp_setcap_help};

#[cfg(feature = "notifications")]
mod notify_proxy;
#[cfg(all(target_os = "macos", feature = "notifications"))]
pub(crate) use notify_proxy::macos_notification_target_for_response;
#[cfg(notify_cleanup_enabled)]
pub(crate) use notify_proxy::{notify_cleanup_help, parse_notify_cleanup_args};
#[cfg(feature = "notifications")]
pub(crate) use notify_proxy::{
    notify_help, parse_notify_args, requests_notify_help, run_notification_proxy,
};
#[cfg(all(test, notify_cleanup_enabled))]
pub(crate) use zntfy::request::NotifyCleanupCommand;

#[cfg(feature = "clipboard")]
mod clipboard;
#[cfg(feature = "clipboard")]
pub(crate) use clipboard::{copy_help, parse_copy_args, parse_paste_args, paste_help};

#[cfg(cli_services)]
use anyhow::Result;

// `zntfy` owns what a notification request is. Zetta links it without its
// `backends` feature everywhere but macOS, where it delivers through it.
#[cfg(feature = "notifications")]
pub(crate) use zntfy::request::NotificationTimeout;
pub(crate) use zntfy::request::{NotificationOptions, NotificationRequest, NotificationTarget};

#[cfg(cli_services)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CliServiceCommand {
    #[cfg(feature = "serial-console")]
    Serial(serial::SerialCommand),
    #[cfg(feature = "http-server")]
    Http(servers::HttpServerCommand),
    #[cfg(feature = "tftp-server")]
    Tftp(servers::TftpServerCommand),
    #[cfg(tftp_setcap)]
    TftpSetcap(tftp_setcap::TftpSetcapCommand),
    #[cfg(feature = "notifications")]
    Notify(NotificationRequest),
    #[cfg(notify_cleanup_enabled)]
    NotifyCleanup(zntfy::request::NotifyCleanupCommand),
    #[cfg(feature = "clipboard")]
    Copy(clipboard::CopyCommand),
    #[cfg(feature = "clipboard")]
    Paste(clipboard::PasteCommand),
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
            #[cfg(tftp_setcap)]
            Self::TftpSetcap(command) => command.run(),
            #[cfg(feature = "notifications")]
            Self::Notify(command) => notify_proxy::run_notification_proxy(
                command,
                zntfy::request::notification_target_from_environment(),
            ),
            #[cfg(notify_cleanup_enabled)]
            Self::NotifyCleanup(command) => notify_proxy::run_notification_cleanup_proxy(command),
            #[cfg(feature = "clipboard")]
            Self::Copy(command) => command.run(),
            #[cfg(feature = "clipboard")]
            Self::Paste(command) => command.run(),
        }
    }
}
