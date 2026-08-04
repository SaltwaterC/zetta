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

Tab names follow the active terminal process. Press `Ctrl-Shift-R` or double-click
a tab to set a persistent name. Submit an empty name to resume automatic naming.
Tabs retain a fixed width as their names change.

## Panes

Splits inherit the active pane's working directory and selected profile. Use
`Cmd-Arrow` on macOS, `Alt-Arrow` on Windows/Linux, or the pointer to move
focus. Exiting a shell removes its pane;
exiting the final pane closes the tab.

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

A maximized pane has a status strip below it. Restore it from that strip or
with `Shift-Escape`.

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

## Pane split templates

The parameterized `zetta::ApplyPaneSplitTemplate` action replaces the active
pane with a reusable layout. Built-in templates are:

- `three-right`: one pane on the left, two stacked on the right
- `three-left`: two stacked on the left, one pane on the right
- `quarters`: a 2-by-2 grid

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

Templates are recursive. `"pane"` is a leaf, `vertical` places two children
side by side, and `horizontal` stacks two children. Define named templates in
`config.json`:

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

Each split must have exactly two children and each template may contain 2–64
panes. A tab is limited to 64 panes in total, including panes created by
recursive applications. Custom entries extend the built-ins and may override
them by using the same name.

The active terminal becomes the first, top-left leaf and retains focus. New
panes inherit its profile and working directory. Applying a template again
therefore recurses into the active pane without changing the rest of the tab.

## Clipboard

Selecting terminal text copies it to the system clipboard while preserving the
selection. `Ctrl-C` copies an existing selection and sends an interrupt when
nothing is selected. `Ctrl-V` pastes and takes precedence over the shell's
traditional quoted-insert use of that chord.

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
| `Ctrl-Shift-1` … `Ctrl-Shift-9` | New tab with profile 1 … 9 |
| `Ctrl-Shift-W` | Close tab |
| `Ctrl-Shift-D` | Detach the active tab into the background |
| `Ctrl-Shift-B` | Toggle automatic backgrounding for the active tab |
| `Ctrl-Shift-A` | Reconnect the most recently detached tab |
| `Ctrl-Shift-O` | Split active pane horizontally, adding a pane below |
| `Ctrl-Shift-E` | Split active pane vertically, adding a pane on the right |
| `Alt-Space` | Open Zetta's title-bar menu |
| `Cmd-Shift-L` (macOS) / `Alt-Shift-L` (Windows/Linux) | Rotate a two-pane layout |
| `Ctrl-Shift-J`, then Arrow keys or a split gutter drag | Toggle pane-resize mode; resize panes |
| `Cmd-Shift-X` (macOS) / `Alt-Shift-X` (Windows/Linux) | Close the active pane or its final tab |
| `PageUp` / `PageDown` | Send page navigation to the foreground program |
| `Shift-PageUp` / `Shift-PageDown` | Scroll history by one page |
| `Cmd-Shift-A` (macOS) / `Alt-Shift-A` (Windows/Linux) | Select all terminal text |
| `Ctrl-Shift-Backspace` | Clear the system clipboard |
| `Cmd-Arrow` (macOS) / `Alt-Arrow` (Windows/Linux) | Focus the pane in that direction |
| `Cmd-Shift-Down` (macOS) / `Alt-Shift-Down` (Windows/Linux) | Minimize the active pane |
| `Cmd-Shift-Left` / `Cmd-Shift-Right` (macOS) / `Alt-Shift-Left` / `Alt-Shift-Right` (Windows/Linux) | Select the previous / next minimized pane |
| `Cmd-Shift-Up` (macOS) / `Alt-Shift-Up` (Windows/Linux) | Restore the selected minimized pane |
| `Shift-Escape` | Maximize or restore the active pane |
| `Ctrl-Shift-I` | Toggle input broadcasting in the active tab |
| `Ctrl-Tab` / `Ctrl-Shift-Tab` | Next / previous tab |
| `Ctrl-PageUp` / `Ctrl-PageDown` | Next / previous tab |
| `Ctrl-C` | Copy selected text or send interrupt |
| `Ctrl-V` | Paste |
| `Cmd-Shift-F` (macOS) / `Alt-Shift-F` (Windows/Linux) | Search the active pane's scrollback |
| `Ctrl-Shift-F` | Search scrollback across the active tab |
| `Ctrl-Cmd-V` (macOS) / `Ctrl-Alt-V` (Windows/Linux) | Paste with surrounding whitespace trimmed |
| `Cmd-Shift-S` (macOS) / `Alt-Shift-S` (Windows/Linux) | Save the active pane's complete output |
| `Ctrl-Shift-P` | Open the command palette |
| `Ctrl-,` | Open the configuration and keymap editor |
| `Ctrl-Shift-S` | Open a serial console in a new pane |
| `Ctrl-Shift-R` | Rename the active tab |
| `Cmd-Shift-R` (macOS) / `Alt-Shift-R` (Windows/Linux) | Label the active pane |
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
