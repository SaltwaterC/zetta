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
#[cfg(feature = "notifications")]
pub(crate) use notify::{notify_help, parse_notify_args};

#[cfg(feature = "clipboard")]
mod clipboard;
#[cfg(feature = "clipboard")]
pub(crate) use clipboard::{copy_help, parse_copy_args, parse_paste_args, paste_help};

#[cfg(cli_services)]
use anyhow::Result;

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
            Self::Notify(command) => command.run(),
            #[cfg(feature = "clipboard")]
            Self::Copy(command) => command.run(),
            #[cfg(feature = "clipboard")]
            Self::Paste(command) => command.run(),
            #[cfg(feature = "clipboard")]
            Self::CopyDaemon => clipboard::run_clipboard_copy_daemon(),
        }
    }
}
