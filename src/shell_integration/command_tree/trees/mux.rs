//! `zetta mux` and `zmux`, built only with the multiplexer.

use super::{
    Attached, CommandSpec, HELP, Helper, LEAF, OptionSpec, Values, attached, exits, option, switch,
};

/// `option`, offered only where a local multiplexer daemon can serve it.
const fn daemon_only(option: OptionSpec) -> OptionSpec {
    OptionSpec {
        needs_daemon: true,
        ..option
    }
}

const MUX_JSON: OptionSpec = switch("--json", &["-j"], "Print machine-readable JSON");
const MUX_IDS_ONLY: OptionSpec = switch(
    "--ids-only",
    &["-I"],
    "Print one numeric session ID per line",
);
const MUX_SSH_TARGET: OptionSpec = attached(
    Attached::Long,
    option(
        "--ssh-target",
        &["-H"],
        "OpenSSH destination",
        Values::Call(Helper::SshTargets),
    ),
);
const MUX_PORT: OptionSpec = attached(
    Attached::Long,
    option("--port", &["-p"], "SSH port", Values::Any),
);
const MUX_VERSION: OptionSpec = exits("--version", &["-v", "-V"], "Print version");
const MUX_IDENTITY: OptionSpec = daemon_only(OptionSpec {
    repeatable: true,
    ..attached(
        Attached::Long,
        option(
            "--identity",
            &["-i"],
            "Age identity file for resume and reconnect",
            Values::Files,
        ),
    )
});
const MUX_SECRET_STDIN: OptionSpec = daemon_only(switch(
    "--secret-stdin",
    &["-S"],
    "Read an optional create secret from standard input",
));
const MUX_LAYOUT: OptionSpec = daemon_only(attached(
    Attached::Long,
    option(
        "--layout",
        &[],
        "Create from a headless layout JSON file",
        Values::Files,
    ),
));
const MUX_PROFILE: OptionSpec = daemon_only(attached(
    Attached::Long,
    option(
        "--profile",
        &[],
        "Create one pane using a host profile",
        Values::ZettaLines("profile list"),
    ),
));
const MUX_TITLE: OptionSpec = daemon_only(attached(
    Attached::Long,
    option(
        "--title",
        &[],
        "Title for a newly created headless session",
        Values::Any,
    ),
));
const MUX_WORKING_DIRECTORY: OptionSpec = daemon_only(attached(
    Attached::Long,
    option(
        "--working-directory",
        &[],
        "Starting directory for a single-pane create",
        Values::Directories,
    ),
));
const MUX_ENV: OptionSpec = daemon_only(OptionSpec {
    repeatable: true,
    ..attached(
        Attached::Long,
        option(
            "--env",
            &[],
            "Environment override for a single-pane create",
            Values::Any,
        ),
    )
});
const MUX_RETENTION: OptionSpec = daemon_only(option(
    "--retention",
    &["-r"],
    "Retention mode for a local daemon",
    Values::Words(&["none", "memory", "disk"]),
));
const MUX_SESSION: Values = Values::Each(&[Values::Call(Helper::MuxSessionIds)]);
/// What `share`, `unshare`, `kill` and `forget` take.
const MUX_SCOPE_OPTIONS: &[OptionSpec] = &[MUX_JSON, MUX_SSH_TARGET, MUX_PORT, MUX_IDENTITY, HELP];

/// `zetta mux`. Its parser reads options anywhere, but a command is offered
/// only first, as the per-command options then follow it. With
/// `ZETTA_NO_MUX=1` there is no local daemon, so what needs one is hidden.
pub(super) const MUX: CommandSpec = CommandSpec {
    name: "mux",
    about: "Control the session multiplexer",
    options: &[
        MUX_JSON,
        MUX_IDS_ONLY,
        MUX_SSH_TARGET,
        MUX_PORT,
        daemon_only(switch(
            "--upgrade",
            &["-u"],
            "Replace the multiplexer, keeping its sessions",
        )),
        MUX_IDENTITY,
        MUX_SECRET_STDIN,
        MUX_LAYOUT,
        MUX_PROFILE,
        MUX_TITLE,
        MUX_WORKING_DIRECTORY,
        MUX_ENV,
        MUX_RETENTION,
        HELP,
        MUX_VERSION,
    ],
    subcommands: &[
        CommandSpec {
            name: "list",
            about: "List the sessions the multiplexer is holding",
            options: &[
                MUX_JSON,
                MUX_IDS_ONLY,
                MUX_SSH_TARGET,
                MUX_PORT,
                MUX_IDENTITY,
                HELP,
            ],
            ..LEAF
        },
        CommandSpec {
            name: "profiles",
            about: "List profiles available on the local or remote host",
            options: &[MUX_JSON, MUX_SSH_TARGET, MUX_PORT, HELP, MUX_VERSION],
            needs_daemon: true,
            ..LEAF
        },
        CommandSpec {
            name: "create",
            about: "Create a headless shared session",
            options: &[
                MUX_JSON,
                MUX_SECRET_STDIN,
                MUX_LAYOUT,
                MUX_PROFILE,
                MUX_TITLE,
                MUX_WORKING_DIRECTORY,
                MUX_ENV,
                MUX_RETENTION,
                MUX_SSH_TARGET,
                MUX_PORT,
                HELP,
                MUX_VERSION,
            ],
            needs_daemon: true,
            ..LEAF
        },
        CommandSpec {
            name: "stop",
            about: "Stop the multiplexer",
            options: &[
                switch("--force", &["-f"], "Stop even while sessions are running"),
                HELP,
            ],
            needs_daemon: true,
            ..LEAF
        },
        CommandSpec {
            name: "reconnect",
            about: "Open a session in a Zetta window",
            options: &[MUX_IDENTITY, HELP],
            operands: MUX_SESSION,
            ..LEAF
        },
        CommandSpec {
            name: "attach",
            about: "Open a shared remote session through OpenSSH",
            options: &[
                MUX_SSH_TARGET,
                MUX_PORT,
                attached(
                    Attached::Long,
                    option(
                        "--protocol",
                        &["-P"],
                        "What carries the attached session's panes",
                        Values::Words(&["ssh", "zosh"]),
                    ),
                ),
                switch("--keep-alive", &["-k"], "Hold each Zosh pane's link open"),
                MUX_IDENTITY,
                HELP,
            ],
            operands: Values::Each(&[
                Values::Call(Helper::MuxAttach),
                Values::Call(Helper::MuxAttach),
            ]),
            ..LEAF
        },
        CommandSpec {
            name: "resume",
            about: "Restore an encrypted disk session with fresh shells",
            options: &[MUX_IDENTITY, HELP],
            operands: Values::Each(&[Values::Call(Helper::MuxRestorableIds)]),
            needs_daemon: true,
            ..LEAF
        },
        CommandSpec {
            name: "share",
            about: "Let every Zetta process attach a backgrounded session",
            options: MUX_SCOPE_OPTIONS,
            operands: MUX_SESSION,
            needs_daemon: true,
            ..LEAF
        },
        CommandSpec {
            name: "unshare",
            about: "Scope a session back to the window that held it",
            options: MUX_SCOPE_OPTIONS,
            operands: MUX_SESSION,
            needs_daemon: true,
            ..LEAF
        },
        CommandSpec {
            name: "kill",
            about: "End a session and everything running in it",
            options: MUX_SCOPE_OPTIONS,
            operands: MUX_SESSION,
            needs_daemon: true,
            ..LEAF
        },
        CommandSpec {
            name: "forget",
            about: "Remove a session from the catalog without killing it",
            options: MUX_SCOPE_OPTIONS,
            operands: MUX_SESSION,
            needs_daemon: true,
            ..LEAF
        },
    ],
    ..LEAF
};

/// `zmux` is `zetta mux`.
pub(super) static ZMUX: CommandSpec = CommandSpec {
    name: "zmux",
    ..MUX
};
