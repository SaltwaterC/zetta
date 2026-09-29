//! Starting a daemon that outlives the session that asked for it.
//!
//! What `zmux --daemon --detach` does, which is how a remote client starts a
//! Windows host's daemon over SSH. It is its own module because on Windows
//! outliving the session takes more than `Command` offers:
//!
//! - sshd ends a session by closing the job its processes are in, so the
//!   daemon has to break away from that job;
//! - sshd ends the *command* only once every holder of its pipes has closed
//!   them, and `Command` always spawns with handle inheritance on — which hands
//!   the child every inheritable handle in this process, not only the three it
//!   was given. The shells between sshd and this process leave such handles
//!   behind, so a daemon spawned by `Command` keeps the command that started
//!   it from ever returning. The daemon is therefore created with no inherited
//!   handles at all.

use std::{ffi::OsString, path::Path};

use anyhow::Result;

/// Starts `executable` with `arguments`, detached from this process's
/// session, and returns once it has been created.
#[cfg(unix)]
pub(crate) fn spawn_detached(executable: &Path, arguments: &[OsString]) -> Result<()> {
    use std::{
        os::unix::process::CommandExt as _,
        process::{Command, Stdio},
    };

    // A process group of its own, so the session's hangup does not reach it.
    Command::new(executable)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn spawn_detached(executable: &Path, arguments: &[OsString]) -> Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    use anyhow::Context as _;
    use windows::{
        Win32::{
            Foundation::CloseHandle,
            System::Threading::{
                CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CreateProcessW,
                DETACHED_PROCESS, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
            },
        },
        core::{PCWSTR, PWSTR},
    };

    let executable_wide = executable
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let words = std::iter::once(executable.as_os_str())
        .chain(arguments.iter().map(OsString::as_os_str))
        .map(|word| {
            word.to_str()
                .with_context(|| format!("{word:?} is not valid Unicode"))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut command_line = command_line(&words)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    let spawn = |flags: PROCESS_CREATION_FLAGS, command_line: &mut Vec<u16>| {
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: both strings are NUL-terminated and outlive the call, the
        // command line buffer is writable as CreateProcessW requires, and no
        // handle is inherited.
        unsafe {
            CreateProcessW(
                PCWSTR(executable_wide.as_ptr()),
                Some(PWSTR(command_line.as_mut_ptr())),
                None,
                None,
                false,
                flags,
                None,
                PCWSTR::null(),
                &startup,
                &mut process,
            )
        }?;
        // SAFETY: CreateProcessW succeeded, so both handles are ours to close.
        unsafe {
            let _ = CloseHandle(process.hThread);
            let _ = CloseHandle(process.hProcess);
        }
        windows::core::Result::Ok(())
    };
    if let Err(error) = spawn(flags | CREATE_BREAKAWAY_FROM_JOB, &mut command_line) {
        // A job that forbids breaking away refuses the spawn outright. Over
        // SSH staying in the job means dying with the session; anywhere else
        // it is some other job's policy, which a daemon can live with.
        anyhow::ensure!(
            std::env::var_os("SSH_CONNECTION").is_none(),
            "this SSH session's job does not let the multiplexer outlive it: {error}"
        );
        spawn(flags, &mut command_line)?;
    }
    Ok(())
}

/// Joins `words` into a Windows command line that `CommandLineToArgvW`, and
/// the Rust runtime's own argument parser, split back into the same words.
#[cfg(any(windows, test))]
fn command_line(words: &[&str]) -> String {
    let mut line = String::new();
    for word in words {
        if !line.is_empty() {
            line.push(' ');
        }
        if !word.is_empty() && !word.contains([' ', '\t', '"']) {
            line.push_str(word);
            continue;
        }
        line.push('"');
        let mut backslashes = 0;
        for character in word.chars() {
            match character {
                '\\' => backslashes += 1,
                '"' => {
                    // Backslashes before a quote are escapes, so double them,
                    // then escape the quote itself.
                    line.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    line.push('"');
                    backslashes = 0;
                }
                other => {
                    line.extend(std::iter::repeat_n('\\', backslashes));
                    line.push(other);
                    backslashes = 0;
                }
            }
        }
        // Before the closing quote, backslashes are escapes too.
        line.extend(std::iter::repeat_n('\\', backslashes * 2));
        line.push('"');
    }
    line
}

#[cfg(test)]
#[path = "tests/daemon_spawn.rs"]
mod tests;
