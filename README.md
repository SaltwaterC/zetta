# Zetta

Zetta is a standalone terminal emulator built from Zed's GPUI and terminal
engine. Its local terminal-view fork retains the GPU renderer and terminal
interaction code without Zed's subsystems. It supports multiple tabs,
selectable profiles, and user-defined key bindings on Linux, macOS, and
Windows.

The name is a portmanteau of Zeta and tty, albeit if you are having a bad
day, that's about the size of the binaries Rust produces. Pun not intended.

## Design philosophy

There is more than one way to do things. This is to allow for muscle memory
formed over years of working under multiple platforms using multiple terminal
emulators that all do things in their specific ways. This applies for things
like copying, pasting, changing tabs for example.

There is an explicit aim to have things work outside the box at the cost of
bundling as well as minimal configuration. For example, a large chunk of the
bundle is formed by the MesloLGS NF font faces.

Convention is preferred over configuration, however, configurations options
exist where they are needed.

## Why?

Have you ever wondered whether Zed's terminal can be a standalone application?
I have and I am not the only one. However, the terminal view doesn't come with
batteries included to make it a terminal emulator fit for the worklow usually
done using a dedicated piece of software. I spent a significant amount of my
life in a terminal emulator and this amount of time has now increased since
various AI harnessess were pretty much released as TUI by default.

Codex helped putting this together as it wouldn't have been possible to do it
in such a short amount of time. Decided to give GPT 5.6-sol a go to see whether
it can actually turn this idea into reality. It started off as a weekend project
albeit once I got the ball rolling things started to add up. The project was
self-hosted in terms of development using Codex TUI before the first commit was
even made.

While the implementation has been done by Codex, the design of the application,
testing, as well as being rather particular about what goes where came from good
old fashion brain and decades of experience typing commands into a box.

I am one of those people who move across platforms quite often, sometimes even
in the span of a single day. Terminal emulator experience has been inconsistent
at best. I am a veteran user of iTerm (macOS) and Terminator (Linux) with
Windows Terminal thrown in the mix since the early releases. While there is a new
generation of terminal emulators that are now available, none cover this list
entirely:

- Cross platform across Windows, macOS, Linux at minimum. Most don't support
Windows. The aim is to have a consitent experience out of the box across all
platforms.
- Focus on performance. The underlying emulator is an embedded Alacritty. While
Alacritty itself works cross-platform, it doesn't implement the surrounding
functionality like tabs or panes by choice.

WSL is actually a first class citizen and the installed WSL instances are
automatically detected as profiles. Furthermore, Zetta implements correct CWD
tracking for WSL instances for bash, fish, and zsh while providing a fallback
method for any other shell.

Noting here that macOS support is not tested yet even though the codebase supports
macOS and I would expect it would be possible to produce a build with minimum
changes.

## Build and run

The `zed` submodule must be initialized before building.

```sh
git submodule update --init
cargo run
```

Use `cargo check` for the fastest feedback while editing. Release builds use
incremental compilation to reduce rebuild time between local changes and emit
a stripped executable.

Linux defaults to Wayland. Build with `cargo run --features x11` to include the
X11 backend as well. GPUI currently links both xkbcommon libraries on Linux, so
Debian/Ubuntu builds require `libfontconfig-dev`, `libxkbcommon-dev`, and
`libxkbcommon-x11-dev`, even for the default Wayland build.

### Windows

Build a release executable from PowerShell with Chocolatey's GNU Make:

```powershell
make build
```

The Windows build target locates the Visual Studio C++ toolchain with
`vswhere.exe` and initializes its x64 build environment automatically. The
Desktop development with C++ workload must be installed.

The executable is written to `target\release\zetta.exe` with the application
icon from `assets\icons\zetta-terminal-icon.ico` embedded as a Windows resource.
The build also stages `conpty.dll` and `OpenConsole.exe` in the same directory;
all three files are required at runtime.

Install Zetta for the current Windows user with:

```powershell
make install
```

This requires no administrator privileges. It copies the executable and its
two ConPTY runtime files to `%LOCALAPPDATA%\Programs\Zetta` and creates
`%APPDATA%\Microsoft\Windows\Start Menu\Programs\Zetta.lnk`, making Zetta
available through Start Menu application search. The shortcut uses the icon
embedded in the executable. Run `make uninstall` to remove the installed
runtime and shortcut.

`make install-binary` updates only the installed executable. `make
install-assets` recreates only the Start Menu shortcut and requires the binary
to already be installed.

### Linux desktop integration

Zetta uses `Zetta` as its Wayland application ID and X11 `WM_CLASS`.
The Makefile builds the release binary as the current user, then installs it
with the desktop entry and icons under `/usr` by default:

```sh
make build
sudo make install
```

When run through `sudo`, `make install` uses the existing release artifact and
does not invoke Cargo again. An unprivileged `make install` still builds first.

To reinstall only the desktop entry and icons without rebuilding Zetta, run:

```sh
sudo make install-assets
```

Use `sudo make uninstall-assets` to remove only those assets. The full
`uninstall` target removes the binary and assets. Set `PREFIX=/usr/local` for a
traditional local system prefix, or use `PREFIX="$HOME/.local"` for a per-user
installation without `sudo`. `DESTDIR` is supported for staged package builds.
Desktop and icon caches are refreshed when the relevant utilities are available
and `DESTDIR` is not set.

WSLg only exports applications discovered in system desktop-entry directories,
so use the default `/usr` prefix there. Zetta installs both 128px and 512px
hicolor icons; the 128px variant is required for WSLg's application-icon lookup.
After installing or upgrading under WSL2, close running Zetta windows and run
`wsl --shutdown` from Windows if the previous taskbar icon remains cached.

Zetta creates profiles for common installed command interpreters. On Windows
this includes Windows PowerShell, PowerShell 7, Command Prompt, and registered
WSL distributions. Select a profile in the top bar, then open a new tab.

Configuration is loaded from `~/.config/zetta/config.json` on Linux/macOS and
`%APPDATA%\\Zetta\\config.json` on Windows. Use `config.example.json` as a
starting point. `--config PATH` and `--keymap PATH` override the defaults. If
the configuration cannot be parsed, Zetta starts with safe defaults and shows
the error in the window; correct the file and reload it without restarting.
The first tab starts in the user's home directory unless `working_directory`
is configured. A detected WSL profile uses the selected distribution's Linux
home rather than the Windows user profile. Later native-shell tabs and splits
inherit the active pane's current directory. Because `wsl.exe` exposes only its
Windows-side directory, Zetta tracks each WSL shell's Linux directory and uses
it for same-profile tabs and splits.

Keyboard shortcuts use Zed's keymap format. The default shortcuts are:

| Shortcut | Action |
| --- | --- |
| `Ctrl-Shift-T` | New tab |
| `Ctrl-Shift-N` | New window |
| `Ctrl-Shift-1` ... `Ctrl-Shift-9` | New tab with profile 1 ... 9 |
| `Ctrl-Shift-W` | Close tab |
| `Ctrl-Shift-O` | Split active pane horizontally (top/bottom) |
| `Ctrl-Shift-E` | Split active pane vertically (left/right) |
| `Ctrl-Shift-A` | Select all terminal text |
| `Ctrl-Shift-Backspace` | Clear the system clipboard |
| `Alt-Arrow` | Focus the pane in that direction |
| `Ctrl-Shift-I` | Toggle input broadcasting to every pane in the active tab |
| `Ctrl-Tab` | Next tab |
| `Ctrl-Shift-Tab` | Previous tab |
| `Ctrl-PageUp` | Next tab |
| `Ctrl-PageDown` | Previous tab |
| `Ctrl-C` | Copy when text is selected; otherwise send interrupt |
| `Ctrl-V` | Paste |
| `Ctrl-Shift-F` | Search the active pane's scrollback |
| `Ctrl-Alt-F` | Search scrollback across every pane in the active tab |
| `Ctrl-Alt-V` | Paste after trimming leading and trailing whitespace |
| `Ctrl-Shift-P` | Open the command palette |
| `Ctrl-,` | Open the configuration and keymap editor |
| `F2` | Rename active tab |
| `Ctrl-=` / `Ctrl-+` | Increase font size globally |
| `Ctrl--` | Decrease font size globally |
| `Ctrl-0` | Reset font size globally |
| `Ctrl-Alt-=` / `Ctrl-Alt-+` | Increase active pane font size |
| `Ctrl-Alt--` | Decrease active pane font size |
| `Ctrl-Alt-0` | Reset active pane font size |
| `Ctrl-Shift-R` | Reload configuration, keymap, and user themes |
| `Ctrl-Shift-F12` | Toggle the performance overlay |

Input broadcasting is scoped to the active tab and is off by default. When it is
enabled, typing, terminal control keys, IME text, and pastes sent to the active
pane are also sent to every other open pane in that tab.

The command palette lists the actions available in the focused terminal and
Zetta window, including their effective keyboard shortcuts. Type to filter,
use the arrow keys to select a command, and press `Enter` to run it.

The settings button in the tab bar (or `Ctrl-,`) opens typed controls for the
active configuration and keymap files. Profiles and themes use checked
dropdowns, the font picker searches installed families in its own modal, and
detected profiles expose theme overrides. Font size and scrollback accept typed
values as well as press-and-hold steppers; scrollback uses a `Max` sentinel.
Inactive-pane opacity uses a percentage slider. Settings and font lists have
independent, visible scrollbars, and new profiles are created in a labeled modal.
Key bindings are grouped by context with action dropdowns. Configuration and
keymap paths follow the platform conventions above; overriding either path remains
CLI-only through `--config` and `--keymap`. Saving validates the changed page,
persists it, applies it without restarting, closes the dialog, and returns focus
to the terminal. Invalid settings or bindings are reported without replacing the
existing file.

Scrollback search is scoped to the active pane. `Enter` and `F3` select the
next match, `Shift-Enter` and `Shift-F3` select the previous match, and `Escape`
closes the search. In terminal vi mode, `/` also opens scrollback search.

Tab-wide search uses `Ctrl-Alt-F`, highlights matches in every pane in
the active tab, and activates the pane containing the current result while you
navigate.

The performance overlay reports GPUI frames drawn during the latest one-second
sample, average and 95th-percentile CPU draw time, average invalidation-to-draw
latency, and frame counts exceeding the 120 Hz and 60 Hz budgets. GPUI renders
on demand, so an idle terminal can report zero or a very low draw FPS; this is
not the monitor refresh rate or GPU presentation latency.

For repeatable terminal-rendering profiles, launch Zetta's built-in workload:

```sh
zetta --profile-terminal-rendering
```

When running from the repository, use
`cargo run --release -- --profile-terminal-rendering`. The mode launches a
deterministic 240 Hz full-grid producer and enables the performance overlay
automatically. It is implemented by the Zetta executable rather than a shell
script, so the same command works on Linux, macOS, and Windows. Use a release
build when comparing CPU measurements. The workload is intentionally faster
than common displays so frame coalescing and presentation overhead remain
visible and comparable between runs.

The overlay provides application-level frame timings. For native stack samples,
attach the platform profiler to the Zetta process while this mode is running:
Linux `perf`, macOS Instruments or `sample`, and Windows Performance Recorder
and Analyzer are all suitable.

For an automated ten-second run that writes a portable JSON report and exits:

```sh
zetta --profile-terminal-rendering \
  --profile-report artifacts/zetta-performance.json
```

Set a different duration, including fractional seconds, with
`--profile-duration`:

```sh
cargo run --release -- \
  --profile-terminal-rendering \
  --profile-report artifacts/zetta-performance.json \
  --profile-duration 30
```

These commands have the same arguments in PowerShell, Command Prompt, and Unix
shells (adjust line continuation syntax when splitting the command). Providing a
report path defaults to ten seconds; `--profile-duration` requires a report
path. Zetta creates missing parent directories, writes the report, and exits.
Closing the window early or failing to write the report returns a non-zero exit
status.

Reports use a versioned JSON schema and include the Zetta version, build
profile, operating system and architecture, workload parameters, requested and
actual elapsed time, per-second samples, total frame count, draw FPS,
average/p50/p95/p99 draw time, average
invalidation-to-draw latency, and counts over the 120 Hz and 60 Hz frame
budgets. Commit reports as CI artifacts or feed them into a separate comparison
step; native stack traces remain separate platform-profiler artifacts.

Tab names follow the active terminal process automatically. Press `F2` or
double-click a tab to set a persistent name. Submit an empty name to clear the
override and resume automatic naming. Tabs retain a fixed width as names
change.

Splits inherit the active pane's working directory and use the selected
profile. Use `Alt-Left`, `Alt-Right`, `Alt-Up`, and `Alt-Down` to move focus, or
click a pane. Exiting a shell removes that pane; exiting the final pane closes
its tab.

Selecting terminal text copies it to the system clipboard while preserving the
selection. `Ctrl-C` copies an existing selection and continues to send an
interrupt when nothing is selected. `Ctrl-V` pastes; this takes precedence over
the shell's traditional quoted-insert use of that chord. Plain right-click
pastes when the clipboard contains data and opens
the context menu when it is empty; `Shift`-right-click always opens the context
menu. The context menu's **Paste Trimmed** action removes leading and trailing
whitespace while preserving whitespace inside the copied text. On Linux and
FreeBSD, selections also populate the PRIMARY
selection and a middle click pastes from PRIMARY, falling back to the system
clipboard when PRIMARY is unavailable or empty. On other platforms, a middle
click pastes from the system clipboard.

These bindings are built into Zetta; `keymap.example.json` mirrors them as a
starting point for overrides and is not loaded automatically. Place overrides
in `keymap.json` and keep the `Zetta > Terminal` context so they take precedence
over Zed's terminal bindings. Key names accept both `pageup`/`pagedown` and the
common `page-up`/`page-down` spellings.

Press `Ctrl-Shift-R` after editing `config.json`, `keymap.json`, or files in the
user themes directory. Configuration changes affect the active window and
global terminal appearance; existing sessions and their scrollback are retained.

Profile shortcuts use the order displayed in the profile menu. Profile 1 is
`System`, followed by detected profiles and any additional configured
`profiles`. A configured profile with the same name as a detected profile
overrides that profile in place. Set `default_profile` to any displayed name,
including a detected profile such as `Zsh`, `PowerShell`, or `WSL: Ubuntu`; the
match is case-insensitive and the default is marked in the profile menu.
Opening a profile from the menu or a shortcut makes it the selection used by
subsequent `Ctrl-Shift-T` tabs. Missing profile slots have no effect.

Profiles may select a Zed theme independently from the application theme. A
detected profile needs only its name and theme; its detected command is
retained. New profiles still require `program`. The override applies to each
terminal pane created from that profile. Each tab also uses its active pane's
theme for its background, text, icons, border, and active indicator:

```json
{
  "default_profile": "Zsh",
  "profiles": [
    { "name": "Zsh", "theme": "Solarized Light" },
    {
      "name": "Login Zsh",
      "program": "/bin/zsh",
      "args": ["-l"],
      "theme": "One Dark"
    }
  ]
}
```

GPUI represents shifted number-row keys by their symbols internally, so custom
keymaps should use `ctrl-!`, `ctrl-@`, through `ctrl-(` as shown in
`keymap.example.json`. `Ctrl-Alt-1` through `Ctrl-Alt-9` are also built-in
fallbacks.

Zetta defaults to the bundled `One Light` theme. Set `theme` to the name of a
bundled Zed theme and `terminal_font_size` to a value from 6 through 100 in
`config.json`. `terminal_font_family` accepts the name of any bundled or
system-installed font. The built binary includes `Solarized Dark` and
`Solarized Light`.

`inactive_pane_opacity` controls inactive split-pane
dimming from 0 to 1 and defaults to 0.8.
`max_scroll_history_lines` defaults to the Alacritty engine's signed
line-coordinate ceiling of 2,147,483,647 lines and disables scrollback when set
to 0. This is effectively unlimited for normal use; memory grows as output is
retained. For example:

```json
{
  "theme": "One Dark",
  "terminal_font_size": 14,
  "terminal_font_family": "MesloLGS NF",
  "inactive_pane_opacity": 0.8,
  "max_scroll_history_lines": 2147483647
}
```

Scrollback changes apply to newly opened tabs.

## Pane split templates

Use the parameterized `zetta::ApplyPaneSplitTemplate` action to replace the
active pane with a reusable layout. Zetta includes `three-right` (one pane on
the left and two stacked on the right), `three-left` (the mirror image), and
`quarters` (a 2-by-2 grid). Each template is available by name in the command
palette. For faster access, add bindings like these to `keymap.json`:

```json
{
  "ctrl-alt-o": [
    "zetta::ApplyPaneSplitTemplate",
    { "name": "three-right" }
  ],
  "ctrl-alt-e": [
    "zetta::ApplyPaneSplitTemplate",
    { "name": "quarters" }
  ]
}
```

Templates are recursive. `"pane"` is a leaf, `vertical` places two children
side by side, and `horizontal` stacks two children. Add named custom templates
to `config.json` like this:

```json
{
  "pane_split_templates": {
    "three-bottom": {
      "horizontal": [
        "pane",
        { "vertical": ["pane", "pane"] }
      ]
    }
  }
}
```

Each split must have exactly two children, and each template may contain from
2 through 64 panes. A tab is limited to 64 panes in total, including panes
created by recursive template applications. Custom entries extend the
built-ins and may override them by using the same name. The active terminal
becomes the first (top-left-most) leaf and keeps focus, while new panes inherit
its profile and working directory. Applying a template again therefore
recurses into that active pane without changing the rest of the tab.

The standard font-size shortcuts apply globally to every terminal. The
`Ctrl-Alt` variants apply only to the active pane, so split panes can use
independent sizes. Pane reset removes that pane's override; global reset returns
to `terminal_font_size` when configured, otherwise to Zed's default buffer size.
Zetta bundles the Regular, Bold, Italic, and Bold Italic faces of MesloLGS NF
and uses that family by default, so Nerd Font prompt glyphs work without a
system font installation.

The bundled MesloLGS NF files come from the Powerlevel10k media repository at
the commit recorded in `assets/fonts/meslo-lg-nerd-font/UPSTREAM.md` and retain
their Apache-2.0 license in the same directory.

The bundled Solarized themes come from the official Zed Solarized extension at
the revision recorded in `assets/themes/solarized/UPSTREAM.md` and retain
their GPL-3.0 license in the same directory.

## User themes

Zetta loads Zed theme-family JSON files from `~/.config/zetta/themes` on
Linux/macOS and `%APPDATA%\Zetta\themes` on Windows. The directory is created
on first launch. Download or extract the `.json` file from a Zed theme
extension, place it directly in that directory, set `theme` in `config.json` to
the theme name declared inside the file, and reload the configuration.

The configuration UI also has a `Themes` tab for searching theme-providing
extensions on the official Zed extensions site. Installing one downloads the
extension archive, copies only the theme JSON files declared by its manifest,
and reloads the configuration and theme selectors immediately. Themes installed
this way are listed in the same tab and can be removed there. Manually placed
theme files are never removed by this UI, and other extension features are not
installed or run.

Solarized Dark and Solarized Light are already bundled and do not belong in
the user themes directory.

Only standalone Zed theme JSON files are loaded; Zetta does not currently
install complete Zed extension packages.

## Licensing

Zetta source code is licensed primarily under GPL-3.0-or-later, with
Apache-2.0 components where marked, matching Zed's licensing model. The full
license texts are available separately:

- [GNU General Public License v3.0](LICENSE-GPL)
- [Apache License 2.0](LICENSE-APACHE)

Copyright 2026 Ștefan Rusu. Portions derived from Zed are copyright
2022–2025 Zed Industries, Inc.

Zetta is an independent project and is not affiliated with Zed Industries,
Inc.
