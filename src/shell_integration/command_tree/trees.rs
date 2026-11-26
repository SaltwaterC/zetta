//! The commands completion is generated for, described once each, and the
//! names each is registered under. `command_tree` owns what a description
//! means; this owns what the commands are.

#[cfg(ssh_clients)]
use super::{Attached, attached};
use super::{
    CommandSpec, Delimiter, Helper, LEAF, OptionSpec, Registration, Values, alone, exits, instead,
    option, switch,
};
use crate::pane::OverlayFontSize;

#[cfg(feature = "zmux")]
mod mux;

const HELP: OptionSpec = exits("--help", &["-h"], "Print help");
const CONFIG: OptionSpec = option(
    "--config",
    &["-c"],
    "Use a configuration file",
    Values::Config,
);
#[cfg(feature = "clipboard")]
const PBOARD: OptionSpec = option(
    "--pboard",
    &["-pboard"],
    "Accepted for compatibility; Zetta has one clipboard",
    Values::Words(&["general", "ruler", "find", "font"]),
);
#[cfg(feature = "clipboard")]
const CLIPBOARD_HELP: OptionSpec = exits("--help", &["-h", "-help"], "Print help");
#[cfg(feature = "zpager")]
const PAGER_WIDTHS: Values = Values::Words(&["40", "60", "80", "100", "120"]);

const HTTP_ROOT: OptionSpec = option("--root", &["-r"], "Directory to serve", Values::Directories);
const HTTP_PORT: OptionSpec = option("--port", &["-p"], "TCP port", Values::Any);
const HTTP_DOTFILES: OptionSpec = switch("--serve-dotfiles", &["-d"], "Serve dot-prefixed names");
const HTTP_ALLOW_HOST: OptionSpec = OptionSpec {
    repeatable: true,
    ..option(
        "--allow-host",
        &["-a"],
        "Also answer this host name or address",
        Values::Any,
    )
};
const HTTP_ALLOW_ANY_HOST: OptionSpec =
    switch("--allow-any-host", &["-A"], "Answer every Host name");

const HTTP_SERVER: CommandSpec = CommandSpec {
    name: "server",
    about: "Serve static files over HTTP",
    options: &[
        HTTP_ROOT,
        HTTP_PORT,
        HTTP_DOTFILES,
        HTTP_ALLOW_HOST,
        HTTP_ALLOW_ANY_HOST,
        HELP,
    ],
    ..LEAF
};

static ZHTTP: CommandSpec = CommandSpec {
    name: "zhttp",
    about: "Serve static files over HTTP",
    options: &[HELP],
    subcommands: &[HTTP_SERVER],
    ..LEAF
};

#[cfg(feature = "zpager")]
static ZPAGER: CommandSpec = CommandSpec {
    name: "zpager",
    about: "View diff text in a scrollable terminal view",
    options: &[
        option(
            "--columns",
            &["-c"],
            "Minimum terminal width per side before stacking",
            PAGER_WIDTHS,
        ),
        option("--width", &["-w"], "Cap each pane's width", PAGER_WIDTHS),
        switch(
            "--no-color",
            &["-n"],
            "Disable syntax colors in the terminal view",
        ),
        exits("--help", &["-h"], "Print this help"),
    ],
    ..LEAF
};

const APP_NAME: OptionSpec = option("--app-name", &["-a"], "Application name", Values::Any);
const ICON: OptionSpec = option(
    "--icon",
    &["-i"],
    "Image to show with the notification",
    Values::Files,
);
const SOUND: OptionSpec = option(
    "--sound",
    &["-s"],
    "Sound name",
    Values::Call(Helper::SoundNames),
);
const TIMEOUT: OptionSpec = option(
    "--timeout",
    &["-t"],
    "default, never, or milliseconds",
    Values::Words(&["default", "never"]),
);

const NOTIFY: CommandSpec = CommandSpec {
    name: "notify",
    about: "Show a desktop notification",
    options: &[APP_NAME, ICON, SOUND, TIMEOUT, HELP],
    subcommands: &[CommandSpec {
        name: "cleanup",
        about: "Reap stale desktop notification workers",
        options: &[
            switch(
                "--dry-run",
                &["-n"],
                "List stale workers without terminating them",
            ),
            HELP,
        ],
        ..LEAF
    }],
    operands: Values::Any,
    ..LEAF
};

/// `zntfy` is `zetta notify`.
pub(super) static ZNTFY: CommandSpec = CommandSpec {
    name: "zntfy",
    ..NOTIFY
};

#[cfg(feature = "clipboard")]
const COPY: CommandSpec = CommandSpec {
    name: "copy",
    about: "Copy standard input to the clipboard",
    options: &[PBOARD, CLIPBOARD_HELP],
    ..LEAF
};

#[cfg(feature = "clipboard")]
const PASTE: CommandSpec = CommandSpec {
    name: "paste",
    about: "Print the clipboard's contents",
    options: &[
        PBOARD,
        option(
            "--prefer",
            &["-prefer", "--Prefer", "-Prefer"],
            "Accepted for compatibility; Zetta stores plain text",
            Values::Words(&["txt", "rtf", "ps"]),
        ),
        CLIPBOARD_HELP,
    ],
    ..LEAF
};

#[cfg(feature = "clipboard")]
static ZCOPY: CommandSpec = CommandSpec {
    name: "zcopy",
    ..COPY
};

#[cfg(feature = "clipboard")]
static ZPASTE: CommandSpec = CommandSpec {
    name: "zpaste",
    ..PASTE
};

static ZETTA: CommandSpec = CommandSpec {
    name: "zetta",
    about: "",
    options: &[
        HELP,
        exits("--version", &["-v"], "Print version"),
        CONFIG,
        option("--keymap", &["-k"], "Use a keymap file", Values::Files),
        option("--profile", &["-p"], "Select a profile", PROFILE_NAMES),
        option(
            "--split",
            &["-s"],
            "Apply a configured pane split template",
            Values::ZettaLines("splits"),
        ),
        switch(
            "--replace-pane",
            &["-r"],
            "Replace the active pane in a running process",
        ),
        option(
            "--theme",
            &["-t"],
            "Non-persistently override the profile theme",
            PROFILE_THEMES,
        ),
        option(
            "--geometry",
            &["-g"],
            "Open the first pane at COLUMNSxROWS",
            Values::Any,
        ),
        option(
            "--cell-size",
            &["-C"],
            "Set cell WIDTHxHEIGHT in backing pixels",
            Values::Any,
        ),
        #[cfg(feature = "zmux")]
        switch(
            "--no-mux",
            &["-n"],
            "Keep background sessions in this process for this launch",
        ),
        switch(
            "--new-window",
            &["-w"],
            "Open a fresh OS window without resuming a dormant session",
        ),
        option(
            "--command",
            &["-e"],
            "Open a tab and run COMMAND; remaining arguments go to the child",
            Values::CommandLine,
        ),
    ],
    subcommands: &[
        PROFILE,
        #[cfg(feature = "zosh-client")]
        MOSH,
        CommandSpec {
            name: "http",
            about: "Serve static files over HTTP",
            options: &[HELP],
            subcommands: &[CommandSpec {
                options: &[
                    HTTP_ROOT,
                    HTTP_PORT,
                    CONFIG,
                    HTTP_DOTFILES,
                    HTTP_ALLOW_HOST,
                    HTTP_ALLOW_ANY_HOST,
                    HELP,
                ],
                ..HTTP_SERVER
            }],
            ..LEAF
        },
        NOTIFY,
        CommandSpec {
            name: "attention",
            about: "Mark the originating tab as needing attention",
            options: &[
                switch("--notify", &["-n"], "Also show a desktop notification"),
                APP_NAME,
                ICON,
                SOUND,
                TIMEOUT,
                HELP,
            ],
            operands: Values::Any,
            ..LEAF
        },
        #[cfg(feature = "clipboard")]
        COPY,
        #[cfg(feature = "clipboard")]
        PASTE,
        BENCHMARK,
        CommandSpec {
            name: "terminal-size",
            about: "Print the current terminal size",
            options: &[
                switch("--json", &["-j"], "Print machine-readable JSON"),
                switch("--resize", &["-r"], "Resize the current pane"),
                option(
                    "--columns",
                    &["-c"],
                    "Set pane width in columns",
                    Values::Any,
                ),
                option("--rows", &["-R"], "Set pane height in rows", Values::Any),
                HELP,
            ],
            ..LEAF
        },
        CommandSpec {
            name: "edit",
            about: "Edit files with EDITOR or Zetta vi",
            options: &[
                switch(
                    "--delete-after",
                    &["-d"],
                    "Delete a managed buffer after editing",
                ),
                HELP,
            ],
            operands: Values::Files,
            ..LEAF
        },
        CommandSpec {
            name: "init",
            about: "Generate shell integration",
            options: &[
                switch("--completions", &["-c"], "Print only the completions"),
                HELP,
            ],
            operands: Values::Words(&["bash", "fish", "powershell", "pwsh", "zsh"]),
            ..LEAF
        },
        SERIAL,
        CommandSpec {
            name: "splits",
            about: "List configured pane split templates",
            options: &[HELP],
            ..LEAF
        },
        #[cfg(feature = "zmux")]
        mux::MUX,
        PANE,
        PROJECT,
        CMD,
        THEME,
        TABICON,
        VI,
        TFTP,
        #[cfg(feature = "worktree")]
        WT,
        CommandSpec {
            name: "overlay",
            about: "Non-persistently show text over the active pane",
            options: &[
                option("--text", &["-t"], "Overlay text", Values::Any),
                option(
                    "--size",
                    &["-s"],
                    "Font size",
                    Values::Words(&OverlayFontSize::CLI_NAMES),
                ),
                option("--opacity", &["-o"], "Opacity percentage", Values::Any),
                option("--color", &["-c"], "Text color", Values::OverlayColors),
                // The parser rejects `--reset` with text or any style option.
                alone("--reset", &["-r"], "Remove the overlay"),
                HELP,
            ],
            operands: Values::Each(&[Values::Any]),
            ..LEAF
        },
    ],
    ..LEAF
};

const PROFILE_NAMES: Values = Values::ConfiguredLines("profile list");
const PROFILE_THEMES: Values = Values::ConfiguredLines("profile themes");
const PROFILE_ICONS: Values = Values::Words(&["auto", "zetta", "bash", "zsh", "fish"]);

/// A `zetta profile` operation, which `profile`'s own `--config` may precede.
const PROFILE_OPERATION: CommandSpec = CommandSpec {
    options: &[CONFIG, HELP],
    after_options: true,
    ..LEAF
};

/// `profile theme` and `profile dark-theme`, which differ only in what they
/// set.
macro_rules! profile_theme {
    ($name:literal, $about:literal, $reset:literal) => {
        CommandSpec {
            name: $name,
            about: $about,
            options: &[instead("--reset", &["-r"], $reset), CONFIG, HELP],
            operands: Values::Each(&[PROFILE_NAMES, PROFILE_THEMES]),
            ..PROFILE_OPERATION
        }
    };
}

/// `zetta profile`, which a root `--config` may precede.
const PROFILE: CommandSpec = CommandSpec {
    name: "profile",
    about: "List and manage profiles",
    options: &[CONFIG, HELP],
    subcommands: &[
        CommandSpec {
            name: "list",
            about: "List all resolved profiles",
            ..PROFILE_OPERATION
        },
        CommandSpec {
            name: "themes",
            about: "List available profile themes",
            ..PROFILE_OPERATION
        },
        CommandSpec {
            name: "disable",
            about: "Hide a profile",
            operands: Values::Each(&[PROFILE_NAMES]),
            ..PROFILE_OPERATION
        },
        CommandSpec {
            name: "enable",
            about: "Show a profile",
            operands: Values::Each(&[PROFILE_NAMES]),
            ..PROFILE_OPERATION
        },
        profile_theme!(
            "theme",
            "Set or reset a profile theme",
            "Remove the profile theme override"
        ),
        profile_theme!(
            "dark-theme",
            "Set or reset a profile dark theme",
            "Remove the profile dark theme override"
        ),
        CommandSpec {
            name: "icon",
            about: "Set or reset a profile icon",
            options: &[
                instead(
                    "--reset",
                    &["-r"],
                    "Restore automatic profile icon inference",
                ),
                CONFIG,
                HELP,
            ],
            operands: Values::Each(&[PROFILE_NAMES, PROFILE_ICONS]),
            ..PROFILE_OPERATION
        },
        CommandSpec {
            name: "default",
            about: "Set the default profile",
            operands: Values::Each(&[PROFILE_NAMES]),
            ..PROFILE_OPERATION
        },
        CommandSpec {
            name: "add",
            about: "Add a custom profile",
            options: &[
                option("--program", &["-p"], "Program to launch", Values::Files),
                OptionSpec {
                    repeatable: true,
                    ..option("--arg", &["-a"], "Program argument", Values::Any)
                },
                option("--theme", &["-t"], "Profile theme", PROFILE_THEMES),
                option(
                    "--dark-theme",
                    &["-d"],
                    "Profile dark theme",
                    PROFILE_THEMES,
                ),
                option("--icon", &["-i"], "Profile icon override", PROFILE_ICONS),
                CONFIG,
                HELP,
            ],
            // The new profile's name.
            operands: Values::Each(&[Values::Any]),
            ..PROFILE_OPERATION
        },
        CommandSpec {
            name: "remove",
            about: "Remove a custom profile",
            operands: Values::Each(&[PROFILE_NAMES]),
            ..PROFILE_OPERATION
        },
    ],
    after_options: true,
    ..LEAF
};

/// `zetta mosh`, which `zosh` is too. Stock Mosh's short-only switches (`-a`,
/// `-n`, `-4`, `-6`) are left out: each has a long spelling here, and an
/// option the tree does not know is read as a switch anyway.
#[cfg(feature = "zosh-client")]
const MOSH: CommandSpec = CommandSpec {
    name: "mosh",
    about: "Run an interactive Mosh session",
    options: &[
        attached(
            Attached::Long,
            option(
                "--client",
                &[],
                "Use a specific local Mosh client",
                Values::Files,
            ),
        ),
        attached(
            Attached::Long,
            option(
                "--server",
                &[],
                "Use a specific remote Mosh server",
                Values::Any,
            ),
        ),
        attached(
            Attached::Long,
            option(
                "--predict",
                &[],
                "Prediction mode",
                Values::Words(&["adaptive", "always", "never", "experimental"]),
            ),
        ),
        switch(
            "--predict-overwrite",
            &["-o"],
            "Allow predictive overwrites",
        ),
        switch(
            "--no-predict-overwrite",
            &[],
            "Do not overwrite predictions",
        ),
        attached(
            Attached::Every,
            switch(
                "--keep-alive",
                &["-k"],
                "Send a keep-alive packet every 500 ms",
            ),
        ),
        attached(
            Attached::Every,
            switch(
                "--scrollback",
                &["-s"],
                "Keep the output that scrolls off the screen (default)",
            ),
        ),
        switch(
            "--no-scrollback",
            &[],
            "Keep only what is on the screen, as stock Mosh does",
        ),
        switch(
            "--forward-agent",
            &[],
            "Forward SSH-agent connections to Zosh hosts",
        ),
        switch("--no-forward-agent", &[], "Disable SSH-agent forwarding"),
        attached(
            Attached::Long,
            option(
                "--family",
                &[],
                "Address family",
                Values::Words(&[
                    "prefer-inet",
                    "prefer-inet6",
                    "inet",
                    "inet6",
                    "auto",
                    "all",
                ]),
            ),
        ),
        attached(
            Attached::Joined,
            option("--port", &["-p"], "Server UDP port or range", Values::Any),
        ),
        attached(
            Attached::Long,
            option(
                "--bind-server",
                &[],
                "Ask the server to reply from an address",
                Values::Words(&["ssh", "any"]),
            ),
        ),
        attached(
            Attached::Long,
            option("--ssh", &[], "SSH command", Values::Any),
        ),
        switch("--ssh-pty", &[], "Request an SSH PTY"),
        switch("--no-ssh-pty", &[], "Do not request an SSH PTY"),
        switch("--init", &[], "Initialize the terminal"),
        switch("--no-init", &[], "Preserve the terminal"),
        switch("--local", &[], "Use locally discovered addressing"),
        attached(
            Attached::Long,
            option(
                "--experimental-remote-ip",
                &[],
                "Address discovery mode",
                Values::Words(&["local", "remote", "proxy"]),
            ),
        ),
        HELP,
        exits("--version", &["-V"], "Print version"),
    ],
    // The host, after which the words are the remote command.
    operands: Values::Each(&[Values::Call(Helper::SshTargets)]),
    options_first: true,
    ..LEAF
};

#[cfg(feature = "zosh-client")]
pub(super) static ZOSH: CommandSpec = CommandSpec {
    name: "zosh",
    ..MOSH
};

/// What `zetta pane` and `zetta pane wait` run, after `--`.
const PANE_COMMAND: Delimiter = Delimiter {
    before: Values::Nothing,
    about: "Execute the remaining values as exact argv",
    after: Values::CommandLine,
};

const PANE: CommandSpec = CommandSpec {
    name: "pane",
    about: "Run a command in a pane",
    options: &[
        option(
            "--direction",
            &["-d"],
            "Direction for a new split",
            Values::Words(&["left", "right", "up", "down"]),
        ),
        option(
            "--label",
            &["-l"],
            "Label for a new split pane",
            Values::Any,
        ),
        option(
            "--pane",
            &["-p"],
            "Target pane label",
            Values::ZettaLines("pane --list"),
        ),
        option(
            "--overlay",
            &["-o"],
            "Overlay text for a new split pane",
            Values::Any,
        ),
        option(
            "--overlay-size",
            &["-S"],
            "Overlay font size",
            Values::Words(&OverlayFontSize::CLI_NAMES),
        ),
        option(
            "--overlay-opacity",
            &["-O"],
            "Overlay opacity percentage",
            Values::Any,
        ),
        option(
            "--overlay-color",
            &["-c"],
            "Overlay text color",
            Values::OverlayColors,
        ),
        switch("--stack", &["-s"], "Run in a stacked task pane"),
        alone("--list", &["-L"], "List pane labels"),
        HELP,
    ],
    subcommands: &[CommandSpec {
        name: "wait",
        about: "Wait for pane commands before running a command",
        options: &[
            switch(
                "--allow-failure",
                &["-a"],
                "Continue after a dependency fails",
            ),
            HELP,
        ],
        operands: Values::Delimited(&Delimiter {
            before: Values::Each(&[Values::Call(Helper::PaneLabelList)]),
            ..PANE_COMMAND
        }),
        ..LEAF
    }],
    operands: Values::Delimited(&PANE_COMMAND),
    ..LEAF
};

const CMD: CommandSpec = CommandSpec {
    name: "cmd",
    about: "Run a registered project command in the active pane",
    options: &[
        alone("--list", &["-l"], "List registered project command names"),
        HELP,
    ],
    operands: Values::Delimited(&Delimiter {
        before: Values::Each(&[Values::ZettaLines("cmd --list")]),
        about: "Pass the remaining values as shell-quoted arguments",
        after: Values::Any,
    }),
    ..LEAF
};

const PROJECT_PATH: OptionSpec = option(
    "--path",
    &["-p"],
    "Project directory",
    Values::ZettaLines("project list"),
);
const PROJECT: CommandSpec = CommandSpec {
    name: "project",
    about: "List and manage projects",
    options: &[HELP],
    subcommands: &[
        CommandSpec {
            name: "add",
            about: "Register a project",
            options: &[
                option("--path", &["-p"], "Project directory", Values::Directories),
                HELP,
            ],
            operands: Values::Each(&[Values::Directories]),
            ..LEAF
        },
        CommandSpec {
            name: "list",
            about: "List registered projects",
            options: &[HELP],
            ..LEAF
        },
        CommandSpec {
            name: "remove",
            about: "Unregister a project",
            options: &[PROJECT_PATH, HELP],
            operands: Values::Each(&[Values::ZettaLines("project list")]),
            ..LEAF
        },
        CommandSpec {
            name: "open",
            about: "Open a project in a new tab",
            options: &[PROJECT_PATH, HELP],
            operands: Values::Each(&[Values::ZettaLines("project list")]),
            ..LEAF
        },
    ],
    ..LEAF
};

/// `theme pane` and `theme tab`, which differ only in the themes they list.
macro_rules! theme_scope {
    ($name:literal, $about:literal, $list:literal) => {
        CommandSpec {
            name: $name,
            about: $about,
            options: &[
                option("--theme", &["-t"], "Theme name", Values::ZettaLines($list)),
                instead("--reset", &["-r"], "Restore the configured theme"),
                alone("--list", &["-l"], "Print theme names"),
                HELP,
            ],
            operands: Values::Each(&[Values::ZettaLines($list)]),
            ..LEAF
        }
    };
}

const THEME: CommandSpec = CommandSpec {
    name: "theme",
    about: "Non-persistently change the active pane or tab's theme",
    options: &[HELP],
    subcommands: &[
        theme_scope!(
            "pane",
            "Change the active pane's theme",
            "theme pane --list"
        ),
        theme_scope!("tab", "Change the active tab's theme", "theme tab --list"),
    ],
    ..LEAF
};

const TAB_ICONS: Values = Values::ZettaLines("tabicon --list");
const TABICON: CommandSpec = CommandSpec {
    name: "tabicon",
    about: "Set the active tab icon",
    options: &[
        option("--icon", &["-i"], "Icon name", TAB_ICONS),
        instead(
            "--reset",
            &["-r"],
            "Restore the configured project or application icon",
        ),
        switch("--queue", &["-q"], "Apply after the running command"),
        alone("--list", &["-l"], "Print built-in icon names"),
        HELP,
    ],
    operands: Values::Each(&[TAB_ICONS]),
    ..LEAF
};

/// `zetta vi`, which `vi` and `zvi` complete as too.
const VI: CommandSpec = CommandSpec {
    name: "vi",
    about: "Edit files with Zetta's built-in vi",
    options: &[HELP],
    operands: Values::Files,
    ..LEAF
};

static ZVI: CommandSpec = CommandSpec { name: "zvi", ..VI };

const TFTP_PORT: OptionSpec = option("--port", &["-p"], "Server port", Values::Any);
const TFTP_GET: CommandSpec = CommandSpec {
    name: "get",
    about: "Download REMOTE, optionally naming the LOCAL output file",
    options: &[TFTP_PORT, HELP],
    // HOST REMOTE [LOCAL]
    operands: Values::Each(&[Values::Any, Values::Any, Values::Files]),
    ..LEAF
};
const TFTP_PUT: CommandSpec = CommandSpec {
    name: "put",
    about: "Upload LOCAL, optionally naming the REMOTE file",
    options: &[TFTP_PORT, HELP],
    // HOST LOCAL [REMOTE]
    operands: Values::Each(&[Values::Any, Values::Files, Values::Any]),
    ..LEAF
};
const TFTP_ROOT: OptionSpec = option("--root", &["-r"], "Directory to serve", Values::Directories);
const TFTP_WRITABLE: OptionSpec = switch(
    "--writable",
    &["-w"],
    "Accept uploads into the served directory",
);
const TFTP_SERVER: CommandSpec = CommandSpec {
    name: "server",
    about: "Serve a directory over TFTP",
    options: &[TFTP_ROOT, TFTP_PORT, TFTP_WRITABLE, HELP],
    ..LEAF
};

static ZTFTP: CommandSpec = CommandSpec {
    name: "ztftp",
    about: "Transfer a file with TFTP",
    options: &[HELP],
    subcommands: &[TFTP_GET, TFTP_PUT, TFTP_SERVER],
    ..LEAF
};

const TFTP: CommandSpec = CommandSpec {
    name: "tftp",
    subcommands: &[
        TFTP_GET,
        TFTP_PUT,
        CommandSpec {
            options: &[TFTP_ROOT, TFTP_PORT, CONFIG, TFTP_WRITABLE, HELP],
            ..TFTP_SERVER
        },
        #[cfg(tftp_setcap)]
        CommandSpec {
            name: "setcap",
            about: "Let the TFTP server bind port 69",
            options: &[
                switch("--check", &["-c"], "Only report whether it can"),
                HELP,
            ],
            ..LEAF
        },
    ],
    ..ZTFTP
};

#[cfg(feature = "worktree")]
const PATH_ONLY: OptionSpec = switch("--path-only", &["-P"], "Print only the worktree path");
#[cfg(feature = "worktree")]
const WT: CommandSpec = CommandSpec {
    name: "wt",
    about: "Create, integrate, synchronize, configure, or abort Git worktrees",
    options: &[HELP],
    subcommands: &[
        CommandSpec {
            name: "new",
            about: "Create a worktree",
            options: &[
                OptionSpec {
                    repeatable: true,
                    ..option(
                        "--copy",
                        &["-c"],
                        "Copy an ignored file into it",
                        Values::Files,
                    )
                },
                PATH_ONLY,
                HELP,
            ],
            operands: Values::Each(&[Values::Any]),
            ..LEAF
        },
        CommandSpec {
            name: "done",
            about: "Integrate and remove the current worktree",
            options: &[PATH_ONLY, HELP],
            ..LEAF
        },
        CommandSpec {
            name: "abort",
            about: "Discard and remove the current worktree",
            options: &[PATH_ONLY, HELP],
            ..LEAF
        },
        CommandSpec {
            name: "status",
            about: "Show worktree state",
            options: &[HELP],
            ..LEAF
        },
        CommandSpec {
            name: "sync",
            about: "Rebase the current worktree onto the source branch",
            options: &[HELP],
            operands: Values::Each(&[Values::Call(Helper::WorktreeCommits)]),
            ..LEAF
        },
        CommandSpec {
            name: "config",
            about: "Install the recommended global Git configuration",
            options: &[HELP],
            ..LEAF
        },
    ],
    ..LEAF
};

#[cfg(feature = "worktree")]
static ZWT: CommandSpec = CommandSpec { name: "zwt", ..WT };

const BENCHMARK: CommandSpec = CommandSpec {
    name: "benchmark",
    about: "Profile terminal rendering",
    options: &[
        option(
            "--profile-report",
            &["-r"],
            "Write a JSON report",
            Values::Files,
        ),
        option(
            "--profile-duration",
            &["-d"],
            "Seconds to profile",
            Values::Any,
        ),
        switch("--profile-pane-stress", &["-s"], "Profile with many panes"),
        switch(
            "--profile-background-stress",
            &["-b"],
            "Checkerboard backgrounds",
        ),
        switch("--profile-sparse-updates", &["-u"], "Sparse cell updates"),
        switch(
            "--profile-alt-screen-scroll",
            &["-a"],
            "Alternate-screen scrolling",
        ),
        switch(
            "--profile-external-terminal",
            &["-x"],
            "Run the workload in this terminal",
        ),
        HELP,
    ],
    subcommands: &[CommandSpec {
        name: "output",
        about: "Measure output throughput",
        options: &[
            option("--size", &["-s"], "MiB to print", Values::Any),
            option(
                "--output-type",
                &["-t"],
                "Kind of lines",
                Values::Words(&["repeated", "unique"]),
            ),
            HELP,
        ],
        ..LEAF
    }],
    ..LEAF
};

const SERIAL: CommandSpec = CommandSpec {
    name: "serial",
    about: "List or connect to serial devices",
    options: &[HELP],
    subcommands: &[
        CommandSpec {
            name: "console",
            about: "Connect to a serial device",
            options: &[
                option(
                    "--device",
                    &["-d"],
                    "Serial device",
                    Values::ZettaLines("serial list"),
                ),
                option("--baud-rate", &["-b"], "Baud rate", Values::Any),
                option(
                    "--data-bits",
                    &["-D"],
                    "Data bits",
                    Values::Words(&["5", "6", "7", "8"]),
                ),
                option(
                    "--parity",
                    &["-p"],
                    "Parity",
                    Values::Words(&["none", "odd", "even"]),
                ),
                option(
                    "--stop-bits",
                    &["-s"],
                    "Stop bits",
                    Values::Words(&["1", "2"]),
                ),
                option(
                    "--flow-control",
                    &["-f"],
                    "Flow control",
                    Values::Words(&["none", "software", "hardware"]),
                ),
                HELP,
            ],
            ..LEAF
        },
        CommandSpec {
            name: "list",
            about: "List serial devices",
            options: &[HELP],
            ..LEAF
        },
    ],
    ..LEAF
};

pub(super) static REGISTRATIONS: &[Registration] = &[
    Registration {
        command: "zetta",
        tree: &ZETTA,
        outside_macos: false,
        native: true,
    },
    // `zvi` is `zetta vi`, which the startup scripts also register `vi` for
    // where there is no `vi`.
    Registration {
        command: "zvi",
        tree: &ZVI,
        outside_macos: false,
        native: false,
    },
    // And `zosh` is `zetta mosh`, likewise registered for a missing `mosh`.
    #[cfg(feature = "zosh-client")]
    Registration {
        command: "zosh",
        tree: &ZOSH,
        outside_macos: false,
        native: true,
    },
    #[cfg(feature = "zmux")]
    Registration {
        command: "zmux",
        tree: &mux::ZMUX,
        outside_macos: false,
        native: true,
    },
    Registration {
        command: "ztftp",
        tree: &ZTFTP,
        outside_macos: false,
        native: false,
    },
    #[cfg(feature = "worktree")]
    Registration {
        command: "zwt",
        tree: &ZWT,
        outside_macos: false,
        native: false,
    },
    Registration {
        command: "zhttp",
        tree: &ZHTTP,
        outside_macos: false,
        native: false,
    },
    #[cfg(feature = "zpager")]
    Registration {
        command: "zpager",
        tree: &ZPAGER,
        outside_macos: false,
        native: false,
    },
    Registration {
        command: "zntfy",
        tree: &ZNTFY,
        outside_macos: false,
        native: false,
    },
    #[cfg(feature = "clipboard")]
    Registration {
        command: "zcopy",
        tree: &ZCOPY,
        outside_macos: false,
        native: false,
    },
    #[cfg(feature = "clipboard")]
    Registration {
        command: "zpaste",
        tree: &ZPASTE,
        outside_macos: false,
        native: false,
    },
    #[cfg(feature = "clipboard")]
    Registration {
        command: "pbcopy",
        tree: &ZCOPY,
        outside_macos: true,
        native: false,
    },
    #[cfg(feature = "clipboard")]
    Registration {
        command: "pbpaste",
        tree: &ZPASTE,
        outside_macos: true,
        native: false,
    },
];
