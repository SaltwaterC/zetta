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

It also provides `zwt`, a wrapper for the Git worktree workflow. `zwt new NAME`
captures `zetta wt new --path-only NAME` and changes into the created worktree;
`zwt done` similarly changes into the integrated source worktree. Other `zwt`
operations pass through to `zetta wt`. Arguments are forwarded as literal shell
arguments, so nested names and paths containing spaces are supported. The
completion scripts offer `wt`, `new`, `done`, `status`, `rerere`, and the long
`--path-only` flag (the short `-P` remains accepted by the CLI).

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

Pane-theme completion works the same way: completing `zetta panetheme` runs
`zetta panetheme --list` at completion time against the running Zetta
process, so the generated script does not embed a theme list and always
offers whatever that process has registered, including user-installed
themes. Use `zetta panetheme THEME` (or `zetta panetheme --theme THEME`) from
a Zetta pane to non-persistently change the active pane's theme;
`zetta panetheme --reset` restores the profile's configured theme. `--theme`
(or `-t`) also completes profile themes when typed after `--profile` at launch,
since it non-persistently overrides that profile's theme for the new window.

Profile administration uses the non-GUI endpoint too. `zetta profile list`
supplies root `--profile`/`-p` and the `disable`, `enable`, `theme`, `default`,
and `remove` profile arguments. `zetta profile themes` supplies profile theme
values for `profile theme`, `profile add --theme`, and root `--theme`/`-t`.
If a `-c`/`--config` value is present in the command line, completion passes it
through to both endpoints. Endpoint output is processed one line at a time,
so names containing spaces or quotes remain single completion candidates.

Pane-split completion works dynamically too: completing the root
`zetta --split`/`-s` option runs `zetta splits` against the current
configuration, so newly added or renamed templates are available without
regenerating the shell integration.

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

When run from MSYS2 on Windows, Zetta resolves its Unix-style `$HOME` with
`cygpath` before writing `.bashrc` or `.zshrc`, including for MSYS2 installed
outside `C:\msys64`.

The startup files and commands are:

- Bash: `~/.bashrc`
- Zsh: `~/.zshrc`
- Fish: `~/.config/fish/config.fish`
- PowerShell: `$PROFILE`

For example, `zetta init` from Zsh adds `eval "$(zetta init zsh)"` to
`~/.zshrc`. You can also add it manually, or add
`zetta init powershell | Out-String | Invoke-Expression` to `$PROFILE`. Start a new shell
or source the file after editing it.

Profile and theme changes are visible on the next completion request; there is
no shell-integration regeneration step. A profile mutation also asks a running
Zetta process using the same configuration path to reload all open and dormant
entities. The persisted file remains authoritative if no matching process is
running, and the CLI reports that live state was not refreshed.

Run `zetta init --help` to see the accepted shell names.
