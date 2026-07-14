# Zetta

Zetta is a standalone terminal emulator built from Zed's GPUI and terminal
engine. Its local terminal-view fork retains the GPU renderer and terminal
interaction code without Zed's editor, project, workspace, database, or
language subsystems. It supports multiple tabs, selectable shell profiles, and
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
Debian/Ubuntu builds require both `libxkbcommon-dev` and
`libxkbcommon-x11-dev`, even for the default Wayland build.

Zetta discovers common installed shells. On Windows this includes Windows
PowerShell, PowerShell 7, Command Prompt, and WSL when their executables are on
`PATH`. Select a profile in the top bar, then open a new tab.

Configuration is loaded from `~/.config/zetta/config.json` on Linux/macOS and
`%APPDATA%\\Zetta\\config.json` on Windows. Use `config.example.json` as a
starting point. `--config PATH` and `--keymap PATH` override the defaults.

Keyboard shortcuts use Zed's keymap format. The default shortcuts are:

| Shortcut | Action |
| --- | --- |
| `Ctrl-Shift-T` | New tab |
| `Ctrl-Shift-N` | New window |
| `Ctrl-Shift-1` ... `Ctrl-Shift-9` | New tab with shell profile 1 ... 9 |
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

Tab names follow the active terminal process automatically. Press `F2` or
double-click a tab to set a persistent name. Submit an empty name to clear the
override and resume automatic naming. Tabs retain a fixed width as names
change.

Splits inherit the active pane's working directory and use the selected shell
profile. Use `Alt-Left`, `Alt-Right`, `Alt-Up`, and `Alt-Down` to move focus, or
click a pane. Exiting a shell removes that pane; exiting the final pane closes
its tab.

These bindings are built into Zetta; `keymap.example.json` mirrors them as a
starting point for overrides and is not loaded automatically. Place overrides
in `keymap.json` and keep the `Zetta > Terminal` context so they take precedence
over Zed's terminal bindings.

Shell profile shortcuts use the order displayed in the tab bar. With automatic
discovery, profile 1 is `System`, followed by detected shells. An explicit
`shells` configuration uses its configured order instead. Opening a profile
this way also makes it the selection used by subsequent `Ctrl-Shift-T` tabs.
Missing profile slots have no effect.

GPUI represents shifted number-row keys by their symbols internally, so custom
keymaps should use `ctrl-!`, `ctrl-@`, through `ctrl-(` as shown in
`keymap.example.json`. `Ctrl-Alt-1` through `Ctrl-Alt-9` are also built-in
fallbacks.

Set `theme` to the name of a bundled Zed theme and `terminal_font_size` to a
value from 6 through 100 in `config.json`. `terminal_font_family` accepts the
name of any bundled or system-installed font. `max_scroll_history_lines`
defaults to the Alacritty engine's signed line-coordinate ceiling of
2,147,483,647 lines and disables scrollback when set to 0. This is effectively
unlimited for normal use; memory grows as output is retained. For example:

```json
{
  "theme": "One Dark",
  "terminal_font_size": 14,
  "terminal_font_family": "MesloLGS NF",
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
the theme name declared inside the file, and restart Zetta.

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
