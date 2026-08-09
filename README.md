# Zetta

Zetta is a standalone, cross-platform terminal emulator built with Rust,
GPUI, and Zed's terminal engine (alacritty_terminal). It combines a
GPU-rendered terminal with the tabs, panes, profiles, and configurable
shortcuts expected from a complete terminal application.

Zetta currently supports Linux, Windows, and macOS. All platforms are targets
for active development.

## Highlights

- Tabs with renameable titles, selectable icons, and tab reordering, recursive pane splits and
  pane rotation, pane templates, pane minimization, input broadcasting, and
  modal keyboard or mouse pane resizing and moving
- Automatically detected shells, including Homebrew-installed shells on
  macOS and Linux, plus first-class WSL/MSYS2 profiles with working directory
  tracking
- Detachable background sessions that can survive after the last window closes
- Optional compact mode moves tabs into the title bar, keeps Menu, Profile,
  and Broadcast available, and preserves responsive tab sizing
- Open paths or the complete pane scrollback in `$EDITOR`, with built-in vi as
  the fallback
- Native command, path, and SSH-alias completion in a multi-command prompt
- Typed settings and keymap editor, per-profile themes, and installable Zed
  themes
- Serial consoles plus built-in HTTP and TFTP tools, usable from panes or the
  CLI
- Git worktree workflow with `zetta wt new`, `zetta wt done`, status, and
  conflict-resolution helpers; shell integration adds the `zwt` directory wrapper
- In-memory tab attention badges from `zetta attention`, with optional
  cross-platform desktop notifications
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
From a Zetta pane, `zetta tabicon ICON` changes the active tab icon, and
`zetta panetheme THEME` non-persistently changes the active pane's theme
(`zetta panetheme --reset` restores the profile's configured theme).
`zetta overlay TEXT` non-persistently shows text over the active pane's
terminal content, with `--size`, `--opacity`, and `--color` options
(`--color` accepts the named presets `black`, `white`, `gray`, `red`,
`orange`, `yellow`, `green`, `cyan`, `blue`, `purple`, `magenta`, and `pink`,
as well as the existing hex formats; `zetta overlay --reset` clears it).
Shell integration completes the current serial-device, tab-icon, and
pane-theme lists dynamically. See [Serial and network tools](docs/tools.md)
for flags and safety notes.

Use `zetta --replace-pane --split NAME` or
`zetta --replace-pane --profile PROFILE` to replace the active pane in a
running process; the command falls back to the normal new-window launch when
no process accepts it. See [Using Zetta](docs/usage.md) for details.

Manage persistent profiles without opening the settings UI with
`zetta profile list`, `zetta profile themes`, and the profile mutation
commands documented in the [configuration guide](docs/configuration.md).
Changes are validated before being saved and request a best-effort live reload
for a Zetta process using the same configuration file.

Use `zetta wt new NAME`, `zetta wt done`, `zetta wt status`, and
`zetta wt rerere` for the Git worktree workflow. The direct commands never
change the caller's directory; generated shell integration provides `zwt new`
and `zwt done` wrappers that enter the resulting worktree, including paths with
spaces and nested names. `zetta wt new` also recursively initializes source
submodules at their pinned commits, reusing initialized source checkouts as
local object references and falling back to configured submodule remotes when a
source checkout is unavailable. Failed initialization cleans up the partial
worktree, branch, and metadata. See [Using Zetta](docs/usage.md#git-worktrees)
for configuration and safety details.

## Multi-command prompt

Press `Ctrl-Shift-M` and enter a command such as:

```sh
ssh {{dev,prod}}-{{eu,us}}.example.com
```

Zetta expands the Cartesian product, tiles the active pane, and runs one
command in each new pane. The prompt completes executables from `PATH`, paths
relative to the active pane, and SSH aliases from `~/.ssh/config`; use `Tab`
and `Shift-Tab` to cycle completions.

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
- [Configuration](docs/configuration.md): settings, profiles, keymaps, fonts,
  and themes
- [Background sessions](docs/background-sessions.md): detach, protect, inspect,
  and reconnect sessions
- [Shell integration](docs/shell-integration.md): command completion and the
  `zvi`/`zwt`/`ztftp`/`zntfy`/`vi` shortcuts
- [Serial and network tools](docs/tools.md): serial consoles, HTTP and TFTP
  servers, the TFTP client, and desktop notifications
- [Performance profiling](docs/performance.md): overlays, automated reports,
  stress workloads, and diagnostics

Use [`config.example.json`](config.example.json) and
[`keymap.example.json`](keymap.example.json) as starting points for local
customization.

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
