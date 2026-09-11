//! The profile set `Config::defaults` starts from, before any file is read.
//!
//! Detecting what is installed lives in `zetta_profiles`, because `zmux` needs
//! the same answer: a pane in a shared session is created on the daemon's host,
//! and the profile name is all that crosses the wire. What is left here is the
//! half that only matters to something with a window — the profile's icon, and
//! the [`Shell`] the application's own spawn path is written against.

use super::*;

/// Converts what this machine can run into what the application shows.
///
/// The icon is derived rather than carried across: `automatic_for_profile`
/// already recognises every family the detection produces — `WSL:` prefixes,
/// the MSYS2 and Cygwin names, and otherwise the shell's own executable — so
/// deciding it here keeps one rule instead of two that can disagree.
pub(super) fn discovered_profiles() -> Vec<Profile> {
    zetta_profiles::discovered_profiles()
        .into_iter()
        .map(|profile| {
            let command = profile_shell(&profile.name, profile.command);
            Profile {
                icon: ProfileIcon::automatic_for_profile(&profile.name, &command),
                name: profile.name,
                command,
                theme: None,
                dark_theme: None,
            }
        })
        .collect()
}

/// A detected command as the application's spawn path spells one.
///
/// The title override is the profile's name, which is what the detection used
/// to set for every command that carried arguments.
pub(crate) fn profile_shell(name: &str, command: zetta_profiles::ProfileCommand) -> Shell {
    let zetta_profiles::ProfileCommand { program, args } = command;
    match program {
        None => Shell::System,
        Some(program) if args.is_empty() => Shell::Program(program),
        Some(program) => Shell::WithArguments {
            program,
            args,
            title_override: Some(name.to_owned()),
        },
    }
}

/// The reverse, for a profile that has to be resolved on another machine: the
/// name travels, and the host it lands on decides what it runs.
#[cfg(feature = "zmux")]
pub(crate) fn profile_command(shell: &Shell) -> zetta_profiles::ProfileCommand {
    match shell {
        Shell::System => zetta_profiles::ProfileCommand::system(),
        Shell::Program(program) => zetta_profiles::ProfileCommand::program(program.clone()),
        Shell::WithArguments { program, args, .. } => {
            zetta_profiles::ProfileCommand::with_args(program.clone(), args.clone())
        }
    }
}

#[cfg(test)]
#[path = "../tests/config/discovery.rs"]
mod tests;
