    # Completes a command from its generated tree, in $zettaTreeNodes: each
    # node lists its subcommands, those that may follow its options (Late),
    # long options, repeatable options, options valid only on their own
    # (Alone), operand kinds, whether the first operand ends the options
    # (OptionsFirst) and what follows `--` (Rest), and its Option block names
    # the long option a word spells and the kind of value that option takes.
    # See command_tree.rs.
    # The last word is the one being completed when the cursor ends it, even
    # an empty one, as a quoted "" is.
    $treeEnd = $words.Count
    if ($treeEnd -gt 1 -and $cursorPosition -le $commandAst.CommandElements[-1].Extent.EndOffset -and
        $words[$treeEnd - 1] -eq $wordToComplete) {
        $treeEnd--
    }
    $treeNode = $zettaTreeRoots[$commandName]
    if ($null -ne $treeNode) {
        # A node with a Daemon block offers its contents too, unless
        # ZETTA_NO_MUX says there is no local multiplexer daemon.
        $treeSpec = {
            param($id)
            $node = $zettaTreeNodes[$id]
            if ($null -eq $node.Daemon -or $env:ZETTA_NO_MUX -eq '1') { return $node }
            $merged = $node.Clone()
            $merged.Subcommands = @($node.Subcommands) + @($node.Daemon.Subcommands)
            $merged.Options = @($node.Options) + @($node.Daemon.Options)
            $merged
        }
        # For a word giving an option its value attached: the option, the
        # value's kind, what the word starts with before the value, and the
        # value. Each `PREFIX|--long|KIND` of Attached is a prefix that takes
        # a value after it, an empty one too after an `=`.
        $treeAttached = {
            param($spec, $word)
            foreach ($entry in @($spec.Attached)) {
                if ($null -eq $entry) { continue }
                $prefix, $long, $kind = $entry.Split('|', 3)
                if ($word.StartsWith($prefix, [StringComparison]::Ordinal) -and
                    ($prefix.EndsWith('=') -or $word.Length -gt $prefix.Length)) {
                    return , @($long, $kind, $prefix, $word.Substring($prefix.Length))
                }
            }
        }
        # The subcommands a word may name once $treeGiven words are given.
        $treeOffered = {
            param($spec)
            if ($treeGiven -eq 0) { $spec.Subcommands } else { $spec.Late }
        }
        $treePending = $null
        $treeOperands = 0
        $treeGiven = 0
        $treeEnded = $false
        $treeUsed = @()
        $treeAlone = $false
        $treeAfter = ''
        $treeInstead = $false
        # The configuration file given, which the runtime lists are read with.
        $treeConfig = @()
        for ($index = 1; $index -lt $treeEnd; $index++) {
            $word = [string]$words[$index]
            $spec = & $treeSpec $treeNode
            if ($null -ne $treePending) {
                if ($treePending -eq 'config') { $treeConfig = @('--config', $word) }
                $treePending = $null
            } elseif (-not $treeEnded -and $word -eq '--') {
                $treeEnded = $true
            } elseif (-not $treeEnded -and $word -match '^-.') {
                $answer = & $spec.Option $word
                # Not a spelling: perhaps an option with its value attached.
                $attachedValue = if ($answer[1] -eq 'nothing') { & $treeAttached $spec $word }
                if ($attachedValue) {
                    $answer = @($attachedValue[0], 'nothing')
                    if ($attachedValue[1] -eq 'config') { $treeConfig = @('--config', $attachedValue[3]) }
                }
                $treeUsed += $answer[0]
                $treeGiven++
                if ($spec.Alone -ccontains $answer[0]) { $treeAlone = $true }
                # An option taking a command line takes the rest of the words.
                if ($answer[1] -eq 'command') {
                    $treeEnded = $true
                    $treeAfter = 'command'
                } elseif ($answer[1] -eq 'instead') {
                    $treeInstead = $true
                } elseif ($answer[1] -eq 'exits') {
                    $treeAlone = $true
                } elseif ($answer[1] -ne 'nothing') {
                    $treePending = $answer[1]
                }
            } elseif (-not $treeEnded -and $treeOperands -eq 0 -and (& $treeOffered $spec) -ccontains $word) {
                # A subcommand is only ever the first word after its parent,
                # or one of the parent's options.
                $treeNode = $treeNode + '__' + $word.Replace('-', '_')
                $treeGiven = 0
            } else {
                $treeOperands++
                $treeGiven++
                if ($spec.OptionsFirst) { $treeEnded = $true }
            }
        }
        $spec = & $treeSpec $treeNode
        $treeOptions = {
            $spec.Options | Where-Object {
                ($treeUsed -cnotcontains $_ -or $spec.Repeatable -ccontains $_) -and
                ($treeGiven -eq 0 -or $spec.Alone -cnotcontains $_)
            }
        }
        $treeValues = {
            param($kind)
            if ($kind -in 'files', 'config', 'directories') {
                # Paths as typed: the directory part kept, wildcard characters
                # in it taken literally, and a directory ending in a separator.
                $directory = $wordToComplete.Substring(0, $wordToComplete.LastIndexOfAny([char[]]'/\') + 1)
                $pattern = [Management.Automation.WildcardPattern]::Escape($wordToComplete) + '*'
                foreach ($item in @(Get-ChildItem -Path $pattern -ErrorAction SilentlyContinue)) {
                    if ($item.PSIsContainer) {
                        $directory + $item.Name + [IO.Path]::DirectorySeparatorChar
                    } elseif ($kind -ne 'directories') {
                        $directory + $item.Name
                    }
                }
            } elseif ($kind.StartsWith('words:')) {
                $kind.Substring(6) -split ' '
            } elseif ($kind.StartsWith('zetta:')) {
                $zettaArguments = $kind.Substring(6) -split ' '
                @(& zetta @zettaArguments 2>$null)
            } elseif ($kind.StartsWith('configured:')) {
                $zettaArguments = @($kind.Substring(11) -split ' ') + $treeConfig
                @(& zetta @zettaArguments 2>$null)
            } elseif ($kind.StartsWith('call:')) {
                $helper = Get-Variable -Name $kind.Substring(5) -ValueOnly -ErrorAction SilentlyContinue
                if ($helper -is [scriptblock]) { & $helper $wordToComplete } else { $helper }
            }
        }
        # Nothing follows an option valid only on its own.
        $treeCandidates = if ($treeAlone) {
            @()
        } elseif ($null -ne $treePending) {
            & $treeValues $treePending
        } elseif (-not $treeEnded -and $wordToComplete -like '-*' -and
            ($attachedValue = & $treeAttached $spec $wordToComplete)) {
            # `--option=VALUE` being typed: its values, after the `--option=`.
            $wordToComplete = $attachedValue[3]
            foreach ($value in @(& $treeValues $attachedValue[1])) { $attachedValue[2] + $value }
            $wordToComplete = $attachedValue[2] + $wordToComplete
        } elseif (-not $treeEnded -and $wordToComplete -like '-*') {
            & $treeOptions
        } else {
            if (-not $treeEnded -and $treeOperands -eq 0) { & $treeOffered $spec }
            # The kind of the operand being typed; the last kind listed repeats.
            # A command line after `--` (kind `command`) offers nothing, so
            # PowerShell falls back to completing paths.
            $operandKind = $spec.Operands[[Math]::Min($treeOperands, $spec.Operands.Count - 1)]
            # A switch given in place of the last operand leaves one fewer to give.
            if ($treeInstead -and $treeOperands -ge $spec.Operands.Count - 2) { $operandKind = 'nothing' }
            if (-not $treeAfter) { $treeAfter = $spec.Rest }
            if ($treeEnded -and $treeAfter) { $operandKind = $treeAfter }
            & $treeValues $operandKind
            if (-not $treeEnded -and $operandKind -in 'nothing', 'any') { & $treeOptions }
        }
    }
