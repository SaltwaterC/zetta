if (-not (Get-Variable __ZettaCompletionState -Scope Global -ErrorAction SilentlyContinue)) {
    $global:__ZettaCompletionState = @{ Handler = $null; Loading = $false }
}
# ZETTA_WORKTREE_INTEGRATION_BEGIN
$zettaWorktreeCommits = {
    try {
        $currentBranch = [string](& git branch --show-current 2>$null | Select-Object -First 1)
        $currentBranch = $currentBranch.Trim()
        if ([string]::IsNullOrEmpty($currentBranch)) { return }
        $sourceBranch = [string](& git config --local --get "wtbranch.$currentBranch.base" 2>$null | Select-Object -First 1)
        $sourceBranch = $sourceBranch.Trim()
        if ([string]::IsNullOrEmpty($sourceBranch)) { return }
        $splitPoint = [string](& git merge-base "refs/heads/$currentBranch" "refs/heads/$sourceBranch" 2>$null | Select-Object -First 1)
        $splitPoint = $splitPoint.Trim()
        if ([string]::IsNullOrEmpty($splitPoint)) { return }
        @(& git rev-list --reverse "$splitPoint..refs/heads/$sourceBranch" 2>$null)
    } catch {}
}
# ZETTA_WORKTREE_INTEGRATION_END
$zettaRunPaneLabels = {
    param($wordToComplete)
    $wordToComplete = [string]$wordToComplete
    $prefix = ''
    $partial = $wordToComplete
    $selected = @()
    $comma = $wordToComplete.LastIndexOf(',')
    if ($comma -ge 0) {
        $prefix = $wordToComplete.Substring(0, $comma + 1)
        $partial = $wordToComplete.Substring($comma + 1)
        if ($comma -gt 0) {
            $selected = @($wordToComplete.Substring(0, $comma).Split(','))
        }
    }
    foreach ($label in @(& zetta pane --list 2>$null)) {
        $label = [string]$label
        if ($label.StartsWith($partial, [StringComparison]::Ordinal) -and $selected -notcontains $label) {
            "$prefix$label"
        }
    }
}

# zetta-default/zetta-ok/zetta-alarm/zetta-gong are bundled tones Zetta plays itself, so
# they always work; the rest are the current platform's own system sound
# names, which only work on that platform, so only that platform's names are
# offered. $IsMacOS/$IsLinux are unset on Windows PowerShell 5.1, which only
# runs on Windows, so the Windows branch is also the correct fallback there.
$zettaSoundNames = @('zetta-default', 'zetta-ok', 'zetta-alarm', 'zetta-gong') + $(
    if ($IsMacOS) {
        'Basso', 'Blow', 'Bottle', 'Frog', 'Funk', 'Glass', 'Hero', 'Morse', 'Ping', 'Pop', 'Purr', 'Sosumi', 'Submarine', 'Tink'
    } elseif ($IsLinux) {
        'bell', 'complete', 'message', 'message-new-instant', 'dialog-information', 'dialog-warning', 'dialog-error', 'trash-empty'
    } else {
        'Default', 'IM', 'Mail', 'Reminder', 'SMS'
    }
)

$zettaSshTargets = {
    $config = Join-Path $HOME '.ssh/config'
    if (-not (Test-Path -LiteralPath $config -PathType Leaf)) { return }
    try {
        foreach ($line in Get-Content -LiteralPath $config -ErrorAction Stop) {
            if ($line -match '^\s*[Hh][Oo][Ss][Tt]\s+(.+)$') {
                foreach ($host in ($Matches[1] -split '\s+')) {
                    if ($host -and $host -notmatch '^[!]' -and $host -notmatch '[*?]' -and
                        $host.StartsWith([string]$wordToComplete, [StringComparison]::Ordinal)) {
                        $host
                    }
                }
            }
        }
    } catch {}
}

# ZETTA_ZMUX_INTEGRATION_BEGIN
$zmuxSessionIds = {
    try {
        $lines = if ($commandName -eq 'zmux') { @(zmux list 2>$null) } else { @(zetta mux list 2>$null) }
        foreach ($line in $lines) {
            if ($line -match '^\s*reconnect\s+id:\s+(\d+:\d+:\d+)(?:\s+\(short:\s+\d+\))?\s*$') { $matches[1] }
        }
    } catch {}
}

$zmuxRemoteSessionIds = {
    param($target, $port, $prefix)
    $sshArguments = @('-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=3')
    if ($null -ne $port -and [string]$port -ne '') {
        $sshArguments += @('-p', [string]$port)
    }
    try {
        foreach ($line in @(& ssh @sshArguments $target zmux list --ids-only 2>$null)) {
            $id = [string]$line
            if ($id -match '^\d+$' -and $id.StartsWith([string]$prefix, [StringComparison]::Ordinal)) {
                $id
            }
        }
    } catch {}
}

# `mux attach`'s operands: an SSH host until one is given, by operand or
# --ssh-target, then that host's session IDs. The tree engine calls it with
# $words and $treeEnd, where the words before the one being completed end, in
# scope.
$zettaMuxAttach = {
    param($wordToComplete)
    $target = ''
    $port = ''
    $index = [array]::IndexOf($words, 'attach')
    for ($index++; $index -lt $treeEnd; $index++) {
        $token = [string]$words[$index]
        switch -Regex ($token) {
            '^(--ssh-target|-H)$' {
                if (++$index -lt $treeEnd) { $target = [string]$words[$index] }
                continue
            }
            '^--ssh-target=' { $target = $token.Substring($token.IndexOf('=') + 1); continue }
            '^(--port|-p)$' {
                if (++$index -lt $treeEnd) { $port = [string]$words[$index] }
                continue
            }
            '^--port=' { $port = $token.Substring($token.IndexOf('=') + 1); continue }
            '^(--identity|-i|--protocol|-P)$' { $index++; continue }
            '^-' { continue }
            default { if ([string]::IsNullOrEmpty($target)) { $target = $token } }
        }
    }
    if ([string]::IsNullOrEmpty($target)) {
        & $zettaSshTargets
    } else {
        & $zmuxRemoteSessionIds $target $port $wordToComplete
    }
}

$zmuxRestorableIds = {
    try {
        $lines = if ($commandName -eq 'zmux') { @(zmux list 2>$null) } else { @(zetta mux list 2>$null) }
        foreach ($line in $lines) {
            if ($line -match '^\s*resume\s+id:\s+(\d+)\s*$') { $matches[1] }
        }
    } catch {}
}
# ZETTA_ZMUX_INTEGRATION_END

# ZETTA_COMMAND_TREE_NODES
# `vi` and `mosh` are `zvi` and `zosh` where the startup script wraps them.
$zettaTreeRoots['vi'] = 'zvi'
# ZETTA_ZOSH_INTEGRATION_BEGIN
$zettaTreeRoots['mosh'] = 'zosh'
# ZETTA_ZOSH_INTEGRATION_END
$zettaCompletions = {
    param($wordToComplete, $commandAst, $cursorPosition)

    # Native completion passes quote delimiters too. Use the parsed value when
    # the cursor ends a quoted token, preserving escaped and empty arguments.
    $currentElement = $commandAst.CommandElements[-1]
    if ($currentElement -is [System.Management.Automation.Language.StringConstantExpressionAst] -and
        $currentElement.StringConstantType -ne 'BareWord' -and
        $cursorPosition -eq $currentElement.Extent.EndOffset) {
        $wordToComplete = $currentElement.Value
    }

    $commandName = $commandAst.CommandElements[0].Value
    $wordList = [Collections.Generic.List[object]]::new()
    foreach ($element in $commandAst.CommandElements) {
        # Single-dash options are CommandParameterAst nodes, without a Value.
        if ($element -is [System.Management.Automation.Language.CommandParameterAst]) {
            $wordList.Add($element.Extent.Text)
        } elseif ($null -ne $element.Value) {
            $wordList.Add($element.Value)
        }
    }
    $words = $wordList.ToArray()
    $treeCandidates = @()
    # ZETTA_COMMAND_TREE_ENGINE
    foreach ($candidate in $treeCandidates) {
        if ($candidate -notlike "$wordToComplete*") { continue }
        $value = $candidate
        $text = if ($value -match '\s' -or $value.Contains("'") -or $value.Contains('"')) {
            "'" + $value.Replace("'", "''") + "'"
        } else {
            $value
        }
        [System.Management.Automation.CompletionResult]::new($text, $value, 'ParameterValue', $value)
    }
}.GetNewClosure()
$global:__ZettaCompletionState.Handler = $zettaCompletions
if ($zettaViMissing) {
    Register-ArgumentCompleter -CommandName vi -ScriptBlock $zettaCompletions
}
# ZETTA_ZOSH_INTEGRATION_BEGIN
if ($zettaMoshMissing) {
    Register-ArgumentCompleter -CommandName mosh -ScriptBlock $zettaCompletions
}
# ZETTA_ZOSH_INTEGRATION_END
# ZETTA_COMMAND_TREE_REGISTRATIONS
