# Zetta

Zetta is a standalone, cross-platform terminal emulator built with Rust,
GPUI, and Zed's terminal engine. It combines a GPU-rendered terminal with the
tabs, panes, profiles, and configurable shortcuts expected from a complete
terminal application.

Zetta currently supports Linux, Windows, and macOS at the code level. Linux
and Windows are the actively tested platforms; macOS support has not yet been
tested.

## Highlights

- Tabs, recursive pane splits, pane templates, pane minimization, and input
  broadcasting
- Automatically detected shells and first-class WSL profiles with working
  directory tracking
- Detachable background sessions that can survive after the last window closes
- Native command, path, and SSH-alias completion in a multi-command prompt
- Typed settings and keymap editor, per-profile themes, and installable Zed
  themes
- Serial consoles plus built-in HTTP and TFTP tools
- Reproducible terminal-rendering performance reports

## Quick start

Initialize the Zed submodule, then run Zetta with the pinned Rust toolchain:

```sh
git submodule update --init
cargo run
```

Linux defaults to Wayland. Use `cargo run --features x11` to include the X11
backend. Linux system dependencies and platform-specific build and desktop
installation instructions are in the [installation guide](docs/installation.md).

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
- [Using Zetta](docs/usage.md): tabs, panes, search, shortcuts, and pane
  templates
- [Configuration](docs/configuration.md): settings, profiles, keymaps, fonts,
  and themes
- [Background sessions](docs/background-sessions.md): detach, protect, inspect,
  and reconnect sessions
- [Serial and network tools](docs/tools.md): serial consoles, HTTP and TFTP
  servers, and the TFTP client
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
