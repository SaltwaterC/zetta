# Using Zetta

## Terminal size

Run `zetta terminal-size` to print the current terminal width in columns and
height in rows. Add `-j` or `--json` for machine-readable output. This works
from Zetta and other terminals on macOS, Linux, and Windows, including
PowerShell.

Inside Zetta, `zetta terminal-size -r -c 120 -R 40` (or
`--resize --columns 120 --rows 40`) resizes the pane that runs it. `--columns`
and `--rows` may be used independently; an omitted dimension remains
unchanged. Programs can make the same request with the standard xterm sequence
`CSI 8 ; rows ; columns t` (for example,
`\033[8;40;120t`).

## Profiles and tabs

Zetta creates profiles for common installed command interpreters. On Windows,
these include Windows PowerShell, PowerShell 7, Command Prompt, and registered
WSL distributions. Select a profile in the top bar, then open a new tab.
New tabs use the configured default profile unless `new_tab_profile` is set to
`"inherit"`.

Launch a profile directly with either form:

```sh
zetta --profile "PROFILE"
zetta -p "PROFILE"
```

Add `--theme "THEME"` (or `-t "THEME"`) alongside `--profile` to
non-persistently override that profile's theme for this launch only. The
override is never written to `config.json` or the profile itself, so the
Settings UI keeps showing the profile's real configured theme and the next
launch uses it again.

Manage persistent profile state with the typed `profile` command family:

```sh
zetta profile list
zetta profile themes
zetta profile disable "PROFILE"
zetta profile enable "PROFILE"
zetta profile theme "PROFILE" "THEME"
zetta profile theme "PROFILE" --reset
zetta profile icon "PROFILE" ICON
zetta profile icon "PROFILE" --reset
zetta profile default "PROFILE"
zetta profile add "NAME" --program PROGRAM [--arg ARG ...] [--theme THEME] [--icon ICON]
zetta profile remove "PROFILE"
```

Use `--config PATH` or `-c PATH` anywhere after `profile`, or use the root
form `zetta -c PATH profile ...`. Profile names are case-insensitive, and
`profile list` includes hidden and detected profiles. `profile themes` is the
source for valid profile themes. Mutations validate and write the selected
configuration while preserving its other settings, then request a best-effort
live reload from a matching Zetta process.

Profile icons accept `auto`, `zetta`, `bash`, `zsh`, and `fish`. `auto` and
`profile icon PROFILE --reset` restore automatic inference; automatic icon
choices are omitted from saved configuration. See the configuration guide for
the platform-specific discovery rules.

Launch the first tab with a configured pane layout using either form:

```sh
zetta --split quarters
zetta -s four-vertical
```

Run `zetta splits` to list the available configured names. The built-in names
are `quarters`, `four-vertical`, `three-left`, and `three-right`; configured
entries can override those templates or add new ones. `--split` can be
combined with `--profile` and `--theme`; it applies only to the initial
window.

Replace the active pane in an already running Zetta process with either a
configured layout or a different profile:

```sh
zetta --replace-pane --split quarters
zetta -r -s four-vertical --profile "PROFILE" --theme "THEME"
zetta --replace-pane --profile "PROFILE"
```

`--replace-pane` requires `--split` or `--profile` and targets the active pane
in the active window. Profile names are matched case-insensitively against the
resolved profile names. A split-only replacement keeps the current active
terminal and profile for its retained pane; a profile or theme override respawns the
retained pane, and the selected profile/theme is used for every pane in the
replacement layout. Surrounding panes, working directory inheritance, focus,
and pane limits are preserved. If no accepting Zetta process is available, the
command falls back to the normal new-window launch; `--config` and `--keymap`
always use that normal launch path.

Tab names follow the active terminal process. Press `Ctrl-Shift-R` or double-click
a tab to set a persistent name. Use `Ctrl-Shift-Y` or the tab context menu
to choose a tab icon. Submit an empty name to resume automatic naming.
From a Zetta pane, `zetta tabicon ICON` changes the active tab icon;
`zetta tabicon none` hides it, and `zetta tabicon --list` prints the available
built-in icon names.
Tabs retain a fixed width as their names change.

### Tab attention

Commands running in a Zetta terminal can mark their originating tab with
`zetta attention [OPTIONS] [SUMMARY] [BODY]`. The summary defaults to
`Attention required`, and the optional body appears in the badge tooltip.
The badge is non-animated, uses the active theme's accent color, and clears
when the tab becomes active and its terminal or minimized-pane shelf is
focused. It is kept in memory only and is not part of session persistence.

Pass `--notify` to also show a desktop notification. The notification reuses
the `notify` command's `--app-name`, `--icon`, `--sound`, and `--timeout`
options; those options require `--notify`. Attention commands outside a Zetta
terminal, or after their originating tab closes, fail rather than targeting
the current active tab. When a notification is issued from a Zetta terminal,
clicking its body activates the issuing window and visible tab. Dismissal,
expiry, replies, and other actions do not focus; closed, dormant, and
background-only tabs are no-ops. Packaged macOS app builds route clicks, while
unbundled development builds only display the notification. Process-wide
Silent mode and the issuing tab's Tab Silent Mode can suppress the terminal
bell and notification sounds without suppressing this badge, notification
content, or its actions. Toggle Tab Silent Mode from the tab's context menu or
the command palette; its bell-off indicator reflects only that tab's transient
setting.

Press `Ctrl-Shift-G`, or right-click a tab and choose the checked `Tab Move
Mode` entry, to make that tab the active moving tab. A `Move tab ← →` indicator
appears in the tab bar while the mode is active. `Left` and `Right` then move
the tab one position at a time without wrapping; normal terminal input is
paused. The mode stays active until you toggle the shortcut or menu entry
again, or use `Tab Move Mode` in the command palette. The moving tab remains
visible as tabs displaced from either end enter the corresponding overflow
menu. Selecting a tab from either overflow menu brings it into the visible
range, anchored on the side from which it was selected, before it can be moved.

## Panes

Splits inherit the active pane's working directory and selected profile. Use
`Cmd-Arrow` on macOS, `Alt-Arrow` on Windows/Linux, or the pointer to move
focus. Exiting a shell removes its pane when it has no stacked commands; a
pane with stacked commands keeps those entries available until they are
closed. Exiting the final empty pane closes the tab.

Pane controls appear when the pointer moves over a pane and hide after a short
period of inactivity. They can maximize, minimize, or close the pane. Each pane
also has a stable per-tab label that remains as panes are rearranged or closed.
The control strip shows the pane's live size next to its label. The maximized
pane status strip shows the same size.
Press `Cmd-Shift-R` on macOS or `Alt-Shift-R` on Windows/Linux, or double-click
the label to assign a custom name; submit an empty name to restore its automatic
label.

Press `Ctrl-Shift-J`, or right-click a pane and toggle "Pane Resize Mode" from
its context menu (shown once 2 or more panes are open), to enter or leave
pane-resize mode. While it is active, the arrow keys move the corresponding
edge of the active pane by one cell and
every visible pane shows its live cell dimensions; normal terminal input is
paused. Each split also exposes a 20px drag gutter, so you can resize either
axis directly with the mouse. For example, Left grows a right-hand pane and Up
grows a bottom pane.
Zetta first takes space from the nearest neighboring pane on that axis. If no
neighbor can give up a cell, it grows the window only within the current
display's usable bounds; a maximized or full-screen window is the hard growth
limit. The native client window never shrinks below the size required for its
window controls.

Press `Alt-Shift-M`, or `Cmd-Shift-M` on macOS, or right-click a pane and
toggle "Pane Move Mode" from its context menu (shown once 2 or more panes are
open), to enter or leave pane-move mode. While it is active, the active pane
shows a "Move mode" label and normal terminal input is paused. Arrow keys then
move the active pane one step in that direction: it swaps with whichever pane,
or group of panes, occupies the nearest matching side of the layout, so the
tiling always stays valid — for example, moving one of the two stacked panes in
the `three-right` template towards the large pane flips the whole layout to
`three-left`. If there is no neighboring pane or group in that direction, the
arrow key does nothing. Hovering a pane shows a grab cursor; drag it onto
another pane and release to swap the two panes directly, independent of the
layout's split structure. Leave move mode with the same shortcut; there is no
separate Escape binding.

A maximized pane has a status strip below it. Restore it from that strip or
with `Shift-Escape`.

Press `Alt-Shift-T`, or `Cmd-Shift-T` on macOS, or run **Change Pane Theme**
from the command palette to open a searchable list of registered themes.
Selecting one non-persistently changes only the active pane's theme; type to
filter, use the arrow keys and `Enter` to apply, or `Escape` to cancel. The
list always shows a checkmark next to the pane's current theme and pins
**Reset to profile default** at the top to clear the override and go back to
the profile's configured theme (or the global default, if the profile has
none). The override is not saved: it is not written to `config.json`, the
profile, or the settings view, so it disappears when the pane closes or the
configuration reloads.

The same change is available from any Zetta pane, or a script, with
`zetta panetheme THEME`; `zetta panetheme --reset` clears the override, and
`zetta panetheme --list` prints the theme names registered in the running
process (built-in and user-installed). Shell integration completes pane
theme names dynamically, the same way it does for `zetta tabicon`.

Run **Set Pane Overlay** from the command palette to edit text shown over the
active pane's terminal content. It renders as larger, translucent text in the pane's top-right
corner, with no background box, so it reads as a watermark rather than
obscuring the terminal underneath. The text is edited in place over the
pane, at full opacity while editing; type to change it, use the arrow keys
to move the cursor, and `Enter` to apply or `Escape` to cancel without
changing it. Submitting empty text clears the overlay. The overlay is not
saved: it is not written to `config.json`, so it disappears when the pane
closes or the configuration reloads.

While editing the overlay, press `Tab` to cycle through sections (Text, Font Size,
Color, Opacity). Each section has its own controls:

**Font Size:**
- `←` / `→` — Decrease/increase font size
- `Home` / `End` — Jump to smallest/largest size

**Color:**
- Click a labelled preset swatch to choose a standard named colour
- `←` / `→` — Adjust saturation
- `↑` / `↓` — Adjust brightness
- `Shift-←` / `Shift-→` — Adjust hue
- `Backspace` — Delete hex digit
- Type hex digits — Enter color as hex

**Opacity:**
- `←` / `↓` — Decrease opacity
- `→` / `↑` — Increase opacity
- `Home` / `End` — Jump to 0% / 100%

In any section:
- `Tab` / `Shift-Tab` — Next/previous section
- `Enter` — Apply
- `Escape` — Cancel

The same change is available from any Zetta pane, or a script, with
`zetta overlay TEXT`; `zetta overlay --reset` clears the overlay. Add
`--size SIZE` (`sm`, `base`, `lg`, `xl` (default), `2xl`, or `3xl`),
`--opacity PERCENT` (`0`-`100`, default `85`), and `--color COLOR` (an
`rgb`, `rgba`, `rrggbb`, or `rrggbbaa` hex value, with or without a leading
`#`, or one of the named presets `black`, `white`, `gray`, `red`, `orange`,
`yellow`, `green`, `cyan`, `blue`, `purple`, `magenta`, and `pink`) to
customize the font size, transparency, and text color. Named colours are
case-insensitive and ignore surrounding whitespace; the leading
`#` is optional and can be omitted, since most shells treat it as a comment
and would otherwise require quoting it. These apply together with the text in
the same invocation; running `zetta overlay` again fully replaces the
previous text and style rather than merging with it.

Rotate the layout around the active pane with `Alt-Shift-L` clockwise or
`Alt-Shift-K` counter-clockwise on Windows/Linux. macOS uses the corresponding
`Cmd-Shift-L` and `Cmd-Shift-K` shortcuts. Rotation is recursive: a focused
pane in an equal local pair rotates that pair without changing the surrounding
layout; a focused dominant pane rotates with the panes occupying the matching
area on the other side of its split. Equal four-pane groups, such as the
`quarters` template, rotate as a complete group. These rules apply at any
nested level. Split proportions are preserved during rotation; two turns in
either direction swap a two-pane pair in the view.

Minimized panes appear on a shelf at the bottom of the tab. The shelf displays
as many complete entries as fit, including each pane's label and profile. Use
these shortcuts to operate it:

- `Cmd-Shift-Down` on macOS / `Alt-Shift-Down` on Windows/Linux minimizes the
  active pane.
- `Cmd-Shift-Left` / `Cmd-Shift-Right` on macOS or `Alt-Shift-Left` /
  `Alt-Shift-Right` on Windows/Linux move the shelf selection.
- `Cmd-Shift-Up` on macOS / `Alt-Shift-Up` on Windows/Linux restores the
  selected minimized pane.

The same actions are available from the command palette.

## Multi-command prompt

Press `Ctrl-Shift-M` to open the multi-command prompt. For example:

```sh
run {{dev,prod}} {{eu,us}}
```

Zetta expands the Cartesian product, tiles the active pane into four panes, and
runs one command in each. Multiple and nested comma brace lists are supported.
Single braces, quoted double braces, and escaped double braces are left for the
shell. Commands without a double-brace list run in one pane. Templates are
limited to 64 KiB so pasted input cannot monopolize the UI during expansion.

Panes use the resolved parameters as their automatic labels: `dev · eu`,
`dev · us`, `prod · eu`, and `prod · us` in this example. A custom pane label
takes precedence; clearing it restores the generated label.

The prompt provides native completion. Use `Tab` and `Shift-Tab` to cycle
through executables from `PATH`, paths relative to the active pane's working
directory, and SSH aliases declared by `Host` entries in `~/.ssh/config`.

## Stacked command panes

Press `Alt-Shift-N` (`Cmd-Shift-N` on macOS) to open the command prompt for a
single stacked command. The command runs in a task-backed PTY using the active
pane's profile and current working directory. Stacked entries share the host
pane's layout region: the selected terminal is expanded and every other entry
is a one-row command/status line. The PTYs remain interactive, so `Ctrl-C`
and other terminal input work normally; stacked input is not sent to sibling
panes when input broadcasting is enabled.

Press `Alt-Shift-[` / `Alt-Shift-]` (`Cmd` on macOS) to select the previous or
next entry. On Linux, the shifted bracket events are also accepted as
`Alt-{` / `Alt-}`. Cycling includes the original interactive terminal, wraps
at both ends, and focuses the selected terminal. A completed command keeps its
output and status, including a numeric exit code when one is available. Click
a row to select it, then use that row's close button or `Alt-Shift-X` / the
corresponding macOS `Cmd-Shift-X` to close the selected stacked entry. The
pane-control strip is hidden while stacking is enabled. Closing the host pane
explicitly closes all of its stacked entries as well.

## Git worktrees

Zetta includes a small, safety-focused Git worktree workflow for temporary
branches:

```sh
zetta wt new feature/api
zetta wt status
zetta wt done
```

`new NAME` creates `wt/NAME` from the current attached branch, records the
source branch in `wtbranch.<branch>.base`, and creates the worktree below
`wt.root`. Nested names such as `feature/api` are supported. Configure a
repository-specific root with Git:

```sh
git config --local wt.root ../project-worktrees
```

Relative roots resolve from the repository's main worktree. Without `wt.root`,
Zetta uses the sibling directory `<repository>-worktrees`. The root is created
on demand by `new`; `status` only reports its resolved path and never creates
directories.

`status` also reports whether the current `HEAD` contains submodules and lists
detected submodule paths, including nested paths. It reports whether native
copy-on-write cloning is available between the current worktree and the
resolved `wt.root`; when that root is missing, the nearest existing ancestor is
inspected without creating it.

Use repeatable `--copy PATH` (or `-c PATH`) options to copy files, directories,
or symlinks from the current source worktree into the identical relative
locations in the new worktree:

```sh
zetta wt new --copy .env.local --copy .cache feature/api
```

Copy paths must be relative, may not contain parent-directory traversal, and may
not traverse an intermediate symlink. Requested paths must not overlap, and the
destination path must not already exist. Symlinks inside copied directories are
recreated as symlinks rather than followed. Zetta uses native copy-on-write
cloning on supported filesystems (Btrfs or reflink-enabled XFS on Linux, APFS on
macOS, and ReFS block cloning on Windows) and falls back to a regular recursive
copy when cloning is unavailable. A failed copy removes the new worktree,
temporary branch, metadata, and directories created for the worktree root.

When the source commit contains submodules, `new` initializes them recursively
at the gitlink commits recorded by that source tree. For each submodule, an
initialized matching checkout in the source worktree is supplied to Git as a
local `--reference`, so its objects can be reused without copying them. Missing
source checkouts omit the reference and use the submodule's configured remote
instead. Nested modules are initialized from their immediate parent, preserving
the corresponding reference at every level; remote branch tips are never used.
If initialization fails, Zetta force-removes the partial worktree, deletes its
temporary branch, and clears the recorded metadata.

`done` must run from a clean, linked `wt/*` worktree created by `new`. It
rebases onto the recorded source branch, checks that the source worktree is
still attached and clean (including submodule changes), fast-forwards the source
branch, then removes the temporary worktree, branch, and metadata. A worktree
whose current commit contains submodules is removed with Git's forced cleanup
after integration. A conflict leaves the rebase in place: resolve the files,
stage them with `git add`, and rerun `zetta wt done`.
Run `zetta wt rerere` once to enable Git's `rerere.enabled` and
`rerere.autoupdate` helpers for repeated conflicts.

When `new NAME` or `done` is run from a terminal opened by Zetta, the originating
tab is updated after the Git operation succeeds: `new NAME` pins its worktree
title to the exact `NAME` (including nested names such as `feature/api`) for the
worktree lifecycle, and a successful `done` clears it. Zetta also detects a
linked worktree from each pane's interactive-shell directory. The active pane's
detected name supplies the automatic title and clears when that shell leaves.
Only linked worktrees on `wt/<name>` branches are named; the main worktree,
detached heads, and other branches are ignored. Manual tab renames take
precedence over pinned and automatic worktree titles, followed by
process-control and terminal-derived titles. An empty manual rename reveals the
pinned title again when one exists. This is best-effort integration with Zetta;
missing or unavailable Zetta process control never changes the Git result or
`--path-only` output.

The direct CLI never changes the caller's directory. After enabling shell
integration, `zwt new NAME` changes into the new worktree and `zwt done`
changes into the integrated source worktree. Use `--path-only` (or `-P`) with
`new` or `done` when scripting; it reserves standard output for exactly one
path, while errors and `new`'s phase progress remain on standard error.

## Pane split templates

The parameterized `zetta::ApplyPaneSplitTemplate` action replaces the active
pane with a reusable layout. Built-in templates are:

- `three-right`: one pane on the left, two stacked on the right
- `three-left`: two stacked on the left, one pane on the right
- `quarters`: a 2-by-2 grid
- `four-vertical`: four equal-width columns

Each is available by name in the command palette. Add bindings like these to
`keymap.json` for direct access. On macOS, replace `alt` with `cmd` in these
custom bindings:

```json
{
  "alt-shift-o": [
    "zetta::ApplyPaneSplitTemplate",
    { "name": "three-right" }
  ],
  "alt-shift-e": [
    "zetta::ApplyPaneSplitTemplate",
    { "name": "quarters" }
  ]
}
```

The directional split actions are also available in the command palette.
`zetta::SplitHorizontalDown` and `zetta::SplitVerticalRight` have the default
shortcuts below; `zetta::SplitHorizontalUp` and `zetta::SplitVerticalLeft` are
unbound by default. Add custom bindings when needed:

```json
[
  {
    "context": "Zetta > Terminal",
    "bindings": {
      "ctrl-alt-shift-up": "zetta::SplitHorizontalUp",
      "ctrl-alt-shift-left": "zetta::SplitVerticalLeft"
    }
  }
]
```

On macOS, use `ctrl-cmd-shift` instead of `ctrl-alt-shift` for these custom
bindings.

Templates are recursive. Each leaf is an object; an empty object is an
unlabeled pane, while `{ "label": "label" }` assigns a label to that leaf.
Labels must use lowercase kebab-case (`[a-z0-9]+(?:-[a-z0-9]+)*`); duplicate
labels are allowed. `vertical` places two children side by side, and
`horizontal` stacks two children. Define named templates in `config.json`:

```json
{
  "pane_split_templates": {
    "three-bottom": {
      "horizontal": [
        { "label": "top" },
        {
          "vertical": [
            { "label": "bottom-left" },
            { "label": "bottom-right" }
          ]
        }
      ]
    }
  }
}
```

Each split must have exactly two children and each template may contain 2–64
panes. A tab is limited to 64 panes in total, including panes created by
recursive applications. Custom entries extend the built-ins and may override
them by using the same name.

Leaves can independently select a configured profile or direct command,
override its theme, add string environment variables, and show an overlay:

```json
{
  "pane_split_templates": {
    "server-pair": {
      "vertical": [
        {
          "label": "server",
          "profile": "Bash",
          "theme": "One Dark",
          "env": { "ROLE": "server" },
          "overlay": {
            "text": "SERVER",
            "size": "xl",
            "opacity": 85,
            "color": "cyan"
          }
        },
        {
          "label": "client",
          "command": { "program": "ssh", "args": ["host"] }
        }
      ]
    }
  }
}
```

`profile` and `command` are mutually exclusive. A command is launched with
exactly the listed program and arguments, without shell-string splitting.
Omitting both inherits the active pane's profile. Environment values must be
strings, overlay sizes are `sm`, `base`, `lg`, `xl`, `2xl`, or `3xl`, opacity
is a percentage from 0 to 100, and color accepts the named overlay colors or a
hex value.

The active terminal becomes the first, top-left leaf and retains focus. Leaves
that omit both `profile` and `command` inherit its profile; all new panes keep
the existing working-directory inheritance rules. Applying a template again
therefore recurses into the active pane without changing the rest of the tab.
Labeled leaves replace automatic pane labels when a template is applied;
manually assigned labels still take precedence, and an unlabeled leaf restores
the `Pane N` fallback.

## Clipboard

Selecting terminal text copies it to the system clipboard while preserving the
selection. `Ctrl-C` copies an existing selection and sends an interrupt when
nothing is selected, while `Ctrl-Insert` copies selected text. `Ctrl-V` and
`Shift-Insert` paste; `Ctrl-V` takes precedence over the shell's traditional
quoted-insert use of that chord.

A plain right-click pastes when the clipboard contains text and opens the
context menu when it does not. `Shift`-right-click always opens the context
menu. **Paste Trimmed** removes leading and trailing whitespace while preserving
whitespace inside the text. Middle-click is passed to the terminal as a mouse
event; it is not a paste gesture.

Ctrl-Shift-click a file path on Windows/Linux, or Cmd-Shift-click it on macOS,
to open the path in `$EDITOR` (or `$env:EDITOR` on Windows). If the variable is
unset, Zetta falls back to `zetta vi`. The editor runs in the active terminal
pane, so terminal editors remain attached to that pane and the pane's current
`EDITOR` value is used.

`Alt-Shift-V` on Windows/Linux or `Cmd-Shift-V` on macOS writes the active
pane's complete retained scrollback to a private managed file and opens it the
same way. Linux uses `/dev/shm` when available and falls back to
`$XDG_CACHE_HOME/zetta` or `~/.cache/zetta`; macOS and Windows use their
per-user temporary directories. Files are randomly named, owner-only on Unix,
and deleted as soon as the editor command returns. Zetta also performs
asynchronous garbage collection at startup, before creating another buffer,
and once per second while managed files exist, removing files left by editor
or application crashes without polling when there is nothing to collect.
A buffer whose editor handoff is not claimed is reaped after a 30-second grace period.
Editors that delegate to an existing GUI process should include their wait
option in `EDITOR` so the managed file remains available until editing ends.
**Edit Scrollback** is also available from the terminal context menu and command
palette.

## Built-in vi syntax highlighting

When the optional `syntax-highlighting` feature is enabled (it is included in
the default build), `zetta vi` uses bundled Tree-sitter grammars and Zed's
highlight queries. Language selection follows the bundled Zed grammar metadata
for file names, suffixes, and first-line patterns.

The supported upstream grammar registry is:

- Bash
- C and C++
- CSS
- Diff
- Git commit messages
- Go, Go modules (`go.mod`), and Go workspaces (`go.work`)
- JSDoc
- JSON and JSONC
- Markdown and Markdown Inline
- Python
- Regular expressions
- Rust
- TSX and TypeScript
- YAML

Zetta also provides pluggable grammar extensions for Makefiles (including
`Makefile`, `GNUmakefile`, `.mk`, and `.mak`) and TOML files. These use the
same embedded config/query setup without modifying Zed's upstream grammar
bundle.

Markdown fenced code blocks can use the other registered grammars, such as
Rust, JSONC, TSX, and TypeScript. `Markdown Inline`, JSDoc, and regular
expressions are also used when included by another grammar's Zed query; they
are not guaranteed to have standalone file-name detection.

## Search

`Cmd-Shift-F` on macOS / `Alt-Shift-F` on Windows/Linux searches the active
pane's scrollback. `Enter` and `F3` select the next match, `Shift-Enter` and
`Shift-F3` select the previous match, and `Escape` closes search. In terminal vi
mode, `/` also opens scrollback search.

`Ctrl-Shift-F` searches every pane in the active tab. It highlights all matches
and activates the pane containing the current result as you navigate.

## Command palette

`Ctrl-Shift-P` opens the command palette. It lists actions available in the
focused terminal and Zetta window, including effective shortcuts. Type to
filter, use the arrow keys to select a command, and press `Enter` to run it.

## Default shortcuts

On macOS, `Cmd` replaces `Alt` in the shortcuts below, except for `Alt-Space`,
which opens Zetta's title-bar menu on every platform. `Ctrl-Alt` combinations
become `Ctrl-Cmd`; for example, paste-trim is `Ctrl-Cmd-V` on macOS and
`Ctrl-Alt-V` on Windows/Linux.

| Shortcut | Action |
| --- | --- |
| `Ctrl-Shift-T` | New tab |
| `Ctrl-Shift-N` | New window |
| `Cmd-H` / `Cmd-Shift-H` (macOS) | Hide the application |
| `Ctrl-Shift-H` | Minimize the current window; a Linux compositor may ignore the request |
| `Ctrl-Shift-1` … `Ctrl-Shift-9` | New tab with profile 1 … 9 |
| `Ctrl-Shift-W` | Close tab |
| `Ctrl-Shift-G`, then `Left` / `Right` | Toggle tab-move mode and move the active tab without wrapping |
| `Ctrl-Shift-D` | Detach the active tab into the background |
| `Ctrl-Shift-B` | Toggle automatic backgrounding for the active tab |
| `Ctrl-Shift-A` | Reconnect the most recently detached tab |
| `Ctrl-Shift-O` | Split active pane horizontally, adding a pane below |
| `Ctrl-Shift-E` | Split active pane vertically, adding a pane on the right |
| `Alt-Space` | Open Zetta's title-bar menu |
| `Cmd-Shift-L` (macOS) / `Alt-Shift-L` (Windows/Linux) | Rotate the active pane layout clockwise, recursively |
| `Cmd-Shift-K` (macOS) / `Alt-Shift-K` (Windows/Linux) | Rotate the active pane layout counter-clockwise, recursively |
| `Ctrl-Shift-J`, then Arrow keys or a split gutter drag | Toggle pane-resize mode; resize panes |
| `Cmd-Shift-M` (macOS) / `Alt-Shift-M` (Windows/Linux), then Arrow keys | Toggle pane-move mode; move panes |
| `Cmd-Shift-X` (macOS) / `Alt-Shift-X` (Windows/Linux) | Close the selected stacked entry, or the active pane/final tab when the host terminal is selected |
| `PageUp` / `PageDown` | Send page navigation to the foreground program |
| `Shift-PageUp` / `Shift-PageDown` | Scroll history by one page |
| `Cmd-Shift-A` (macOS) / `Alt-Shift-A` (Windows/Linux) | Select all terminal text |
| `Ctrl-Shift-Backspace` | Clear the system clipboard |
| `Cmd-Arrow` (macOS) / `Alt-Arrow` (Windows/Linux) | Focus the pane in that direction |
| `Cmd-Shift-Down` (macOS) / `Alt-Shift-Down` (Windows/Linux) | Minimize the active pane |
| `Cmd-Shift-Left` / `Cmd-Shift-Right` (macOS) / `Alt-Shift-Left` / `Alt-Shift-Right` (Windows/Linux) | Select the previous / next minimized pane |
| `Cmd-Shift-Up` (macOS) / `Alt-Shift-Up` (Windows/Linux) | Restore the selected minimized pane |
| `Cmd-Shift-N` (macOS) / `Alt-Shift-N` (Windows/Linux) | Open the stacked-command prompt |
| `Cmd-Shift-[` / `Cmd-Shift-]` (macOS) / `Alt-Shift-[` / `Alt-Shift-]` (Windows/Linux) | Select the previous / next stacked entry |
| `Shift-Escape` | Maximize or restore the active pane |
| `Ctrl-Shift-I` | Toggle input broadcasting in the active tab |
| `Ctrl-Shift-S` | Toggle process-wide Silent mode |
| `Ctrl-Tab` / `Ctrl-Shift-Tab` | Next / previous tab |
| `Ctrl-PageUp` / `Ctrl-PageDown` | Next / previous tab |
| `Ctrl-C` | Copy selected text or send interrupt |
| `Ctrl-Insert` | Copy selected text |
| `Ctrl-V` | Paste |
| `Shift-Insert` | Paste |
| `Cmd-Shift-F` (macOS) / `Alt-Shift-F` (Windows/Linux) | Search the active pane's scrollback |
| `Ctrl-Shift-F` | Search scrollback across the active tab |
| `Ctrl-Cmd-V` (macOS) / `Ctrl-Alt-V` (Windows/Linux) | Paste with surrounding whitespace trimmed |
| `Cmd-Shift-S` (macOS) / `Alt-Shift-S` (Windows/Linux) | Save the active pane's complete output |
| `Ctrl-Shift-P` | Open the command palette |
| `Ctrl-,` | Open the configuration and keymap editor |
| `Ctrl-Shift-R` | Rename the active tab |
| `Ctrl-Shift-Y` | Change the active tab icon |
| `Cmd-Shift-T` (macOS) / `Alt-Shift-T` (Windows/Linux) | Change the active pane's theme (non-persistent) |
| `Cmd-Shift-R` (macOS) / `Alt-Shift-R` (Windows/Linux) | Label the active pane |
| `Cmd-Shift-B` (macOS) / `Alt-Shift-B` (Windows/Linux) | Set the active pane's overlay text (non-persistent) |
| `Tab`, `←`, `→`, `↑`, `↓`, `Shift-←`, `Shift-→`, `Home`, `End`, `Backspace` (in overlay picker) | Adjust overlay text, font size, color (hue/saturation/brightness/hex), and opacity |
| `Ctrl-=` / `Ctrl-+` | Increase font size globally |
| `Ctrl--` | Decrease font size globally |
| `Ctrl-0` | Reset font size globally |
| `Cmd-Shift-=` / `Cmd-Shift-+` (macOS) / `Alt-Shift-=` / `Alt-Shift-+` (Windows/Linux) | Increase active pane font size |
| `Cmd-Shift--` (macOS) / `Alt-Shift--` (Windows/Linux) | Decrease active pane font size |
| `Cmd-Shift-0` (macOS) / `Alt-Shift-0` (Windows/Linux) | Reset active pane font size |
| `Ctrl-Cmd-R` (macOS) / `Ctrl-Alt-R` (Windows/Linux) | Reload configuration, keymap, and themes |
| `Ctrl-Shift-F12` | Toggle the performance overlay |

Unmodified function keys remain available to terminal applications.

Input broadcasting is scoped to the active tab and disabled by default. When
enabled, typing, terminal control keys, IME text, and pastes sent to the active
pane are also sent to every other open pane in that tab.

See [Configuration](configuration.md) to customize these bindings and
[Background sessions](background-sessions.md) for detach and reconnect details.
