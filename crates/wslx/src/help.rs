//! What `wslx --help` says about `wslx.exe` before `wsl.exe` prints its own
//! help.
//!
//! `wslx.exe` adds no options — every argument is `wsl.exe`'s — so this is
//! prose about the agent forwarding rather than an options table.

pub const HELP: &str = r#"wslx: wsl.exe with the shell's SSH agent carried into WSL

Usage: wslx [wsl.exe arguments]

Every argument is passed to wsl.exe unchanged, and wslx exits with its
status. wsl.exe's own help follows this section.

SSH agent forwarding:
  When SSH_AUTH_SOCK names a Windows named pipe, such as the
  \\.\pipe\zosh-agent-... a Zosh pane is given, and the arguments start a
  session, wslx first starts a relay in the same distribution as the same
  user. The session then starts with SSH_AUTH_SOCK set to the relay's Unix
  socket, added to WSLENV so WSL passes it through.

  The socket is in a directory only that user can open, and is removed when
  the session ends. The relay is a static Linux binary inside wslx.exe, for
  x86-64 and Arm64; the first session of each Zetta build copies it to
  ~/.cache/zetta/wslx in the distribution, so nothing has to be installed
  there. If it cannot start, wslx says why and starts the session without an
  agent.

  Commands that start no session (--list, --shutdown, --install, ...), and an
  SSH_AUTH_SOCK that is unset or not a pipe, run as plain wsl.exe.

Shell startup files:
  A startup file that sets SSH_AUTH_SOCK itself (wsl2-ssh-agent, keychain,
  eval $(ssh-agent)) replaces the forwarded agent. Have it set its own only
  when no agent socket was handed in:

    [ -S "$SSH_AUTH_SOCK" ] || eval "$(ssh-agent)"

"#;

#[cfg(test)]
#[path = "tests/help.rs"]
mod tests;
