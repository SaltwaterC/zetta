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
the error in the window. Correct the file and press `Ctrl-Shift-R` to reload
configuration, keymaps, and user themes without restarting. Existing sessions
and their scrollback are retained.

## Settings editor

Press `Ctrl-,` or use the tab-bar settings button to open typed controls for
the active configuration and keymap files. Profiles and themes use checked
dropdowns, the font picker searches installed families, and detected profiles
expose theme overrides.

Font size and scrollback accept typed values and press-and-hold steppers;
scrollback also supports a `Max` sentinel. Inactive-pane opacity uses a
percentage slider. Settings and font lists have independent scrollbars, and new
profiles are created in a labeled modal. Key bindings are grouped by context
with action dropdowns.

Saving validates the active page, persists and applies it without restarting,
closes the dialog, and returns focus to the terminal. Invalid settings or
bindings are reported without replacing the existing file. Custom `--config`
and `--keymap` paths remain CLI-only settings.

## Profiles and working directories

Zetta detects common shells. On Windows this includes Windows PowerShell,
PowerShell 7, Command Prompt, and registered WSL distributions.

Profile 1 is `System`, followed by detected profiles and configured `profiles`
in the order displayed by the profile menu. A configured profile with the same
name as a detected profile overrides it in place. Set `default_profile` to any
displayed name; matching is case-insensitive. Opening a profile from the menu
or a shortcut makes it the selection for subsequent `Ctrl-Shift-T` tabs.
Missing shortcut slots have no effect.

The first tab starts in the user's home directory unless `working_directory`
is set. Setting it to `"~"` is equivalent to leaving it unset. Later native
tabs and splits inherit the active pane's current directory.

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

GPUI represents shifted number-row keys by their US ASCII symbols. Use
`ctrl-!`, `ctrl-@`, through `ctrl-(` in custom keymaps and set
`"use_key_equivalents": true` on that keymap section, as demonstrated in
`keymap.example.json`.

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
  "terminal_font_size": 14,
  "terminal_font_family": "MesloLGS NF",
  "inactive_pane_opacity": 0.8,
  "pane_controls_position": "right",
  "max_scroll_history_lines": 2147483647
}
```

`terminal_font_size` accepts values from 6 through 100.
`terminal_font_family` accepts bundled and system-installed fonts.
`inactive_pane_opacity` accepts values from 0 through 1 and defaults to 0.8.
`pane_controls_position` accepts `"left"` or `"right"` and defaults to
`"right"`. It controls the pane overlay buttons independently of the system
window-button layout so they do not move over a left-aligned prompt unless you
choose that placement explicitly. Tab close buttons do follow the system
window-button side.

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
