use std::io::Write as _;

#[cfg(unix)]
use std::fs::OpenOptions;

use anyhow::{Context as _, Result};
use zeroize::Zeroizing;

#[cfg(windows)]
use std::io::{self, IsTerminal as _};

use crate::{
    background_sessions::{
        BackgroundSessionCatalog, SessionSecret, read_session_catalogs, session_catalog_dir,
    },
    process_control::{ReconnectSessionResult, request_reconnect_session},
};

#[derive(Debug)]
struct SessionTarget {
    process_id: u32,
    runner_id: u64,
    session_id: u64,
    authentication_required: bool,
}

pub(crate) fn run_reconnect_session(identifier: &str) -> Result<()> {
    let catalogs = read_session_catalogs(&session_catalog_dir())?;
    let target = find_session(&catalogs, identifier)?;
    let secret = target
        .authentication_required
        .then(read_private_secret)
        .transpose()?;
    match request_reconnect_session(
        target.process_id,
        target.runner_id,
        target.session_id,
        secret,
    )? {
        ReconnectSessionResult::Reconnected => {
            println!("Reconnected session {identifier}.");
            Ok(())
        }
        ReconnectSessionResult::AuthenticationFailed => {
            anyhow::bail!(
                "could not reconnect session {identifier:?}: the session secret was incorrect"
            )
        }
        ReconnectSessionResult::SessionNotFound => {
            anyhow::bail!(
                "could not reconnect session {identifier:?}: the session no longer exists"
            )
        }
        ReconnectSessionResult::StillStarting => anyhow::bail!(
            "could not reconnect session {identifier:?}: the session is still starting; try again shortly"
        ),
        ReconnectSessionResult::Rejected => anyhow::bail!(
            "could not reconnect session {identifier:?}: Zetta rejected the reconnect request"
        ),
    }
}

fn find_session(catalogs: &[BackgroundSessionCatalog], identifier: &str) -> Result<SessionTarget> {
    let parsed = identifier
        .split_once(':')
        .and_then(|(_, rest)| rest.split_once(':'))
        .is_some();
    let target = if parsed {
        let mut parts = identifier.split(':');
        let process_id = parts
            .next()
            .context("session ID must have the form PROCESS:RUNNER:SESSION")?
            .parse::<u32>()
            .context("session process ID must be a positive whole number")?;
        let runner_id = parts
            .next()
            .context("session ID must have the form PROCESS:RUNNER:SESSION")?
            .parse::<u64>()
            .context("session runner ID must be a positive whole number")?;
        let session_id = parts
            .next()
            .context("session ID must have the form PROCESS:RUNNER:SESSION")?
            .parse::<u64>()
            .context("session ID must be a positive whole number")?;
        anyhow::ensure!(
            parts.next().is_none(),
            "session ID must have the form PROCESS:RUNNER:SESSION"
        );
        catalogs.iter().find_map(|catalog| {
            (catalog.process_id == process_id && catalog.runner_id == runner_id)
                .then(|| {
                    catalog
                        .sessions
                        .iter()
                        .find(|session| session.id == session_id)
                })
                .flatten()
                .map(|session| SessionTarget {
                    process_id,
                    runner_id,
                    session_id,
                    authentication_required: session.authentication_required,
                })
        })
    } else {
        let session_id = identifier
            .parse::<u64>()
            .context("session ID must be PROCESS:RUNNER:SESSION")?;
        let matches = catalogs.iter().flat_map(|catalog| {
            catalog.sessions.iter().filter_map(|session| {
                (session.id == session_id).then_some(SessionTarget {
                    process_id: catalog.process_id,
                    runner_id: catalog.runner_id,
                    session_id,
                    authentication_required: session.authentication_required,
                })
            })
        });
        let mut matches = matches.collect::<Vec<_>>();
        anyhow::ensure!(
            !matches.is_empty(),
            "background session {identifier:?} was not found"
        );
        anyhow::ensure!(
            matches.len() == 1,
            "session ID {identifier:?} is ambiguous; use the full PROCESS:RUNNER:SESSION ID"
        );
        Some(matches.remove(0))
    };
    target.with_context(|| format!("background session {identifier:?} was not found"))
}

fn read_private_secret() -> Result<SessionSecret> {
    read_private_secret_text().map(SessionSecret::from_zeroizing)
}

fn read_private_secret_text() -> Result<Zeroizing<String>> {
    #[cfg(unix)]
    {
        let mut terminal = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("opening the controlling terminal for the session secret")?;
        write!(terminal, "Session secret: ")?;
        terminal.flush()?;
        let echo = NoEcho::disable(&terminal)?;
        let mut input = terminal
            .try_clone()
            .context("duplicating the controlling terminal")?;
        let result = read_masked_secret(&mut input, &mut terminal);
        drop(echo);
        writeln!(terminal)?;
        let secret = result?;
        anyhow::ensure!(!secret.is_empty(), "session secret must not be empty");
        Ok(secret)
    }

    #[cfg(windows)]
    {
        anyhow::ensure!(
            io::stdin().is_terminal(),
            "protected session reconnect requires an interactive terminal"
        );
        let mut stdout = io::stdout().lock();
        write!(stdout, "Session secret: ")?;
        stdout.flush()?;
        let echo = NoEcho::disable()?;
        let mut stdin = io::stdin().lock();
        let result = read_masked_secret(&mut stdin, &mut stdout);
        drop(echo);
        writeln!(stdout)?;
        let secret = result?;
        anyhow::ensure!(!secret.is_empty(), "session secret must not be empty");
        Ok(secret)
    }

    #[cfg(not(any(unix, windows)))]
    anyhow::bail!("protected session reconnect is not supported on this platform")
}

fn read_masked_secret<R: std::io::Read, W: std::io::Write>(
    input: &mut R,
    output: &mut W,
) -> Result<Zeroizing<String>> {
    let mut secret = Zeroizing::new(String::new());
    let mut pending_utf8 = Vec::new();
    let mut byte = [0; 1];
    loop {
        input
            .read_exact(&mut byte)
            .context("reading the session secret")?;
        match byte[0] {
            b'\r' | b'\n' => {
                anyhow::ensure!(
                    pending_utf8.is_empty(),
                    "session secret contains invalid UTF-8"
                );
                break;
            }
            3 | 4 => anyhow::bail!("session secret input interrupted"),
            8 | 127 => {
                pending_utf8.clear();
                if secret.pop().is_some() {
                    write!(output, "\x08 \x08")?;
                }
            }
            21 => {
                pending_utf8.clear();
                let characters = secret.chars().count();
                secret.clear();
                for _ in 0..characters {
                    write!(output, "\x08 \x08")?;
                }
            }
            byte if byte.is_ascii_control() => {}
            byte => {
                pending_utf8.push(byte);
                match std::str::from_utf8(&pending_utf8) {
                    Ok(character) => {
                        secret.push_str(character);
                        pending_utf8.clear();
                        write!(output, "*")?;
                    }
                    Err(error) if error.error_len().is_some() => {
                        anyhow::bail!("session secret contains invalid UTF-8")
                    }
                    Err(_) => {}
                }
            }
        }
        output.flush()?;
    }
    Ok(secret)
}

#[cfg(test)]
#[path = "tests/session_cli.rs"]
mod tests;

#[cfg(unix)]
struct NoEcho {
    fd: std::os::unix::io::RawFd,
    original: libc::termios,
}

#[cfg(unix)]
impl NoEcho {
    fn disable<T: std::os::unix::io::AsRawFd>(terminal: &T) -> Result<Self> {
        let fd = terminal.as_raw_fd();
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: fd belongs to the open controlling terminal and the pointer is writable.
        anyhow::ensure!(
            unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } == 0,
            "reading controlling terminal settings"
        );
        // SAFETY: tcgetattr initialized original after returning zero above.
        let original = unsafe { original.assume_init() };
        let mut quiet = original;
        // SAFETY: quiet is a valid termios structure owned by this function.
        unsafe { libc::cfmakeraw(&mut quiet) };
        // SAFETY: fd belongs to the open controlling terminal and quiet is initialized.
        anyhow::ensure!(
            unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) } == 0,
            "disabling terminal echo"
        );
        Ok(Self { fd, original })
    }
}

#[cfg(unix)]
impl Drop for NoEcho {
    fn drop(&mut self) {
        // SAFETY: fd and original were captured from the controlling terminal.
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.original) };
    }
}

#[cfg(windows)]
struct NoEcho {
    handle: windows::Win32::Foundation::HANDLE,
    original: windows::Win32::System::Console::CONSOLE_MODE,
}

#[cfg(windows)]
impl NoEcho {
    fn disable() -> Result<Self> {
        use windows::Win32::System::Console::{
            CONSOLE_MODE, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
            GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
        };
        // SAFETY: the API obtains the current process's standard input handle.
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) }?;
        let mut original = CONSOLE_MODE(0);
        // SAFETY: handle comes from GetStdHandle and original is writable.
        unsafe { GetConsoleMode(handle, &mut original) }?;
        let quiet = original & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT);
        // SAFETY: handle comes from GetStdHandle and quiet is a valid mode bitset.
        unsafe { SetConsoleMode(handle, quiet) }?;
        Ok(Self { handle, original })
    }
}

#[cfg(windows)]
impl Drop for NoEcho {
    fn drop(&mut self) {
        use windows::Win32::System::Console::SetConsoleMode;
        // SAFETY: handle and original were captured from the active console.
        let _ = unsafe { SetConsoleMode(self.handle, self.original) };
    }
}
