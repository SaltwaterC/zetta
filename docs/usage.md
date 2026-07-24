# Using Zetta

## Profiles and tabs

Zetta creates profiles for common installed command interpreters. On Windows,
these include Windows PowerShell, PowerShell 7, Command Prompt, and registered
WSL distributions. Select a profile in the top bar, then open a new tab.

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
`Alt-Arrow` or the pointer to move focus. Exiting a shell removes its pane;
exiting the final pane closes the tab.

Pane controls appear when the pointer moves over a pane and hide after a short
period of inactivity. They can maximize, minimize, or close the pane. Each pane
also has a stable per-tab label that remains as panes are rearranged or closed.
Press `Alt-Shift-R` or double-click the label to assign a custom name; submit an
empty name to restore its automatic label.

A maximized pane has a status strip below it. Restore it from that strip or
with `Shift-Escape`.

Minimized panes appear on a shelf at the bottom of the tab. The shelf displays
as many complete entries as fit, including each pane's label and profile. Use
these shortcuts to operate it:

- `Alt-Shift-Down` minimizes the active pane.
- `Alt-Shift-Left` and `Alt-Shift-Right` move the shelf selection.
- `Alt-Shift-Up` restores the selected pane.

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
`keymap.json` for direct access:

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

## Search

`Alt-Shift-F` searches the active pane's scrollback. `Enter` and `F3` select
the next match, `Shift-Enter` and `Shift-F3` select the previous match, and
`Escape` closes search. In terminal vi mode, `/` also opens scrollback search.

`Ctrl-Shift-F` searches every pane in the active tab. It highlights all matches
and activates the pane containing the current result as you navigate.

## Command palette

`Ctrl-Shift-P` opens the command palette. It lists actions available in the
focused terminal and Zetta window, including effective shortcuts. Type to
filter, use the arrow keys to select a command, and press `Enter` to run it.

## Default shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl-Shift-T` | New tab |
| `Ctrl-Shift-N` | New window |
| `Ctrl-Shift-1` … `Ctrl-Shift-9` | New tab with profile 1 … 9 |
| `Ctrl-Shift-W` | Close tab |
| `Ctrl-Shift-D` | Detach the active tab into the background |
| `Ctrl-Shift-B` | Toggle automatic backgrounding for the active tab |
| `Ctrl-Shift-A` | Reconnect the most recently detached tab |
| `Ctrl-Shift-O` | Split active pane horizontally (top/bottom) |
| `Ctrl-Shift-E` | Split active pane vertically (left/right) |
| `Alt-Shift-L` | Rotate a two-pane layout |
| `Alt-Shift-X` | Close the active pane or its final tab |
| `PageUp` / `PageDown` | Send page navigation to the foreground program |
| `Shift-PageUp` / `Shift-PageDown` | Scroll history by one page |
| `Alt-Shift-A` | Select all terminal text |
| `Ctrl-Shift-Backspace` | Clear the system clipboard |
| `Alt-Arrow` | Focus the pane in that direction |
| `Alt-Shift-Down` | Minimize the active pane |
| `Alt-Shift-Left` / `Alt-Shift-Right` | Select the previous / next minimized pane |
| `Alt-Shift-Up` | Restore the selected minimized pane |
| `Shift-Escape` | Maximize or restore the active pane |
| `Ctrl-Shift-I` | Toggle input broadcasting in the active tab |
| `Ctrl-Tab` / `Ctrl-Shift-Tab` | Next / previous tab |
| `Ctrl-PageUp` / `Ctrl-PageDown` | Next / previous tab |
| `Ctrl-C` | Copy selected text or send interrupt |
| `Ctrl-V` | Paste |
| `Alt-Shift-F` | Search the active pane's scrollback |
| `Ctrl-Shift-F` | Search scrollback across the active tab |
| `Ctrl-Alt-V` | Paste with surrounding whitespace trimmed |
| `Alt-Shift-S` | Save the active pane's complete output |
| `Ctrl-Shift-P` | Open the command palette |
| `Ctrl-,` | Open the configuration and keymap editor |
| `Ctrl-Shift-S` | Open a serial console in a new pane |
| `Ctrl-Shift-R` | Rename the active tab |
| `Alt-Shift-R` | Label the active pane |
| `Ctrl-=` / `Ctrl-+` | Increase font size globally |
| `Ctrl--` | Decrease font size globally |
| `Ctrl-0` | Reset font size globally |
| `Alt-Shift-=` / `Alt-Shift-+` | Increase active pane font size |
| `Alt-Shift--` | Decrease active pane font size |
| `Alt-Shift-0` | Reset active pane font size |
| `Ctrl-Alt-R` | Reload configuration, keymap, and themes |
| `Ctrl-Shift-F12` | Toggle the performance overlay |

Unmodified function keys remain available to terminal applications.

Input broadcasting is scoped to the active tab and disabled by default. When
enabled, typing, terminal control keys, IME text, and pastes sent to the active
pane are also sent to every other open pane in that tab.

See [Configuration](configuration.md) to customize these bindings and
[Background sessions](background-sessions.md) for detach and reconnect details.
