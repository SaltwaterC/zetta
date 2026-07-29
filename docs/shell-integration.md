# Shell integration

Zetta can emit a shell-specific integration script with completion for its
subcommands, flags, and flag values. The generated script includes the profile
names from Zetta's current configuration, so `zetta --profile <Tab>` completes
them as well. The script also provides `ztftp`, a shortcut for the built-in
TFTP client; it has the same TFTP completion as `zetta tftp`.

It also completes the full-length `zetta terminal-size` resize flags, which
resize the current Zetta pane while retaining an omitted dimension.

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
zetta init powershell | Invoke-Expression
```

## Enable persistently

Run `zetta init` to detect the current shell and add the applicable command to
its startup file. It prints the file it writes, or reports that the integration
is already present without changing the file.

The startup files and commands are:

- Bash: `~/.bashrc`
- Zsh: `~/.zshrc`
- Fish: `~/.config/fish/config.fish`
- PowerShell: `$PROFILE`

For example, `zetta init` from Zsh adds `eval "$(zetta init zsh)"` to
`~/.zshrc`. You can also add it manually, or add
`zetta init powershell | Invoke-Expression` to `$PROFILE`. Start a new shell
or source the file after editing it.

Profile names are captured when the integration script is generated. After
changing your Zetta profiles, start a new shell or run the applicable command
again to refresh its completions.

Run `zetta init --help` to see the accepted shell names.
