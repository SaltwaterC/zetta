# Configuring Zetta

Use [`config.example.json`](../config.example.json) and
[`keymap.example.json`](../keymap.example.json) as starting points. A repository
can use [`project.config.example.json`](../project.config.example.json) as a
starting point for `.zetta/config.json`. These examples are not loaded
automatically.

## File locations and reloads

Zetta loads configuration from:

- Linux and macOS: `~/.config/zetta/config.json`
- Windows: `%APPDATA%\Zetta\config.json`

The keymap is `keymap.json` in the same platform-specific directory. Use
`--config PATH` and `--keymap PATH` to override these locations.

## Projects

A Zetta project is a registered directory with a `.zetta/config.json` file.
The registry is kept separately in `projects.json` beside Zetta's normal
configuration (`~/.config/zetta/projects.json` on Linux and macOS, or
`%APPDATA%\Zetta\projects.json` on Windows). Manage it from the **Projects**
tab in Settings or from the CLI:

```sh
zetta project add [PATH]
zetta project list
zetta project open [PATH]
zetta project remove [PATH]
```

`project add` creates `{}` at `.zetta/config.json` when the file is absent,
validates it, and records the canonical root. With no path it uses the nearest
native Git repository root, falling back to the current directory. `open` and
`remove` accept either a project root or a path inside one; `remove` never
deletes the repository's configuration. Plain `zetta` launched inside a
registered project opens that project, and `project open` opens a new active
tab in an existing Zetta process when possible. The deepest registered
ancestor wins for nested projects.

Zetta-managed `wt/*` linked worktrees are aliases for their registered main
repository: Zetta loads the main project's `.zetta/config.json`, keeps the
main root as the project context, and does not offer a second project import.
The worktree only changes the initial terminal directory, so launching plain
`zetta` or using `project open` from inside it starts there. This association
requires the main repository to be registered already; ordinary, detached, and
unregistered worktrees retain normal directory matching and import offers.

Project configuration is an overlay on the normal configuration and supports
these deliberately scoped fields:

- `theme` (light appearance), `dark_theme` (dark appearance), `default_profile`,
  `profiles`, and `default_tab_icon`
- `working_directory`, as an existing project-relative directory that cannot
  escape the project root, defaulting to the project root itself
- `env`, an object of string environment variables; reserved `ZETTA_*` names
  cannot be replaced
- `inactive_pane_opacity`
- `pane_split_templates`
- `initial_split`, naming a built-in or project-defined pane template

A tab opened for a project starts in that project's working directory,
regardless of `working_directory_scope`: opening a project is a move into it,
not a continuation of wherever the session happened to be. This covers `zetta`
launched inside a project, `project open`, **Open** in the Settings
**Projects** tab, and the command palette's `Zetta: Open Project: NAME (PATH)`
entry, which the palette lists for every registered project.

The active pane's current directory selects its project. The active tab then
controls the window theme. Moving outside the project immediately restores the
normal configuration and removes project-only templates from the command
palette; moving back restores them. Project environment values are inherited
by spawned terminals, with template and individual-pane values taking
precedence. `initial_split` replaces the active pane subtree once when a tab
first enters the project. An explicit `zetta --split NAME` takes precedence at
startup.

Zetta detects `.zetta/config.json` at native Git repository roots in a
background task and offers to register the project. This discovery does not
block startup or terminal input. WSL directories are not scanned; register a
WSL project explicitly and its reported UNC path is matched lexically.

Registration is a trust boundary. A project pane template can launch commands,
both as a pane's own program and as stacked commands seeded beside it, so add
only repositories whose `.zetta/config.json` you trust.

The Settings **Projects** tab can add, open, edit, and unregister projects.
**Edit config** opens a typed builder for every supported field, including the
same pane-template editor as the **Templates** tab: the application's templates
appear there as read-only presets, and overriding one or adding a new one
applies only inside the project. Controls left as *Inherit* stay absent from the
file, so the project keeps following the application configuration. **Save
project** validates the result the way loading it would — including that
`working_directory` still exists inside the project — and replaces the file
atomically, so a rejected edit never damages a working configuration. **Open in
editor** is still available for hand-editing through Zetta's normal
`$EDITOR`/built-in vi flow.

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
dropdowns, the font picker searches installed families, and every profile
exposes theme, icon, and individual visibility controls for the Profiles menu.
The Configuration page's **Background sessions (zmux)** section controls
detached/shared-session screen retention and its memory budget.

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

## Background-session retention

Background sessions are owned by the separate `zmux` daemon. Configure the
screen retained while a pane is detached or shared with:

```json
{
  "sessions": {
    "retention": "disk",
    "ring_bytes": 262144,
    "persistence": {
      "recipients": [
        "age1example...",
        "github:example-user"
      ],
      "identity": "~/.config/age/zetta-identity.txt",
      "auto_protect": false
    }
  }
}
```

`sessions.retention` accepts `"memory"` (the default), `"none"`, or `"disk"`.
`memory` keeps a bounded terminal grid and scrollback; `ring_bytes` is the
budget used to derive that bound and must be between 4096 and 67108864 bytes.
`none` keeps the processes alive but does not request or retain a detach
snapshot. `disk` keeps the same in-memory screen while detached and
additionally writes encrypted age v1 metadata and scrollback when
`persistence.recipients` is non-empty. Recipients may be native age X25519,
ML-KEM-768/X25519, SSH Ed25519, SSH RSA, or `github:USER`; post-quantum
recipients cannot be mixed with classical or SSH recipients. `identity` is
the default client-side identity file for resuming a disk record; it is never
sent to the daemon. With disk selected and no recipients, no persistence files
are written.

When disk retention includes a `github:USER` recipient and GitHub is temporarily
unreachable, Zetta keeps the requested disk setting but configures the daemon
with the configured `ring_bytes` memory budget until the lookup succeeds. This
is a temporary durability tradeoff: new detached or shared sessions remain
daemon-owned and attachable from another Zetta process, but no new encrypted
disk records are written during the fallback. Existing persistence files are
left untouched. Zetta retries in the background after 5, 10, 20, 40, and then
60 seconds (repeating at most every 60 seconds), and switches back to disk
without restarting the daemon or terminating running shells. Encrypted disk
resume is unavailable while the setting is degraded. Invalid usernames,
malformed keys, invalid direct recipients, and non-retryable GitHub responses
remain configuration errors.

The setting is global to the daemon and is applied when a Zetta process
connects or reloads configuration; an already-running daemon does not need to
be restarted. Use `zmux list` to see opaque restorable records and
`zmux resume SESSION -i PATH` to decrypt one;
`-i/--identity` may be repeated. `"persist"` is rejected with a migration
hint to `"disk"`.

If an SSH identity produces an `unknown cipher "aes256-gcm@openssh.com"` error
when it is passed to `age`, see [SSH identity cipher
compatibility](background-sessions.md#ssh-identity-cipher-compatibility).

### Protecting sessions with your key instead of a secret

`persistence.auto_protect` replaces the secret dialog with your age key pair.
Detaching, keeping, or sharing a tab then generates a 256-bit session key,
protects the session with it exactly as a typed secret would, and seals that key
to `persistence.recipients`. Reattaching opens the sealed key with
`persistence.identity`, so no secret is asked for in either direction — in a
window, or from `zmux reconnect` and `zmux resume`.

The session is still protected: attaching requires opening the sealed key, which
requires the private key. The multiplexer is unchanged by this — it stores the
same Argon2id verifier and never sees the identity.

It needs both a recipient and an identity, and the settings page only offers the
toggle once both are set, because either one missing would create sessions that
cannot be reattached. It applies under every `sessions.retention` mode; only the
recipients matter, not disk persistence.

**Lose the identity and automatically protected sessions cannot be reattached.**
There is no recovery path by design: the key exists only inside the envelope.
Keep a backup of the identity file, or add a second recipient you control.

Automatically protected sessions are listed as `Protected session` with no pane
details, the same as any other protected session, so the reconnect picker shows
less about them than it does for an unprotected detach.

The `--retention` option is a bootstrap option for an independently launched
`zmux --daemon`; it is not a live-state report. Zetta starts its daemon with
`--daemon` and applies the retention and persistence settings loaded from
`config.json` after the daemon is ready. The daemon's configured state, rather
than an old process command line, is authoritative.

## Git worktree root

The standalone `zwt` command and the compatible `zetta wt` commands use Git's
effective `wt.root` configuration; it is not a Zetta JSON setting. The
recommended repository-local configuration is:

```sh
git config --local wt.root ../project-worktrees
```

Relative values resolve from the repository's main worktree root. Absolute
values are used as written. If the setting is absent, Zetta defaults to the
sibling directory `<repository>-worktrees`. `zetta wt new` creates missing
parent directories and rejects existing destinations and symlink collisions;
`zetta wt status` reports the configured or default path without creating it,
along with current `HEAD` submodule paths and native CoW availability. A
missing root is checked through its nearest existing ancestor.

Run `zetta wt rerere` to enable the two global Git settings recommended for
this workflow: `rerere.enabled=true` and `rerere.autoupdate=true`.

## Profiles and working directories

Zetta detects common shells. On macOS and Linux, shells installed by Homebrew
are also detected as separate profiles (for example, `Bash (Homebrew)`), even
when the graphical application was not launched with Homebrew's `bin`
directory in its `PATH`. Use the exact resolved Homebrew name in configuration,
the settings editor, and CLI arguments; for example, configure Fish as
`Fish (Homebrew)`, not `Fish`:

```json
{
  "default_profile": "Fish (Homebrew)",
  "profiles": [
    { "name": "Fish (Homebrew)", "theme": "One Light", "dark_theme": "One Dark" }
  ]
}
```

On Windows this includes Windows PowerShell,
PowerShell 7, Command Prompt, MSYS2, Cygwin, and registered WSL distributions. The
MSYS2 Start Menu shortcut is used to find current custom installation paths;
legacy uninstall registration is also checked, with `C:\msys64` as a fallback.
Zetta launches `msys2_shell.cmd` in the MSYS environment so the normal MSYS2
initialization is retained. The launcher imports the Windows `PATH`, with
Zetta's installation directory placed first, so `zetta init` and other Zetta
commands are immediately available inside the profile. Bash and Zsh profiles
also report their current MSYS2 directory to Zetta, so new tabs, split panes,
multi-command panes, and local server actions inherit the directory after `cd`.

Cygwin is discovered from its per-user or machine-wide installation registry
entries (including 32-bit registry views), Cygwin paths found in `PATH`, and
the conventional `C:\cygwin64` and `C:\cygwin` roots. A custom installation
does not need to be on `PATH` when it is registered. The first root containing
`bin\cygwin1.dll` supplies the stable profiles `Cygwin`, `Cygwin: Zsh`,
`Cygwin: Fish`, and `Cygwin: Nushell`; a profile is omitted when its matching
`bin\*.exe` is not installed. These profiles launch the shell executable
directly from `bin` with `-l`, rather than through `Cygwin.bat`, so each shell
remains distinct. Cygwin's `/cygdrive/c/...` and UNC paths are converted to
native Windows paths for tracked directories and pane inheritance. Cygwin
startup keeps the inherited `PATH`, prepends the installation's `bin`, and
sets `CHERE_INVOKING=1` so configured or inherited native working directories
survive login startup.

Each profile also has an icon. When `icon` is omitted, `null`, or `"auto"`,
Zetta infers it from the shell executable on macOS and Linux (`bash`, `zsh`,
and `fish` get their bundled artwork; other programs use the Zetta fallback).
Windows profiles use the executable's native icon when it can be resolved and
extracted. WSL distributions use generic Tux artwork because WSL discovery does
not expose the distribution's default shell, while MSYS2 Bash and Zsh use the
corresponding bundled artwork. Cygwin Bash, Zsh, and Fish use the matching
bundled artwork; Cygwin Nushell uses the Zetta fallback. Set `icon` explicitly to `"zetta"`, `"bash"`,
`"zsh"`, or `"fish"` to override automatic selection. Explicit icons are
shown in the settings editor and profile menu; automatic selections are not
written back to the configuration file.

MSYS2 Bash appears as `MSYS2`. If `usr\bin\zsh.exe` is installed, Zetta also
adds `MSYS2: Zsh` and passes MSYS2's supported `-shell zsh` launcher option.
Select that profile as `default_profile` (or in the settings editor) to use
Zsh without relying on `chsh`:

```json
{
  "default_profile": "MSYS2: Zsh"
}
```

Cygwin profiles use the same stable names on every installation. For example,
select `Cygwin: Fish` as the default without including the installation path:

```json
{
  "default_profile": "Cygwin: Fish"
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
zetta profile theme "Bash" "One Light"
zetta profile theme "Bash" --reset
zetta profile dark-theme "Bash" "One Dark"
zetta profile dark-theme "Bash" --reset
zetta profile icon "Bash" fish
zetta profile icon "Bash" --reset
zetta profile default "Bash"
zetta profile add "Project Shell" --program bash --arg -l --theme "One Light" --dark-theme "One Dark" --icon bash
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
`"tab"` scope inherits for both new panes and new tabs. Tabs opened for a
project are the exception and always start in the project's own directory.

Detected WSL profiles start in the selected distribution's Linux home. Zetta
tracks the Linux directory for bash, fish, and zsh, with a fallback for other
shells, so same-profile tabs and splits inherit it even though `wsl.exe` exposes
only a Windows-side directory. On Windows, prompt integration similarly tracks
the active filesystem directory for Windows PowerShell, PowerShell 7, and
Command Prompt without replacing the user's prompt. Cygwin Bash, Zsh, Fish,
and Nushell profiles report their Cygwin directory after `cd`; `/cygdrive` and
root-relative paths are converted before native project, worktree, and pane
inheritance decisions are made.

Profiles may choose separate Zed themes and an icon independently from the
application themes. `theme` is used only for light appearance and `dark_theme`
only for dark appearance; if a dark value is absent, the global `dark_theme`
(or bundled `One Dark`) is used. A detected profile needs only its name, theme, or icon; its detected
command is retained. New profiles require `program`. Each terminal pane uses
its profile's theme, and each tab uses its active pane's theme for its background, text,
icons, border, and active indicator:

```json
{
  "default_profile": "Zsh",
  "profiles": [
    { "name": "Zsh", "theme": "Solarized Light", "dark_theme": "One Dark" },
    {
      "name": "Login Zsh",
      "program": "/bin/zsh",
      "args": ["-l"],
      "theme": "One Light",
      "dark_theme": "One Dark"
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

One shortcut exists per *visible* profile, numbered in menu order, and the set
follows the active [project](#projects): a project that adds a profile gets the
next free number, and one that hides an inherited profile releases a number.
Entering or leaving the project rebinds them, so the accelerator shown in the
Profile menu is always the one that works.

Zetta normalizes these physical keys on Linux so the shortcuts work with
layouts whose shifted characters differ. On Windows and macOS, shortcuts
follow the active keyboard mapping and are rebuilt when the layout changes.
`Ctrl-Alt` number-row fallbacks are not built in because they collide with
AltGr on layouts that use it.

## Appearance and scrollback

Zetta follows the operating system's light/dark appearance. It defaults to the
bundled `One Light` theme in light appearance and `One Dark` in dark appearance,
with MesloLGS NF as the font. Common appearance settings include:

```json
{
  "theme": "One Light",
  "dark_theme": "One Dark",
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
