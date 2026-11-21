//! The `ssh` command lines the transport runs, and the shell quoting the
//! remote half of each needs.

use super::*;

/// A command on the remote host, over `control` when there is one.
///
/// A session on a shared connection also drops the forwards the user's
/// configuration would add: the login that owns them already has them, and a
/// second request for the same local port would only fail.
pub(super) fn command_arguments(
    target: &RemoteTarget,
    control: Option<&Path>,
    remote_command: String,
) -> Vec<String> {
    let mut arguments = vec!["-T".to_owned()];
    if let Some(control) = control {
        arguments.extend([
            "-S".to_owned(),
            control.display().to_string(),
            "-o".to_owned(),
            "ControlMaster=no".to_owned(),
            "-o".to_owned(),
            "ClearAllForwardings=yes".to_owned(),
        ]);
    }
    push_target_options(&mut arguments, target);
    arguments.extend([target.destination.clone(), remote_command]);
    arguments
}

#[cfg(any(unix, test))]
pub(super) fn endpoint_arguments(target: &RemoteTarget, control: Option<&Path>) -> Vec<String> {
    command_arguments(target, control, remote_endpoint_command())
}

pub(super) fn program_arguments(
    target: &RemoteTarget,
    control: Option<&Path>,
    platform: HostPlatform,
) -> Vec<String> {
    let command = match platform {
        HostPlatform::Posix => remote_program_command(),
        HostPlatform::Windows => remote_host::program_command(),
    };
    command_arguments(target, control, command)
}

pub(super) fn profiles_arguments(
    target: &RemoteTarget,
    control: Option<&Path>,
    platform: HostPlatform,
) -> Vec<String> {
    let command = match platform {
        HostPlatform::Posix => REMOTE_PROFILES_COMMAND.to_owned(),
        HostPlatform::Windows => remote_host::profiles_command(),
    };
    command_arguments(target, control, command)
}

pub(super) fn start_daemon_arguments(
    target: &RemoteTarget,
    control: Option<&Path>,
    program: &Path,
    platform: HostPlatform,
) -> Vec<String> {
    if platform == HostPlatform::Windows {
        return command_arguments(target, control, remote_host::start_daemon_command(program));
    }
    let program = shell_escape_double_quoted(&program.to_string_lossy());
    let command = format!(
        r#"/bin/sh -c 'if command -v zmux >/dev/null 2>&1; then nohup "{program}" --daemon >/dev/null 2>&1 </dev/null & exit; fi; exec "${{SHELL:-/bin/sh}}" -lic "nohup \"{program}\" --daemon >/dev/null 2>&1 </dev/null &"'"#
    );
    command_arguments(target, control, command)
}

/// The login every other Unix command shares. It runs nothing itself (`-N`);
/// `ControlPersist=no` keeps it in the foreground as this process's child, so
/// it ends when the transport does rather than lingering on its own.
#[cfg(any(unix, test))]
pub(super) fn master_arguments(target: &RemoteTarget, control_path: &Path) -> Vec<String> {
    let mut arguments = vec![
        "-T".to_owned(),
        "-N".to_owned(),
        "-M".to_owned(),
        "-S".to_owned(),
        control_path.display().to_string(),
        "-o".to_owned(),
        "ControlPersist=no".to_owned(),
        "-o".to_owned(),
        "ExitOnForwardFailure=yes".to_owned(),
    ];
    push_target_options(&mut arguments, target);
    arguments.push(target.destination.clone());
    arguments
}

/// Adds (`forward`) or removes (`cancel`) a stream-local forward on the
/// master. The master does the work locally; nothing crosses the network.
#[cfg(any(unix, test))]
pub(super) fn forward_request_arguments(
    target: &RemoteTarget,
    control_path: &Path,
    operation: &str,
    forwarding: &str,
) -> Vec<String> {
    vec![
        "-S".to_owned(),
        control_path.display().to_string(),
        "-O".to_owned(),
        operation.to_owned(),
        "-L".to_owned(),
        forwarding.to_owned(),
        target.destination.clone(),
    ]
}

/// A session that keeps the forwarded agent linked where daemon-owned shells
/// look for it. On a shared connection this asks the master to forward the
/// agent for this session, which it can because it was started with `-A`.
#[cfg(any(unix, test))]
pub(super) fn agent_holder_arguments(
    target: &RemoteTarget,
    control_path: &Path,
    socket: &Path,
) -> Vec<String> {
    command_arguments(target, Some(control_path), agent_holder_command(socket))
}

/// The multiplexed bridge: a Windows client's one login, or a session on a
/// Unix client's master.
pub(super) fn bridge_arguments(
    target: &RemoteTarget,
    control: Option<&Path>,
    platform: HostPlatform,
) -> Vec<String> {
    let link_agent = target.forward_agent == Some(true);
    let command = match platform {
        HostPlatform::Posix => remote_bridge_command(link_agent),
        HostPlatform::Windows => remote_host::bridge_command(link_agent),
    };
    let mut arguments = command_arguments(target, control, command);
    if control.is_none() {
        // A login of its own drops the configured forwards too: the bridge is
        // the only thing it carries.
        arguments.splice(
            1..1,
            ["-o".to_owned(), "ClearAllForwardings=yes".to_owned()],
        );
    }
    arguments
}

pub(super) fn shell_escape_double_quoted(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
}

#[cfg(any(unix, test))]
pub(super) fn agent_holder_command(socket: &Path) -> String {
    let socket = shell_quote(&socket.to_string_lossy());
    let script = format!(
        "umask 077; link={socket}; temporary=\"$link.$$\"; if test -n \"$SSH_AUTH_SOCK\"; then rm -f \"$temporary\"; ln -s \"$SSH_AUTH_SOCK\" \"$temporary\" && mv -f \"$temporary\" \"$link\"; fi; exec sleep 2147483647"
    );
    format!("/bin/sh -c {}", shell_quote(&script))
}

#[cfg(any(unix, test))]
pub(super) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub(super) fn push_target_options(arguments: &mut Vec<String>, target: &RemoteTarget) {
    if let Some(forward_agent) = target.forward_agent {
        arguments.push(if forward_agent { "-A" } else { "-a" }.to_owned());
    }
    if let Some(port) = target.port {
        arguments.push("-p".to_owned());
        arguments.push(port.to_string());
    }
}
