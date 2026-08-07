#[cfg(linux_like)]
use std::env;
use std::ffi::OsString;
use std::io::{self, Read as _, Write as _};

use anyhow::{Context as _, Result};

use super::CliServiceCommand;

const CLIPBOARD_DAEMON_FLAG: &str = "--internal-clipboard-daemon";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CopyCommand;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PasteCommand;

pub(crate) fn copy_help() -> &'static str {
    "Copy standard input to the clipboard\n\nUsage: zetta copy [OPTIONS]\n\nReads standard input and writes it to the system clipboard as UTF-8 text, mirroring macOS's pbcopy. Available as zcopy in shell integration, and as pbcopy on platforms other than macOS so pbcopy muscle memory keeps working there too.\n\nOptions:\n  -pboard NAME                 Accepted for pbcopy compatibility (general, ruler, find, or font); Zetta has only one clipboard, so this has no effect\n  -h, -help, --help            Print help\n\nOn Linux and FreeBSD, Zetta forks a short-lived background process that keeps serving the clipboard after this command exits, since the X11 and Wayland clipboards are only available while their owning process is running. macOS and Windows keep the clipboard through their own system services, so no such process is needed there."
}

pub(crate) fn paste_help() -> &'static str {
    "Print the clipboard's contents\n\nUsage: zetta paste [OPTIONS]\n\nWrites the system clipboard's text contents to standard output, mirroring macOS's pbpaste. Available as zpaste in shell integration, and as pbpaste on platforms other than macOS so pbpaste muscle memory keeps working there too. Prints nothing if the clipboard is empty or holds no text.\n\nOptions:\n  -pboard NAME                 Accepted for pbpaste compatibility (general, ruler, find, or font); Zetta has only one clipboard, so this has no effect\n  -Prefer TYPE                 Accepted for pbpaste compatibility (txt, rtf, or ps); Zetta only stores plain text, so this has no effect\n  -h, -help, --help            Print help"
}

fn parse_pboard_name(argument: &OsString) -> Result<()> {
    let value = argument.to_string_lossy();
    anyhow::ensure!(
        matches!(
            value.to_ascii_lowercase().as_str(),
            "general" | "ruler" | "find" | "font"
        ),
        "-pboard must be general, ruler, find, or font, got {value:?}"
    );
    Ok(())
}

fn parse_prefer_type(argument: &OsString) -> Result<()> {
    let value = argument.to_string_lossy();
    anyhow::ensure!(
        matches!(value.to_ascii_lowercase().as_str(), "txt" | "rtf" | "ps"),
        "-Prefer must be txt, rtf, or ps, got {value:?}"
    );
    Ok(())
}

pub(crate) fn parse_copy_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let mut pboard_seen = false;
    let mut daemon = false;
    let mut arguments = args.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            CLIPBOARD_DAEMON_FLAG => daemon = true,
            "-pboard" | "--pboard" => {
                anyhow::ensure!(!pboard_seen, "-pboard may only be specified once");
                pboard_seen = true;
                parse_pboard_name(
                    &arguments
                        .next()
                        .context("-pboard requires general, ruler, find, or font")?,
                )?;
            }
            "--help" | "-h" | "-help" => anyhow::bail!("{}", copy_help()),
            option if option.starts_with('-') => anyhow::bail!("unknown copy option {option:?}"),
            value => anyhow::bail!("unexpected copy argument {value:?}"),
        }
    }
    Ok(if daemon {
        CliServiceCommand::CopyDaemon
    } else {
        CliServiceCommand::Copy(CopyCommand)
    })
}

pub(crate) fn parse_paste_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let mut pboard_seen = false;
    let mut prefer_seen = false;
    let mut arguments = args.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "-pboard" | "--pboard" => {
                anyhow::ensure!(!pboard_seen, "-pboard may only be specified once");
                pboard_seen = true;
                parse_pboard_name(
                    &arguments
                        .next()
                        .context("-pboard requires general, ruler, find, or font")?,
                )?;
            }
            "-Prefer" | "--Prefer" | "-prefer" | "--prefer" => {
                anyhow::ensure!(!prefer_seen, "-Prefer may only be specified once");
                prefer_seen = true;
                parse_prefer_type(
                    &arguments
                        .next()
                        .context("-Prefer requires txt, rtf, or ps")?,
                )?;
            }
            "--help" | "-h" | "-help" => anyhow::bail!("{}", paste_help()),
            option if option.starts_with('-') => anyhow::bail!("unknown paste option {option:?}"),
            value => anyhow::bail!("unexpected paste argument {value:?}"),
        }
    }
    Ok(CliServiceCommand::Paste(PasteCommand))
}

#[cfg(linux_like)]
fn spawn_clipboard_copy_daemon(text: String) -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;

    let executable = env::current_exe().context("locating the zetta executable")?;
    let mut command = std::process::Command::new(executable);
    command
        .arg("copy")
        .arg(CLIPBOARD_DAEMON_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir("/");
    // SAFETY: setsid(2) is async-signal-safe and is the only call made in the forked child
    // before it execs; detaching into its own session keeps the clipboard daemon alive after
    // this shell's session, and its controlling terminal, goes away.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut daemon = command.spawn().context("spawning the clipboard daemon")?;
    daemon
        .stdin
        .take()
        .context("the clipboard daemon did not provide a standard input pipe")?
        .write_all(text.as_bytes())
        .context("sending clipboard contents to the daemon")?;
    Ok(())
}

pub(super) fn run_clipboard_copy_daemon() -> Result<()> {
    let mut input = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut input)
        .context("reading standard input")?;
    let mut clipboard = arboard::Clipboard::new().context("opening the system clipboard")?;
    #[cfg(linux_like)]
    {
        use arboard::SetExtLinux as _;
        clipboard
            .set()
            .wait()
            .text(input)
            .context("serving the system clipboard")?;
    }
    #[cfg(not(linux_like))]
    clipboard
        .set_text(input)
        .context("writing to the system clipboard")?;
    Ok(())
}

impl CopyCommand {
    pub(super) fn run(&self) -> Result<()> {
        let mut input = String::new();
        io::stdin()
            .lock()
            .read_to_string(&mut input)
            .context("reading standard input")?;
        #[cfg(linux_like)]
        {
            spawn_clipboard_copy_daemon(input)
        }
        #[cfg(not(linux_like))]
        {
            let mut clipboard =
                arboard::Clipboard::new().context("opening the system clipboard")?;
            clipboard
                .set_text(input)
                .context("writing to the system clipboard")?;
            Ok(())
        }
    }
}

impl PasteCommand {
    pub(super) fn run(&self) -> Result<()> {
        let mut clipboard = arboard::Clipboard::new().context("opening the system clipboard")?;
        match clipboard.get_text() {
            Ok(text) => io::stdout()
                .write_all(text.as_bytes())
                .context("writing the clipboard contents to standard output")?,
            Err(arboard::Error::ContentNotAvailable) => {}
            Err(error) => return Err(error).context("reading the system clipboard"),
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/cli_services/clipboard.rs"]
mod tests;
