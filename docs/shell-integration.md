# Shell integration

Zetta can emit a shell-specific integration script with completion for its
subcommands, flags, and flag values. Profile and theme names are fetched at
completion time from `zetta profile list` and `zetta profile themes`, so
`zetta --profile <Tab>` and profile-management commands use the selected
configuration's current state. Completing a value that contains spaces or
quotes (such as a profile named `Project Shell` or a theme named `Gruvbox Light
Hard`) inserts it as a single argument, so the completed line runs without
manual quoting. The script also provides `zvi`, an
unconditional shortcut for
the built-in vi editor, `ztftp`, a shortcut for the built-in TFTP client,
`zntfy`, a shortcut for sending desktop notifications, and `zcopy`/`zpaste`,
shortcuts for the clipboard; each has the same completion as its corresponding
`zetta` command. The top-level `attention` command also completes its long
notification options while retaining all short aliases.

`zmux`, the standalone multiplexer binary, gets the same completion as
`zetta mux`, since the two take identical arguments.
The mux command list includes `list`, `stop`, `reconnect`, `share`, `unshare`,
`kill`, and `forget`; the session argument for `reconnect`, `share`, `unshare`,
`kill`, and `forget` is
fetched dynamically from `zmux list`/`zetta mux list` in the full
`PROCESS:RUNNER:SESSION` form. Bare numeric IDs remain accepted as a
compatibility shorthand. The root `--no-mux` opt-out is completed as a long option;
its short `-n` spelling remains available to the parser without adding noise
to completion candidates.

Inside a shell launched by `zetta --no-mux` (or `zetta -n`), the integration
knows that no daemon owns that shell's background sessions. It offers only
`list` and `reconnect` for `zetta mux` and `zmux`, and their `--json`, `--help`,
and `--version` options; daemon-only commands and options are omitted. The
same reduced surface is shown by `zetta mux --help` in that shell.

Completion intentionally offers the same catalog IDs for `share` and
`reconnect`: `share` changes a scoped session to shared mode, while
`reconnect` is the action that opens it in a Zetta window.

It also provides `zwt`, a wrapper for the Git worktree workflow. `zwt new NAME`
captures `zetta wt new --path-only NAME` and changes into the created worktree;
`zwt done` similarly changes into the integrated source worktree. Other `zwt`
operations pass through to `zetta wt`. Arguments are forwarded as literal shell
arguments, so nested names and paths containing spaces are supported. The
completion scripts offer `wt`, `new`, `done`, `status`, `rerere`, the long
`--copy` and `--path-only` flags, and filesystem completion for copy paths. The
short `-c` and `-P` forms remain accepted by the CLI but are omitted from the
candidate list to keep completion concise.

The root `--split`/`-s` launch option runs `zetta splits` at completion time,
so it completes every currently configured layout name without embedding a
hardcoded list in the integration script. Run `zetta splits` directly to
print those names, one per line.

The root `--replace-pane`/`-r` modifier is also completed as a flag without a
value. Combine it with `--split`/`-s` or `--profile`/`-p`; split names,
profiles, and profile themes continue to complete dynamically. When no
running process accepts the request, Zetta keeps the normal launch behavior.

If `EDITOR` is not already set, it defaults to `zetta vi`. When no `vi`
command, alias, function, or other executable is already available, the
integration adds `vi` as a wrapper for Zetta's built-in editor.
On every platform other than macOS, the script also defines `pbcopy` and
`pbpaste` as the same shortcuts as `zcopy`/`zpaste`, taking priority over any
preexisting `pbcopy`/`pbpaste` alias so that muscle memory from macOS keeps
working there too; macOS already has real `pbcopy`/`pbpaste`, so Zetta leaves
them untouched there.

Serial-device completion is dynamic: completing `zetta serial console --device`
runs `zetta serial list` at completion time. A serial device connected after
the integration was generated is therefore available without rerunning
`zetta init`.

Tab-icon completion is dynamic too: completing `zetta tabicon` runs
`zetta tabicon --list` at completion time, so the generated script does not
embed the built-in icon list. Use `zetta tabicon ICON` (or
`zetta tabicon --icon ICON`) from a Zetta pane; `none` hides the active tab
icon.

Theme completion works the same way: completing `zetta theme pane` or
`zetta theme tab` runs `zetta theme <scope> --list` at completion time against
the running Zetta process, so the generated script does not embed a theme list
and always offers whatever that process has registered, including
user-installed themes. Use `zetta theme pane THEME` or `zetta theme tab THEME`
(or the corresponding `--theme THEME` form) from a Zetta pane to set a
session-scoped theme. The choice survives backgrounding, reconnect, and
encrypted disk resume, but not pane/tab close or a configuration reload.
`zetta theme pane --reset` falls back to the tab theme; `zetta theme tab
--reset` restores the configured theme. `--theme`
(or `-t`) also completes profile themes when typed after `--profile` at launch,
since it non-persistently overrides that profile's theme for the new window.

Profile administration uses the non-GUI endpoint too. `zetta profile list`
supplies root `--profile`/`-p` and the `disable`, `enable`, `theme`, `dark-theme`, `icon`,
`default`, and `remove` profile arguments. `zetta profile themes` supplies
theme values for `profile theme`, `profile dark-theme`, `profile add --theme`,
`profile add --dark-theme`, and root `--theme`/`-t`. `profile icon` and
`profile add --icon` complete the fixed
values `auto`, `zetta`, `bash`, `zsh`, and `fish`.
If a `-c`/`--config` value is present in the command line, completion passes it
through to both endpoints. Endpoint output is processed one line at a time,
so names containing spaces or quotes remain single completion candidates.

Pane-split completion works dynamically too: completing the root
`zetta --split`/`-s` option runs `zetta splits` against the current
configuration, so newly added or renamed templates are available without
regenerating the shell integration.

The `pane` subcommand is completed in the same generated scripts. Its
`--direction`/`-d` value offers `left`, `right`, `up`, and `down`, while
`--pane`/`-p` runs `zetta pane --list` against the active Zetta process so
current pane labels are available without regenerating the integration. The
`--label` value remains free-form, and `--stack` and `--list` are offered as
flags. New split overlays complete their fixed size and named-color values;
use `--overlay TEXT` with `--overlay-size`, `--overlay-opacity`, and
`--overlay-color` to configure them. Commands still begin after `--`,
preserving their exact argument boundaries.

`zetta overlay`'s text (`--text`/`-t`) and opacity (`--opacity`/`-o`) flags
take free-form values. Its color flag (`--color`/`-c`) completes the fixed
named presets `black`, `white`, `gray`, `red`, `orange`, `yellow`, `green`,
`cyan`, `blue`, `purple`, `magenta`, and `pink`; hex values remain accepted
when typed manually. Its size flag (`--size`/`-s`) is a fixed, compile-time set
of names (`sm`, `base`, `lg`, `xl`, `2xl`, `3xl`), so the generated script
completes those directly. Use `zetta overlay TEXT`
(or `zetta overlay --text TEXT`) from a Zetta pane to non-persistently show
text over the active pane's terminal content, and add `--size`, `--opacity`,
or `--color` to customize its font size, transparency, or text color;
`zetta overlay --reset` clears it.

It also completes the full-length `zetta terminal-size` resize flags, which
resize the current Zetta pane while retaining an omitted dimension. Completion
lists full-length option names alongside valid subcommands without requiring a
leading dash, while short aliases remain supported, including completion of
their argument values.

The supported shell names are `bash`, `zsh`, `fish`, and `powershell` (`pwsh`
is accepted as an alternative spelling).

The PowerShell integration supports Windows PowerShell 5.1 and PowerShell 7+
(`pwsh`).

## Enable for the current shell

For Bash or Zsh:

```sh
eval "$(zetta init bash)"
eval "$(zetta init zsh)"
```

For Fish:

```fish
zetta init fish | source
```

For PowerShell:

```powershell
zetta init powershell | Out-String | Invoke-Expression
```

## Enable persistently

Run `zetta init` to detect the active shell process and add the applicable
command to its startup file. This takes precedence over an inherited `$SHELL`,
which may describe the login shell rather than a shell selected as a Zetta
profile. If process inspection cannot identify a supported shell, Zetta falls
back to `$SHELL`. It prints the file it writes, or reports that the integration
is already present without changing the file.

When run from MSYS2 or Cygwin on Windows, Zetta resolves its Unix-style `$HOME`
with `cygpath` before writing `.bashrc` or `.zshrc`, including for installations
outside their conventional roots. Cygwin profiles also install session-local
prompt and foreground-command hooks for Bash, Zsh, Fish, and Nushell; these
hooks report `zetta-cwd:` and `zetta-cmd:` markers without changing the user's
shell startup files. Native editor dispatch uses `cygpath` for Cygwin paths.

The startup files and commands are:

- Bash: `~/.bashrc`
- Zsh: `~/.zshrc`
- Fish: `~/.config/fish/config.fish`
- PowerShell: `$PROFILE`

For example, `zetta init` from Zsh adds `eval "$(zetta init zsh)"` to
`~/.zshrc`. You can also add it manually, or add
`zetta init powershell | Out-String | Invoke-Expression` to `$PROFILE`. Start a new shell
or source the file after editing it.

Profile and light/dark theme changes are visible on the next completion request; there is
no shell-integration regeneration step. A profile mutation also asks a running
Zetta process using the same configuration path to reload all open and dormant
entities. The persisted file remains authoritative if no matching process is
running, and the CLI reports that live state was not refreshed.

Run `zetta init --help` to see the accepted shell names.
