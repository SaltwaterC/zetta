//! Reading `wsl.exe`'s own command line, which `wslx.exe` forwards untouched.
//!
//! It is read for two answers only. Does this invocation run something in a
//! distribution, as opposed to `--list`, `--shutdown` or `--install`, which
//! get no agent? And which distribution and user does it run as, so that the
//! relay is started in the same place and its socket belongs to the user who
//! will connect to it?
//!
//! Options ahead of the command line that this does not recognise are taken to
//! be management commands. Getting that wrong for a new session option costs
//! only the forwarded agent; getting it wrong the other way would start a
//! relay for a command that never opens a session — and against whatever
//! distribution the unknown option's value happened to name.

use std::ffi::{OsStr, OsString};

#[derive(Debug, PartialEq, Eq)]
pub enum Invocation<'a> {
    /// A session, and the arguments that select where it runs and as whom —
    /// the ones the relay has to be started with too.
    Session { target: Vec<&'a OsStr> },
    /// Anything else; `wsl.exe` runs it without an agent.
    Management,
}

/// Options that pick the distribution or user, and so are repeated for the
/// relay, followed by how many values each takes.
const TARGET_OPTIONS: [(&str, usize); 6] = [
    ("-d", 1),
    ("--distribution", 1),
    ("--distribution-id", 1),
    ("-u", 1),
    ("--user", 1),
    ("--system", 0),
];

/// Session options that do not change where the session runs.
const SESSION_OPTIONS: [&str; 2] = ["--cd", "--shell-type"];

/// Arguments after which everything is the command line.
const COMMAND_MARKERS: [&str; 3] = ["--", "-e", "--exec"];

pub fn classify(args: &[OsString]) -> Invocation<'_> {
    let mut target = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        // A command line may well not be UTF-8, but no option is.
        let Some(text) = argument.to_str() else {
            break;
        };
        if let Some(&(_, values)) = TARGET_OPTIONS.iter().find(|(name, _)| *name == text) {
            let Some(selected) = args.get(index..=index + values) else {
                return Invocation::Management;
            };
            target.extend(selected.iter().map(OsString::as_os_str));
            index += 1 + values;
        } else if SESSION_OPTIONS.contains(&text) {
            if args.get(index + 1).is_none() {
                return Invocation::Management;
            }
            index += 2;
        } else if text == "~" && index == 0 {
            // `wsl ~` starts in the home directory, and only as the first
            // argument.
            index += 1;
        } else if COMMAND_MARKERS.contains(&text) || !text.starts_with('-') {
            break;
        } else {
            return Invocation::Management;
        }
    }
    Invocation::Session { target }
}

#[cfg(test)]
#[path = "tests/args.rs"]
mod tests;
