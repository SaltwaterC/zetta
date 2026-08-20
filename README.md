# Zetta

Zetta is a standalone, cross-platform terminal emulator built with Rust,
GPUI, and Zed's terminal engine (alacritty_terminal). It combines a
GPU-rendered terminal with the tabs, panes, profiles, and configurable
shortcuts expected from a complete terminal application.

Zetta currently supports Linux, Windows, and macOS. All platforms are targets
for active development.

## Highlights

- Tabs with renameable titles, selectable icons, session-only pinned tabs, and tab reordering, recursive pane splits and
  pane rotation, pane templates, pane minimization, input broadcasting, and
  modal keyboard or mouse pane resizing and moving
- Automatically detected shells, including Homebrew-installed shells on
  macOS and Linux, plus first-class WSL/MSYS2 profiles with working directory
  tracking
- Detachable background sessions, held by the `zmux` multiplexer so they
  outlive Zetta itself,
  with retained diagnostics for unexpected terminal exits
- Shareable tabs: offer a tab that is still on screen and join it from another
  Zetta window, with both driving the same panes
- Optional compact mode moves tabs into the title bar, keeps Menu, Profile,
  Broadcast, and Silent available, and preserves responsive tab sizing
- Open paths or the complete pane scrollback in `$EDITOR`, with built-in vi as
  the fallback
- Native command, path, and SSH-alias completion in a multi-command prompt
- Stacked command panes that retain each command's PTY, output, and exit status
- Typed settings and keymap editor, per-profile themes and icons, and
  installable Zed themes
- Registered projects with repository-local themes, environment, profiles,
  pane templates, initial layouts, live command-palette scoping, and a
  command-palette entry that opens each registered project
- Serial consoles plus built-in HTTP and TFTP tools, usable from panes or the
  CLI
- Git worktree workflow with `zetta wt new`, `zetta wt done`, status, and
  conflict-resolution helpers; shell integration adds the `zwt` directory wrapper
- In-memory tab attention badges from `zetta attention`, with optional
  cross-platform desktop notifications that click back to the issuing visible
  tab
- Transient process-wide and tab-scoped Silent modes that suppress terminal
  bells and notification sounds while preserving notification content,
  actions, and attention badges; process-wide silence also follows system Do
  Not Disturb when available
- A small syntax-highlighted built-in [vi editor](https://github.com/SaltwaterC/busy-v),
  available as `zetta vi`, unconditionally as `zvi`, and conditionally as `vi`
  through shell integration;
  see the [supported grammars](docs/usage.md#built-in-vi-syntax-highlighting)

## Quick start

Initialize the Zed submodule, then run Zetta with the pinned Rust toolchain:

```sh
git submodule update --init
cargo run
```

Linux defaults to Wayland. Use `make build X11=1` to include the X11 backend
as well. Linux system dependencies and platform-specific build and desktop
installation instructions are in the [installation guide](docs/installation.md).
`make build` uses an incremental development build; use `RELEASE=1` for an
optimized release build.

Corporate or otherwise restricted deployments can omit the serial console,
network tools, and desktop notification support at build time. For example, `make
build SERIAL=0 HTTP=0 TFTP=0 NOTIFY=0` produces a terminal-only build.
`TFTP_SERVER=0` and `TFTP_CLIENT=0` control the two TFTP components
independently. Set `SYNTAX_HIGHLIGHTING=0` to omit the optional bundled
Tree-sitter grammar set from the vi editor. The flags also accept `false`,
`no`, or `off`.

The tools are available directly when enabled: `zetta serial console --device
PATH`, `zetta http server`, `zetta tftp server`, and `zetta notify`. From a
Zetta terminal, `zetta attention [OPTIONS] [SUMMARY] [BODY]` marks the
originating tab with a badge; it works in badge-only builds, defaults to
`Attention required`, and adds a desktop notification when `--notify` is used.
Notifications issued from a Zetta terminal can be clicked to activate their
issuing window and visible tab; closed, dormant, or background-only tabs are
left alone. Plain `zetta notify` outside Zetta remains fire-and-forget.
Packaged macOS app builds support click routing; unbundled development builds
still display notifications but do not route clicks.
From a Zetta pane, `zetta tabicon ICON` sets a per-tab icon override on the
active tab (`none` explicitly hides its icon); it is kept with the logical tab
but is not written to user or project configuration. `zetta panetheme THEME`
non-persistently changes the active pane's theme
(`zetta panetheme --reset` restores the profile's configured theme).
`zetta overlay TEXT` non-persistently shows text over the active pane's
terminal content, with `--size`, `--opacity`, and `--color` options
(`--color` accepts the named presets `black`, `white`, `gray`, `red`,
`orange`, `yellow`, `green`, `cyan`, `blue`, `purple`, `magenta`, and `pink`,
as well as the existing hex formats; `zetta overlay --reset` clears it).
Shell integration completes the current serial-device, tab-icon, pane-theme,
and command-pane label lists dynamically. See [Serial and network tools](docs/tools.md)
for flags and safety notes.

Use `zetta --replace-pane --split NAME` or
`zetta --replace-pane --profile PROFILE` to replace the active pane in a
running process; the command falls back to the normal new-window launch when
no process accepts it. See [Using Zetta](docs/usage.md) for details.

Run an exact command in an existing or new pane with `zetta pane`:

```sh
zetta pane --direction right --label api -- npm run dev
zetta pane --direction right --label api --overlay API -- npm run dev
zetta pane --pane api -- make test
zetta pane --pane api --stack -- tail -f server.log
zetta pane --list
```

`--direction` creates a split relative to the active pane (`up`/`down` are
horizontal and `left`/`right` are vertical). Without it, the command targets
the active pane or the exact, case-sensitive label supplied by `--pane`;
`--stack` uses a task-backed PTY. Arguments after `--` are preserved as
individual argv values. This command requires an accepting Zetta process and
reports an error instead of opening a new window when none is available.
New split panes can show an overlay with `--overlay TEXT`; use
`--overlay-size`, `--overlay-opacity`, and `--overlay-color` to style it.

Manage persistent profiles without opening the settings UI with
`zetta profile list`, `zetta profile themes`, and the profile mutation
commands documented in the [configuration guide](docs/configuration.md).
Changes are validated before being saved and request a best-effort live reload
for a Zetta process using the same configuration file.

Register repository-local configuration with `zetta project add`, then place
project settings in `.zetta/config.json`. `zetta project open`, `list`, and
`remove` manage the separate project registry; removal preserves the repository
file. Native repositories with an unregistered `.zetta/config.json` are
detected asynchronously and offered in the UI. The Settings **Projects** tab
builds a project's configuration with typed controls, pane-template editor
included, and leaves anything set to *Inherit* out of the file. See
[Projects](docs/configuration.md#projects) for supported fields, WSL behavior,
and the template-command trust boundary.

Use `zetta wt new NAME`, `zetta wt done`, `zetta wt status`, and
`zetta wt rerere` for the Git worktree workflow. The direct commands never
change the caller's directory; generated shell integration provides `zwt new`
and `zwt done` wrappers that enter the resulting worktree, including paths with
spaces and nested names. `zetta wt new` also recursively initializes source
submodules at their pinned commits, reusing initialized source checkouts as
local object references and falling back to configured submodule remotes when a
source checkout is unavailable. Failed initialization cleans up the partial
worktree, branch, and metadata. Repeatable `--copy PATH` (or `-c PATH`) options
copy relative files, directories, and symlinks from the source worktree into the
same locations in the new worktree. Copy-on-write cloning is used when supported,
with a regular-copy fallback; invalid, overlapping, or already-existing paths are
rejected. Failed initialization or copying cleans up the partial worktree, branch,
and metadata. See [Using Zetta](docs/usage.md#git-worktrees) for configuration
and safety details. Zetta detects linked worktrees from each pane's
interactive-shell directory, preserving nested names such as `feature/api`;
the active pane supplies the automatic title. A successful `new NAME` records
the originating tab's worktree title, and only a successful `done` clears that
record. Terminal-side title requests remain available for ordinary tabs, but
are masked while a worktree title is active. A live detected worktree name can
replace the seed when the shell reports a different linked worktree. Manual
renames take precedence over the worktree title, and an empty manual rename
reveals it again.
The Git operations and `--path-only` output do not depend on Zetta being
available. `wt status` also reports whether the current `HEAD` contains
submodules, lists detected nested submodule paths, and reports native
copy-on-write availability for the current worktree and resolved `wt.root`.
`wt new` emits phase progress on stderr while preserving its normal and
`--path-only` stdout.

## Multi-command prompt

Press `Ctrl-Shift-M` and enter a command such as:

```sh
ssh {{dev,prod}}-{{eu,us}}.example.com
```

Zetta expands the Cartesian product, tiles the active pane, and runs one
command in each new pane. The prompt completes executables from `PATH`, paths
relative to the active pane, and SSH aliases from `~/.ssh/config`; use `Tab`
and `Shift-Tab` to cycle completions.

## Stacked command panes

Press `Alt-Shift-N` (or `Cmd-Shift-N` on macOS) to run one command in a
stacked entry inside the active pane. The command inherits that pane's
profile and current working directory. Its PTY remains available for
interactive input, including `Ctrl-C`, while compact rows above the selected
terminal show every other command and its running or completed status.

Use `Alt-Shift-[` / `Alt-Shift-]` (or the corresponding `Cmd` shortcuts on
macOS) to cycle through the host terminal and stacked commands. Linux also
accepts the normalized `Alt-{` / `Alt-}` events generated by shifted bracket
keys. Completed commands remain selectable with their output and numeric exit
code until the row's close control or the close shortcut is used. Opening
another command while one is selected adds it to the same host pane; the host
pane controls are hidden while stacking is enabled.

## Built with Codex and GPT-5.6

Zetta was developed using Codex with GPT-5.6. Codex accelerated the project
from an experimental question—whether Zed's terminal could become a standalone
application—into a working cross-platform terminal emulator in a short
development cycle. It implemented the application, the local terminal and
platform forks, tests, documentation, and iterative fixes through the Codex
TUI.

The project was self-hosted early: development continued inside Zetta using the
Codex TUI before the first commit was made. That provided a practical feedback
loop in which terminal interaction, rendering, panes, profiles, and
cross-platform behavior could be exercised while the application itself was
being built.

The key product and engineering decisions remained human-led. These included
the cross-platform terminal workflow, feature priorities, interaction design,
application architecture, testing strategy, and the boundaries between Zetta's
code, its maintained forks, and upstream Zed. Codex handled implementation and
rapid iteration; decades of day-to-day terminal use guided what should be
built, how it should behave, and when the result was good enough.

## Documentation

- [Installation](docs/installation.md): build requirements and platform
  integration
- [Using Zetta](docs/usage.md): tabs, panes, search, shortcuts, pane templates,
  and Git worktrees
- [Configuration](docs/configuration.md): settings, projects, profiles,
  keymaps, fonts, and themes
- [Background sessions](docs/background-sessions.md): detach, share, protect,
  inspect, and reconnect sessions
- [Compatibility versioning](docs/versioning.md): executable, protocol, and
  persisted-format version markers
- [Shell integration](docs/shell-integration.md): command completion and the
  `zvi`/`zwt`/`ztftp`/`zntfy`/`vi` shortcuts
- [Serial and network tools](docs/tools.md): serial consoles, HTTP and TFTP
  servers, the TFTP client, and desktop notifications
- [Performance profiling](docs/performance.md): overlays, automated reports,
  stress workloads, and diagnostics

Use [`config.example.json`](config.example.json) and
[`keymap.example.json`](keymap.example.json) as starting points for local
customization.

## Security notes

Two areas are worth knowing about before you rely on them.

The built-in HTTP and TFTP servers listen on `0.0.0.0` and have no
authentication or encryption, so every host that can route to the port can read
the served directory. That is deliberate — their purpose is handing files to
another device — but it means the GUI **Start HTTP server** and **Start TFTP
server** actions expose the active pane's working directory to the network in
one step. TFTP uploads are refused unless you pass `zetta tftp server
--writable`, and the GUI action is always read only. Restrict access with a
firewall on untrusted networks; see [Serial and network tools](docs/tools.md).

Background-session protection stores a salted Argon2id verifier in memory while
the session is live. Disk-retained sessions keep that verifier, their commands,
titles, and directories only inside age-encrypted records; the cleartext
catalog contains no protected session details. Protected sessions remain
unreachable over the process control socket — the endpoint token cannot
reattach one, modify one, or confirm one exists.

It rests on one assumption: that other processes running as your user cannot
read Zetta's memory. On Linux that means `kernel.yama.ptrace_scope` must be `1`
or higher, since a detached session's terminals are PTYs whose file descriptors
belong to the Zetta process. Verify this before relying on protection for a
session running a privileged shell. See
[Background sessions](docs/background-sessions.md#the-prerequisite-process-memory-must-be-protected).

## Design philosophy

Zetta favors useful conventions and a consistent experience across platforms,
while retaining configuration where users' established terminal muscle memory
differs. It aims to work out of the box, even when that means bundling assets
such as the MesloLGS NF font family.

The project's terminal-view fork retains Zed's GPU renderer and terminal
interaction without bringing along the rest of the editor.

The name combines Zeta and tty—though it also happens to describe the size of
some Rust binaries.

## Licensing

Zetta source code is licensed primarily under GPL-3.0-or-later, with
Apache-2.0 components where marked, matching Zed's licensing model:

- [GNU General Public License v3.0](LICENSE-GPL)
- [Apache License 2.0](LICENSE-APACHE)

Copyright 2026 Ștefan Rusu. Portions derived from Zed are copyright
2022–2025 Zed Industries, Inc.

Zetta is an independent project and is not affiliated with Zed Industries,
Inc.
