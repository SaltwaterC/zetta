# Configuring Zetta

Use [`config.example.json`](../config.example.json) and
[`keymap.example.json`](../keymap.example.json) as starting points. They are
examples and are not loaded automatically.

## File locations and reloads

Zetta loads configuration from:

- Linux and macOS: `~/.config/zetta/config.json`
- Windows: `%APPDATA%\Zetta\config.json`

The keymap is `keymap.json` in the same platform-specific directory. Use
`--config PATH` and `--keymap PATH` to override these locations.

If configuration cannot be parsed, Zetta starts with safe defaults and shows
the error in the window. Correct the file and press `Ctrl-Cmd-R` on macOS or
`Ctrl-Alt-R` on Windows/Linux to reload configuration, keymaps, and user themes
without restarting. Existing sessions
and their scrollback are retained.

After a successful reload started from Zetta, a green `Configuration reloaded`
banner appears for three seconds. This also happens when saving configuration
from the Settings UI or installing and removing theme extensions. Failed
reloads continue to show the existing error banner, while reloads requested by
the CLI through process control remain silent.

## Settings editor

Press `Ctrl-,` or use the tab-bar settings button to open typed controls for
the active configuration and keymap files. Profiles and themes use checked
dropdowns, the font picker searches installed families, and detected profiles
expose theme overrides and individual visibility toggles for the Profiles menu.

Font size and scrollback accept typed values and press-and-hold steppers;
scrollback also supports a `Max` sentinel. Inactive-pane opacity uses a
percentage slider. Settings and font lists have independent scrollbars, and new
profiles are created in a labeled modal. Key bindings are grouped by context
with action dropdowns.

All configuration dropdowns support fuzzy type-to-search. Open a dropdown and
type letters to filter its entries; matching is case-insensitive and supports
subsequences, so `ond` can find `One Dark`. The query stays visible above the
scrollable results, while the selected match is scrolled into view. Use
Backspace to revise the query, the arrow keys to move among matches, and
Enter or Space to select an entry. Escape or Tab closes the dropdown, and a
query with no matches leaves the current value unchanged.

Saving validates the active page, persists and applies it without restarting,
closes the dialog, and returns focus to the terminal. Invalid settings or
bindings are reported without replacing the existing file. Custom `--config`
and `--keymap` paths remain CLI-only settings.

The HTTP and TFTP server ports are typed settings backed by
`http_server_port` and `tftp_server_port` in `config.json`. They default to
8000 and 69 respectively and accept integers from 1 through 65535.

## Profiles and working directories

Zetta detects common shells. On macOS and Linux, shells installed by Homebrew
are also detected as separate profiles (for example, `Bash (Homebrew)`), even
when the graphical application was not launched with Homebrew's `bin`
directory in its `PATH`. On Windows this includes Windows PowerShell,
PowerShell 7, Command Prompt, MSYS2, and registered WSL distributions. The
MSYS2 Start Menu shortcut is used to find current custom installation paths;
legacy uninstall registration is also checked, with `C:\msys64` as a fallback.
Zetta launches `msys2_shell.cmd` in the MSYS environment so the normal MSYS2
initialization is retained. The launcher imports the Windows `PATH`, with
Zetta's installation directory placed first, so `zetta init` and other Zetta
commands are immediately available inside the profile. Bash and Zsh profiles
also report their current MSYS2 directory to Zetta, so new tabs, split panes,
multi-command panes, and local server actions inherit the directory after `cd`.

MSYS2 Bash appears as `MSYS2`. If `usr\bin\zsh.exe` is installed, Zetta also
adds `MSYS2: Zsh` and passes MSYS2's supported `-shell zsh` launcher option.
Select that profile as `default_profile` (or in the settings editor) to use
Zsh without relying on `chsh`:

```json
{
  "default_profile": "MSYS2: Zsh"
}
```

Profile 1 is `System`, followed by detected profiles and configured `profiles`
in the order displayed by the profile menu. A configured profile with the same
name as a detected profile overrides it in place. Set `default_profile` to any
displayed name; matching is case-insensitive. Opening a profile from the menu
or a shortcut does not change the profile used by subsequent new tabs by
default. New tabs use `default_profile`; set `new_tab_profile` to `"inherit"`
to use the active tab's profile instead. The same setting is available as
**New Tab profile** under **Default profile** in the Configuration panel:

```json
{
  "default_profile": "System",
  "new_tab_profile": "inherit"
}
```

Missing shortcut slots have no effect.

To keep an individual detected profile out of the Profiles menu, use its
visibility toggle in the settings editor or set `hidden` on that profile. A
hidden profile remains available to `default_profile` and `--profile`; hidden
profiles do not consume the numbered menu shortcuts, so later visible profiles
move into the released shortcut slot:

```json
{
  "profiles": [
    { "name": "PowerShell 7", "hidden": true }
  ]
}
```

The same profile state can be managed without opening the UI:

```sh
zetta profile list
zetta profile themes
zetta profile disable "Bash"
zetta profile enable "Bash"
zetta profile theme "Bash" "One Dark"
zetta profile theme "Bash" --reset
zetta profile default "Bash"
zetta profile add "Project Shell" --program bash --arg -l --theme "One Dark"
zetta profile remove "Project Shell"
```

Use `-c PATH` or `--config PATH` with any profile operation, either after the
`profile` command or before it (`zetta -c PATH profile list`). Names are
matched case-insensitively. `profile list` prints every resolved profile,
including hidden profiles, while `profile themes` prints the sorted bundled
and installed theme names used for validation. Mutations preserve the other
configuration settings and validate the complete candidate before writing it.
Detected profiles and the active default profile cannot be removed.

After a successful mutation, Zetta asks a running process using the same
normalized configuration path to reload. Open and dormant entities are
refreshed, and new tabs use the updated profile state. The file change remains
successful when no matching process is running; the CLI prints a notice in
that case.

The first tab starts in the user's home directory unless `working_directory`
is set. Setting it to `"~"` is equivalent to leaving it unset. Later native
tabs and splits inherit the active pane's current directory by default. Set
`working_directory_scope` to `"none"` to always use the configured directory,
or to `"pane"` to inherit only for new shells in the same tab. The default
`"tab"` scope inherits for both new panes and new tabs.

Detected WSL profiles start in the selected distribution's Linux home. Zetta
tracks the Linux directory for bash, fish, and zsh, with a fallback for other
shells, so same-profile tabs and splits inherit it even though `wsl.exe` exposes
only a Windows-side directory. On Windows, prompt integration similarly tracks
the active filesystem directory for Windows PowerShell, PowerShell 7, and
Command Prompt without replacing the user's prompt.

Profiles may choose a Zed theme independently from the application theme. A
detected profile needs only its name and theme; its detected command is
retained. New profiles require `program`. Each terminal pane uses its profile's
theme, and each tab uses its active pane's theme for its background, text,
icons, border, and active indicator:

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

Launch a specific profile with `zetta --profile "PROFILE"` or
`zetta -p "PROFILE"`. The Windows Jump List uses the same option through the
no-console launcher.

## Key bindings

Keyboard shortcuts use Zed's keymap format. Put overrides in `keymap.json` and
retain the `Zetta > Terminal` context so they take precedence over terminal
defaults. Key names accept both `pageup`/`pagedown` and the common
`page-up`/`page-down` spellings. See [Using Zetta](usage.md) for the complete
default shortcut table.

The Settings > Keymap editor and keymap JSON use physical-key aliases:
`Ctrl+Shift+1` through `Ctrl+Shift+9`, plus `Ctrl+Shift+0` for profile 10. The
recorder produces the aliases too, so the display and examples stay stable
across keyboard layouts. Older normalized key names remain accepted for
backward compatibility, but are not written by the editor. Set
`"use_key_equivalents": true` on that keymap section, as demonstrated in
`keymap.example.json`.

These aliases refer to the number-row key position, not the character printed
by the active layout. For example, on a British layout `Ctrl+Shift+3` may type
`£`, while the alias remains `Ctrl+Shift+3` and the shortcut still opens profile
3.

Zetta normalizes these physical keys on Linux so the shortcuts work with
layouts whose shifted characters differ. On Windows and macOS, shortcuts
follow the active keyboard mapping and are rebuilt when the layout changes.
`Ctrl-Alt` number-row fallbacks are not built in because they collide with
AltGr on layouts that use it.

## Appearance and scrollback

Zetta defaults to the bundled `One Light` theme and MesloLGS NF font. Common
appearance settings include:

```json
{
  "theme": "One Dark",
  "default_tab_icon": "terminal",
  "terminal_font_size": 14,
  "terminal_font_family": "MesloLGS NF",
  "inactive_pane_opacity": 0.8,
  "compact_mode": false,
  "hide_pane_size": true,
  "hide_title_bar_labels": false,
  "hide_title_bar_buttons": false,
  "hide_title_bar_menus": true,
  "pane_controls_position": "right",
  "pane_controls_hidden_by_default": false,
  "max_scroll_history_lines": 2147483647
}
```

`terminal_font_size` accepts values from 6 through 100.
`terminal_font_family` accepts bundled and system-installed fonts.
`default_tab_icon` accepts any built-in icon name, or `null` to hide icons on
new tabs. It can also be changed through Settings > Configuration.
`inactive_pane_opacity` accepts values from 0 through 1 and defaults to 0.8.
`compact_mode` defaults to `false`. When enabled, tabs move into the title bar,
title-bar labels and pane size are hidden, and only Menu, Profile, and
Broadcast remain on the left. If background sessions exist, the reconnect
indicator always appears at the end of the title bar in compact mode.
`hide_pane_size` defaults to `true` and hides the active pane dimensions from
the title bar. `hide_title_bar_labels` and `hide_title_bar_buttons` default to
`false`; they hide title-bar text and controls respectively. On macOS,
`hide_title_bar_menus` defaults to `true` and hides the Menu and Profile menus
from the title bar. It is ignored on other platforms and is not shown in their
settings editor.
`pane_controls_position` accepts `"left"` or `"right"` and defaults to
`"right"`. It controls the pane overlay buttons independently of the system
window-button layout so they do not move over a left-aligned prompt unless you
choose that placement explicitly. Tab close buttons do follow the system
window-button side.
`pane_controls_hidden_by_default` defaults to `false`. Set it to `true` to
start new panes with the controls hidden. Use the pane controls or command
palette to toggle controls for the active pane or every pane in the active tab.
When this setting changes and configuration reloads, Zetta resets every open
pane to the selected default visibility.

`max_scroll_history_lines` defaults to the Alacritty engine's signed
line-coordinate ceiling of 2,147,483,647 lines, which is effectively unlimited
for typical use. Retained output consumes memory. Set it to 0 to disable
scrollback. Changes apply to newly opened tabs.

The standard font-size shortcuts apply to all terminals. `Ctrl-Alt` variants
apply only to the active pane, allowing split panes to use independent sizes.
Pane reset removes that pane's override; global reset returns to
`terminal_font_size` when configured, otherwise to Zed's default buffer size.

Zetta bundles the Regular, Bold, Italic, and Bold Italic faces of MesloLGS NF,
so Nerd Font prompt glyphs work without a system installation. The files come
from Powerlevel10k at the commit recorded in
[`assets/fonts/meslo-lg-nerd-font/UPSTREAM.md`](../assets/fonts/meslo-lg-nerd-font/UPSTREAM.md)
and retain their Apache-2.0 license.

## User themes

Zetta loads Zed theme-family JSON files from:

- Linux and macOS: `~/.config/zetta/themes`
- Windows: `%APPDATA%\Zetta\themes`

The directory is created on first launch. Place a standalone theme JSON file
there, set `theme` in `config.json` to a theme name declared by that file, and
reload the configuration.

The settings UI also has a **Themes** tab that searches theme-providing
extensions on the official Zed extensions site. Installing an extension
downloads its archive, copies only theme JSON files declared by its manifest,
and immediately reloads the configuration and theme selectors. Themes installed
this way can be removed from the same tab. Manually placed files are never
removed by the UI, and other extension features are not installed or run.

`Solarized Dark` and `Solarized Light` are bundled and do not belong in the
user theme directory. Their files come from the official Zed Solarized
extension at the revision recorded in
[`assets/themes/solarized/UPSTREAM.md`](../assets/themes/solarized/UPSTREAM.md)
and retain their GPL-3.0 license.
