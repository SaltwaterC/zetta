# Zetta

Zetta is a standalone terminal emulator built from Zed's GPUI and terminal
engine. Its local terminal-view fork retains the GPU renderer and terminal
interaction code without Zed's editor, project, workspace, database, or
language subsystems. It supports multiple tabs, selectable profiles, and
user-defined key bindings on Linux, macOS, and Windows.

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
home rather than the Windows user profile. Later tabs and splits inherit the
active pane's current directory.

Keyboard shortcuts use Zed's keymap format. The default shortcuts are:

| Shortcut | Action |
| --- | --- |
| `Ctrl-Shift-T` | New tab |
| `Ctrl-Shift-N` | New window |
| `Ctrl-Shift-1` ... `Ctrl-Shift-9` | New tab with profile 1 ... 9 |
| `Ctrl-Shift-W` | Close tab |
| `Ctrl-Shift-O` | Split active pane horizontally (top/bottom) |
| `Ctrl-Shift-E` | Split active pane vertically (left/right) |
| `Alt-Arrow` | Focus the pane in that direction |
| `Ctrl-Tab` | Next tab |
| `Ctrl-Shift-Tab` | Previous tab |
| `F2` | Rename active tab |
| `Ctrl-=` / `Ctrl-+` | Increase terminal font size |
| `Ctrl--` | Decrease terminal font size |
| `Ctrl-0` | Reset terminal font size |
| `Ctrl-Shift-R` | Reload configuration, keymap, and user themes |

Tab names follow the active terminal process automatically. Press `F2` or
double-click a tab to set a persistent name. Submit an empty name to clear the
override and resume automatic naming. Tabs retain a fixed width as names
change.

Splits inherit the active pane's working directory and use the selected
profile. Use `Alt-Left`, `Alt-Right`, `Alt-Up`, and `Alt-Down` to move focus, or
click a pane. Exiting a shell removes that pane; exiting the final pane closes
its tab.

Selecting terminal text copies it to the system clipboard while preserving the
selection. On Linux and FreeBSD, selections also populate the PRIMARY
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
system-installed font. `inactive_pane_opacity` controls inactive split-pane
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

Font-size shortcuts apply immediately to every open terminal. Reset returns to
`terminal_font_size` when configured, otherwise to Zed's default buffer size.
Zetta bundles the Regular, Bold, Italic, and Bold Italic faces of MesloLGS NF
and uses that family by default, so Nerd Font prompt glyphs work without a
system font installation.

The bundled MesloLGS NF files come from the Powerlevel10k media repository at
the commit recorded in `assets/fonts/meslo-lg-nerd-font/UPSTREAM.md` and retain
their Apache-2.0 license in the same directory.

## User themes

Zetta loads Zed theme-family JSON files from `~/.config/zetta/themes` on
Linux/macOS and `%APPDATA%\Zetta\themes` on Windows. The directory is created
on first launch. Download or extract the `.json` file from a Zed theme
extension, place it directly in that directory, set `theme` in `config.json` to
the theme name declared inside the file, and reload the configuration.

For Solarized on Linux/macOS:

```sh
mkdir -p ~/.config/zetta/themes
curl -L https://raw.githubusercontent.com/harmtemolder/Solarized.zed/main/themes/solarized.json \
  -o ~/.config/zetta/themes/solarized.json
```

On Windows PowerShell:

```powershell
$themes = Join-Path $env:APPDATA "Zetta\themes"
New-Item -ItemType Directory -Force $themes
Invoke-WebRequest `
  https://raw.githubusercontent.com/harmtemolder/Solarized.zed/main/themes/solarized.json `
  -OutFile (Join-Path $themes "solarized.json")
```

Then configure:

```json
{
  "theme": "Solarized Light"
}
```

Only standalone Zed theme JSON files are loaded; Zetta does not currently
install complete Zed extension packages.

## Licensing

Zetta source code is licensed primarily under GPL-3.0-or-later, with
Apache-2.0 components where marked, matching Zed's licensing model. See
`LICENSE-GPL` and `LICENSE-APACHE` for the full license texts. Zetta is an
independent project and is not affiliated with Zed Industries, Inc.
