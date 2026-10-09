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
$zettaProfiles = { param($configArguments) @(& zetta profile list @configArguments 2>$null) }
$zettaProfileThemes = { param($configArguments) @(& zetta profile themes @configArguments 2>$null) }
$zettaInitCandidates = @('bash', 'fish', 'powershell', 'pwsh', 'zsh', '--completions', '--help')
$zettaOverlayColors = @(ZETTA_OVERLAY_COLORS)
$zettaTabIcons = { @(& zetta tabicon --list 2>$null) }
$zettaThemes = { param($scope) @(& zetta theme $scope --list 2>$null) }
$zettaSplits = { @(& zetta splits 2>$null) }
$zettaProjects = { @(& zetta project list 2>$null) }
$zettaProjectCommands = { @(& zetta cmd --list 2>$null) }
$zettaPaneLabels = { @(& zetta pane --list 2>$null) }
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

# ZETTA_ZMUX_INTEGRATION_BEGIN
$zmuxSessionIds = {
    try {
        $lines = if ($commandName -eq 'zmux') { @(zmux list 2>$null) } else { @(zetta mux list 2>$null) }
        foreach ($line in $lines) {
            if ($line -match '^\s*reconnect\s+id:\s+(\d+:\d+:\d+)(?:\s+\(short:\s+\d+\))?\s*$') { $matches[1] }
        }
    } catch {}
}

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

$zettaMuxAttachArguments = {
    param($wordToComplete, $words)
    $target = ''
    $port = ''
    for ($index = 3; $index -lt ($words.Count - 1); $index++) {
        $token = [string]$words[$index]
        switch -Regex ($token) {
            '^(--ssh-target|-H)$' {
                if ($index + 1 -lt ($words.Count - 1)) { $target = [string]$words[++$index] }
                continue
            }
            '^--ssh-target=' { $target = $token.Substring($token.IndexOf('=') + 1); continue }
            '^(--port|-p)$' {
                if ($index + 1 -lt ($words.Count - 1)) { $port = [string]$words[++$index] }
                continue
            }
            '^--port=' { $port = $token.Substring($token.IndexOf('=') + 1); continue }
            '^(--identity|-i)$' { $index++; continue }
            '^--identity=' { continue }
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
    $noMux = $env:ZETTA_NO_MUX -eq '1'
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
# ZETTA_ZMUX_INTEGRATION_BEGIN
    if ($commandName -eq 'zmux') {
        # `zmux` takes the same arguments as `zetta mux`, so completing it
        # reuses that logic by inserting the subcommand it stands in for.
        $wordList.Insert(1, 'mux')
        $words = $wordList.ToArray()
    }
# ZETTA_ZMUX_INTEGRATION_END
    for ($index = 1; $index -lt $words.Count; $index++) {
        if ($words[$index] -in '--command', '-e') {
            # --command owns the rest of argv, including tokens that happen to
            # look like Zetta options or subcommands.
            return @()
        }
        if ($words[$index] -eq '--' -and $words[1] -eq 'pane' -and $words.Count -gt 2 -and $words[2] -eq 'wait') {
            # `pane wait --` owns the rest of argv too.
            return @()
        }
        if ($words[$index] -eq '--' -and $words[1] -eq 'cmd') {
            # `cmd NAME --` owns the rest of argv too.
            return @()
        }
    }
    $previous = if ($words.Count -gt 1) { $words[$words.Count - 2] } else { '' }
    $last = if ($words.Count -gt 1) { $words[$words.Count - 1] } else { '' }

    $configArguments = @()
    for ($index = 1; $index -lt $words.Count; $index++) {
        if (($words[$index] -eq '--config' -or ($words[$index] -eq '-c' -and $words[1] -ne 'init')) -and $index + 1 -lt $words.Count) {
            $configArguments += '--config'
            $configArguments += $words[$index + 1]
            $index++
        }
    }
    $profileOperation = ''
    $profileOperationIndex = -1
    $profileIndex = -1
    for ($index = 1; $index -lt $words.Count; $index++) {
        if ($words[$index] -in '--config', '-c', '--keymap', '-k', '--profile', '-p', '--split', '-s', '--theme', '-t', '--geometry', '-g') {
            $index++
        } elseif ($words[$index] -eq 'profile') {
            $profileIndex = $index
            break
        }
    }
    $subcommand = $null
    foreach ($word in $words) {
        if ($word -in 'benchmark', 'terminal-size', ZETTA_MUX_ROOT_COMMAND_PS 'splits', 'pane', 'profile', 'project', 'cmd', 'edit', 'vi', 'init', 'mosh', 'serial', 'http', 'tftp', 'notify', 'attention', 'copy', 'paste', 'tabicon', 'theme', 'overlay'ZETTA_WORKTREE_ROOT_COMMANDS) { $subcommand = $word; break }
    }
    $worktreeCommand = $false
    $worktreeOperation = ''
    if ZETTA_WORKTREE_STANDALONE_CHECK {
        $worktreeCommand = $true
        if ($words.Count -gt 1) { $worktreeOperation = $words[1] }
    } elseif ZETTA_WORKTREE_ROOT_CHECK {
        $worktreeCommand = $true
        if ($words.Count -gt 2) { $worktreeOperation = $words[2] }
    }
    if ($profileIndex -ge 0) {
        $subcommand = 'profile'
    }
    if ($profileIndex -ge 0) {
        for ($index = $profileIndex + 1; $index -lt $words.Count; $index++) {
            if ($words[$index] -in '--config', '-c') {
                $index++
            } elseif ([string]::IsNullOrEmpty($words[$index])) {
                continue
            } elseif ($words[$index] -notlike '-*') {
                $profileOperation = $words[$index]
                $profileOperationIndex = $index
                break
            }
        }
    }

    $moshHostGiven = $false
    if ($commandName -eq 'zosh' -or $subcommand -eq 'mosh') {
        $moshIndex = if ($commandName -eq 'zosh') { 0 } else { [array]::IndexOf($words, 'mosh') }
        for ($index = $moshIndex + 1; $index -lt $words.Count; $index++) {
            $moshToken = $words[$index]
            if ($moshToken -eq '--') { continue }
            if ($moshToken -like '-*') { continue }
            if ($words[$index - 1] -in '--predict', '--family', '--experimental-remote-ip', '--bind-server', '--client', '--server', '--ssh', '--port', '-p') { continue }
            $moshHostGiven = $true
            break
        }
    }

    $candidates = if ($commandName -eq 'zosh') {
        if ($moshHostGiven) {
            @()
        } elseif ($previous -eq '--predict') { 'adaptive', 'always', 'never', 'experimental' }
        elseif ($previous -eq '--family') { 'prefer-inet', 'prefer-inet6', 'inet', 'inet6', 'auto', 'all' }
        elseif ($previous -eq '--experimental-remote-ip') { 'local', 'remote', 'proxy' }
        elseif ($previous -eq '--bind-server') { 'ssh', 'any' }
        elseif ($previous -in '--client', '--server') {
            @(Get-ChildItem -Name -Path "$wordToComplete*" -ErrorAction SilentlyContinue)
        } elseif ($wordToComplete -like '-*') {
            '--client', '--server', '--predict', '-a', '-n', '-o', '--predict-overwrite', '--no-predict-overwrite', '-k', '--keep-alive', '--scrollback', '--no-scrollback', '-4', '-s', '--forward-agent', '--no-forward-agent', '-6', '--family', '-p', '--port', '--bind-server', '--ssh', '--ssh-pty', '--no-ssh-pty', '--init', '--no-init', '--local', '--experimental-remote-ip', '-h', '--help', '-V', '--version', '--'
        } else {
            @(& $zettaSshTargets) + '--client', '--server', '--predict', '-a', '-n', '-o', '--predict-overwrite', '--no-predict-overwrite', '-k', '--keep-alive', '--scrollback', '--no-scrollback', '-4', '-s', '--forward-agent', '--no-forward-agent', '-6', '--family', '-p', '--port', '--bind-server', '--ssh', '--ssh-pty', '--no-ssh-pty', '--init', '--no-init', '--local', '--experimental-remote-ip', '-h', '--help', '-V', '--version', '--'
        }
    } elseif ($commandName -eq 'ztftp') {
        if ($words.Count -le 1) { 'get', 'put', '--help' } else { '--port', '--help' }
    } elseif ($commandName -eq 'zntfy') {
        if ($previous -in '--timeout', '-t') { 'default', 'never' }
        elseif ($previous -in '--sound', '-s') { $zettaSoundNames }
        else { '--app-name', '--icon', '--sound', '--timeout', '--help' }
# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
    } elseif ($commandName -in 'zcopy', 'pbcopy') {
        if ($previous -in '--pboard', '-pboard') { 'general', 'ruler', 'find', 'font' }
        else { '--pboard', '--help' }
    } elseif ($commandName -in 'zpaste', 'pbpaste') {
        if ($previous -in '--pboard', '-pboard') { 'general', 'ruler', 'find', 'font' }
        elseif ($previous -in '--prefer', '-prefer', '--Prefer', '-Prefer') { 'txt', 'rtf', 'ps' }
        else { '--pboard', '--prefer', '--help' }
# ZETTA_CLIPBOARD_INTEGRATION_END
    } elseif ($subcommand -eq 'mosh') {
        if ($moshHostGiven) {
            @()
        } elseif ($previous -eq '--predict') { 'adaptive', 'always', 'never', 'experimental' }
        elseif ($previous -in '--family', '--experimental-remote-ip') {
            if ($previous -eq '--family') { 'prefer-inet', 'prefer-inet6', 'inet', 'inet6', 'auto', 'all' }
            else { 'local', 'remote', 'proxy' }
        } elseif ($previous -eq '--bind-server') { 'ssh', 'any' }
        elseif ($previous -in '--client', '--server') {
            @(Get-ChildItem -Name -Path "$wordToComplete*" -ErrorAction SilentlyContinue)
        } elseif ($wordToComplete -like '-*') {
            '--client', '--server', '--predict', '-a', '-n', '-o', '--predict-overwrite', '--no-predict-overwrite', '-k', '--keep-alive', '--scrollback', '--no-scrollback', '-4', '-s', '--forward-agent', '--no-forward-agent', '-6', '--family', '-p', '--port', '--bind-server', '--ssh', '--ssh-pty', '--no-ssh-pty', '--init', '--no-init', '--local', '--experimental-remote-ip', '-h', '--help', '-V', '--version', '--'
        } else {
            @(& $zettaSshTargets) + '--client', '--server', '--predict', '-a', '-n', '-o', '--predict-overwrite', '--no-predict-overwrite', '-k', '--keep-alive', '--scrollback', '--no-scrollback', '-4', '-s', '--forward-agent', '--no-forward-agent', '-6', '--family', '-p', '--port', '--bind-server', '--ssh', '--ssh-pty', '--no-ssh-pty', '--init', '--no-init', '--local', '--experimental-remote-ip', '-h', '--help', '-V', '--version', '--'
        }
    } elseif ($subcommand -eq 'cmd') {
        $delimiter = $false
        for ($index = 2; $index -lt $words.Count; $index++) {
            if ($words[$index] -eq '--') {
                $delimiter = $true
                break
            }
        }
        if ($delimiter) {
            @()
        } elseif ($words.Count -ge 3 -and $words[2] -in '--list', '--help') {
            @()
        } elseif ($words.Count -le 2) {
            @(& $zettaProjectCommands) + '--help', '--list', '--'
        } elseif ($words.Count -eq 3 -and $wordToComplete -notlike '-*' -and -not [string]::IsNullOrEmpty($wordToComplete)) {
            & $zettaProjectCommands
        } elseif ($wordToComplete -like '-*') {
            '--help', '--'
        } elseif ($words.Count -eq 3 -and [string]::IsNullOrEmpty($wordToComplete)) {
            '--help', '--'
        } else {
            @()
        }
    } elseif ($previous -eq '--geometry' -or ($previous -eq '-g' -and $null -eq $subcommand)) {
        @()
    } elseif (
        $previous -eq '--split' -or $last -eq '--split' -or
        (($previous -eq '-s' -or $last -eq '-s') -and $null -eq $subcommand)
    ) {
        & $zettaSplits
    } elseif ($previous -eq '--replace-pane' -or ($previous -eq '-r' -and $null -eq $subcommand)) {
        if ($wordToComplete -like '-*' -or [string]::IsNullOrEmpty($wordToComplete)) {
            '--help', '--version', '--config', '--keymap', '--profile', '--split', '--theme', '--geometry', ZETTA_NO_MUX_OPTION_PS '--new-window', '--command'
        } else {
            @()
        }
    } elseif ($subcommand -eq 'pane' -and $words.Count -gt 2 -and $words[2] -eq 'wait' -and $words -notcontains '--' -and (
        $previous -eq 'wait' -or
        $previous -in '--allow-failure', '-a' -or
        $wordToComplete -like '*,*'
    )) {
        & $zettaRunPaneLabels $wordToComplete
    } elseif ($previous -in '--pane', '-p' -and $subcommand -eq 'pane') {
        & $zettaPaneLabels
    } elseif ($previous -in '--direction', '-d' -and $subcommand -eq 'pane') {
        'left', 'right', 'up', 'down'
    } elseif ($previous -in '--overlay-size', '-S' -and $subcommand -eq 'pane') {
        'sm', 'base', 'lg', 'xl', '2xl', '3xl'
    } elseif ($previous -in '--overlay-color', '-c' -and $subcommand -eq 'pane') {
        $zettaOverlayColors
    } elseif (
        $previous -eq '--profile' -or $last -eq '--profile' -or
        (($previous -eq '-p' -or $last -eq '-p') -and $null -eq $subcommand)
    ) {
        & $zettaProfiles $configArguments
    } elseif ($previous -in '--timeout', '-t' -and $subcommand -in 'notify', 'attention') {
        'default', 'never'
    } elseif ($previous -in '--output-type', '-t', '--theme', '--text') {
        if ($subcommand -eq 'profile' -or $null -eq $subcommand) { & $zettaProfileThemes $configArguments }
        elseif ($subcommand -eq 'theme' -and $words.Count -gt 2 -and $words[2] -in 'pane', 'tab') { & $zettaThemes $words[2] }
        elseif ($subcommand -in 'notify', 'attention') { 'default', 'never' }
        elseif ($subcommand -eq 'overlay') { @() }
        else { 'repeated', 'unique' }
    } elseif ($previous -in '--device', '-d') {
        if ($subcommand -eq 'serial') { @(& zetta serial list 2>$null) } else { @() }
    } elseif ($previous -in '--data-bits', '-D') {
        if ($subcommand -eq 'serial') { '5', '6', '7', '8' } else { @() }
    } elseif ($previous -eq '--parity' -or ($previous -eq '-p' -and $subcommand -eq 'serial')) {
        'none', 'odd', 'even'
    } elseif ($previous -in '--stop-bits', '-s', '--size') {
        if ($subcommand -eq 'serial') { '1', '2' }
        elseif ($subcommand -in 'notify', 'attention') { $zettaSoundNames }
        elseif ($subcommand -eq 'overlay') { 'sm', 'base', 'lg', 'xl', '2xl', '3xl' }
        else { @() }
    } elseif ($previous -eq '--sound') {
        $zettaSoundNames
    } elseif ($previous -in '--flow-control', '-f') {
        'none', 'software', 'hardware'
    } elseif ($previous -in '--pboard', '-pboard') {
        'general', 'ruler', 'find', 'font'
    } elseif ($previous -in '--prefer', '-prefer', '--Prefer', '-Prefer') {
        'txt', 'rtf', 'ps'
    } elseif ($previous -in '--opacity', '-o', '--overlay-opacity', '-O', '--overlay') {
        @()
    } elseif ZETTA_WORKTREE_POWERSHELL_COPY_COMPLETION_CHECK {
        @(Get-ChildItem -Name -Path "$wordToComplete*" -ErrorAction SilentlyContinue)
    } elseif ($previous -eq '-c' -and $subcommand -eq 'init') {
        $zettaInitCandidates
    } elseif ($previous -in '--color', '-c') {
        if ($subcommand -eq 'overlay') { $zettaOverlayColors } else { @() }
    } elseif ($commandName -in 'vi', 'zvi' -or $subcommand -in 'edit', 'vi') {
        if ($wordToComplete -like '-*') {
            '--help'
        } else {
            @(Get-ChildItem -Name -Path "$wordToComplete*" -ErrorAction SilentlyContinue)
        }
    } elseif (
        $previous -in '--columns', '--rows', '-R' -or
        ($previous -eq '-c' -and ($subcommand -eq 'terminal-size' -or $subcommand -eq 'overlay'))
    ) {
        @()
    } elseif ($subcommand -eq 'profile' -and $profileOperation -in 'add', 'icon' -and $previous -in '--icon', '-i') {
        'auto', 'zetta', 'bash', 'zsh', 'fish'
    } elseif ($subcommand -eq 'profile' -and $profileOperation -eq 'add' -and $previous -in '--theme', '-t', '--dark-theme', '-d') {
        & $zettaProfileThemes $configArguments
    } elseif ($subcommand -eq 'tabicon' -and ($words -contains '--reset' -or $words -contains '-r')) {
        @()
    } elseif ($subcommand -eq 'tabicon' -and (
        $previous -in '--icon', '-i' -or $wordToComplete -notlike '-*'
    )) {
        & $zettaTabIcons
    } elseif ($subcommand -eq 'theme' -and $words.Count -gt 2 -and $words[2] -in 'pane', 'tab' -and $wordToComplete -notlike '-*') {
        & $zettaThemes $words[2]
    } elseif ($subcommand -eq 'profile' -and $profileOperation -in 'theme', 'dark-theme' -and $wordToComplete -notlike '-*' -and $words -notcontains '--reset' -and $words -notcontains '-r' -and -not ($profileOperationIndex -eq ($words.Count - 1) -and -not [string]::IsNullOrEmpty($wordToComplete))) {
        $profileArguments = @(for ($i = $profileIndex + 2; $i -lt $words.Count; $i++) {
            if ($words[$i] -notlike '-*' -and -not [string]::IsNullOrEmpty($words[$i])) { $words[$i] }
        })
        if ($profileArguments.Count -ge 2 -or ($profileArguments.Count -eq 1 -and [string]::IsNullOrEmpty($wordToComplete))) { & $zettaProfileThemes $configArguments }
        else { & $zettaProfiles $configArguments }
# ZETTA_ZMUX_INTEGRATION_BEGIN
    } elseif ($subcommand -eq 'mux' -and $words.Count -ge 3 -and $words[2] -eq 'attach' -and $wordToComplete -notlike '-*') {
        if ($previous -in '--ssh-target', '-H') {
            & $zettaSshTargets
        } elseif ($previous -in '--port', '-p') {
            @()
        } elseif ($previous -in '--identity', '-i') {
            @(Get-ChildItem -Name -Path "$wordToComplete*" -ErrorAction SilentlyContinue)
        } else {
            & $zettaMuxAttachArguments $wordToComplete $words
        }
    } elseif ($subcommand -eq 'mux' -and $words.Count -ge 3 -and (
        $words[2] -eq 'reconnect' -or
        $words[2] -eq 'resume' -or
        (-not $noMux -and $words[2] -in 'share', 'unshare', 'kill', 'forget')
    ) -and $wordToComplete -notlike '-*') {
        if ($words[2] -eq 'resume') { & $zmuxRestorableIds } else { & $zmuxSessionIds }
# ZETTA_ZMUX_INTEGRATION_END
# ZETTA_WORKTREE_INTEGRATION_BEGIN
    } elseif ZETTA_WORKTREE_COMPLETION_CHECK {
        if ([string]::IsNullOrEmpty($worktreeOperation)) {
            'new', 'done', 'abort', 'status', 'sync', 'config', '--help'
        } elseif ($worktreeOperation -eq 'new') {
            '--copy', '--path-only', '--help'
        } elseif ($worktreeOperation -eq 'done' -or $worktreeOperation -eq 'abort') {
            '--path-only', '--help'
        } elseif ($worktreeOperation -eq 'sync') {
            $operationIndex = if ($commandName -eq 'zwt') { 1 } else { 2 }
            $targetEnd = $words.Count
            if ($targetEnd -gt ($operationIndex + 1) -and $words[$targetEnd - 1] -eq $wordToComplete) {
                $targetEnd--
            }
            $targetCount = 0
            for ($index = $operationIndex + 1; $index -lt $targetEnd; $index++) {
                if ($words[$index] -notlike '-*' -and -not [string]::IsNullOrEmpty($words[$index])) {
                    $targetCount++
                }
            }
            if ($targetCount -eq 0 -and $wordToComplete -notlike '-*') {
                & $zettaWorktreeCommits
            } elseif ($wordToComplete -like '-*') {
                '--help'
            } else {
                @()
            }
        } else {
            '--help'
        }
# ZETTA_WORKTREE_INTEGRATION_END
    } elseif ($null -eq $subcommand) {
        'benchmark', 'terminal-size', ZETTA_MUX_ROOT_COMMAND_PS 'profile', 'project', 'cmd', 'splits', 'pane', 'edit', 'vi', 'init', 'mosh', 'serial', 'http', 'tftp', 'notify', 'attention', 'copy', 'paste', 'tabicon', 'theme', 'overlay'ZETTA_WORKTREE_ROOT_COMMANDS, '--help', '--version', '--config', '--keymap', '--profile', '--split', '--replace-pane', '--theme', '--geometry', ZETTA_NO_MUX_OPTION_PS '--new-window', '--command'
    } else {
        switch ($subcommand) {
            'benchmark' {
                if ($words.Count -gt 2 -and $words[2] -eq 'output') {
                    '--size', '--output-type', '--help'
                } else {
                    'output', '--profile-report', '--profile-duration', '--profile-pane-stress', '--profile-background-stress', '--profile-sparse-updates', '--profile-alt-screen-scroll', '--profile-external-terminal', '--help'
                }
            }
            'terminal-size' { '--json', '--resize', '--columns', '--rows', '--help' }
            'edit' { '--delete-after', '--help' }
            'vi' { '--help' }
        'mosh' { '--client', '--server', '--predict', '-a', '-n', '-o', '--predict-overwrite', '--no-predict-overwrite', '-k', '--keep-alive', '--scrollback', '--no-scrollback', '-4', '-s', '--forward-agent', '--no-forward-agent', '-6', '--family', '-p', '--port', '--bind-server', '--ssh', '--ssh-pty', '--no-ssh-pty', '--init', '--no-init', '--local', '--experimental-remote-ip', '-h', '--help', '-V', '--version', '--' }
# ZETTA_ZMUX_INTEGRATION_BEGIN
            'mux' {
                if ($words.Count -le 2) {
                    if ($noMux) { 'list', 'reconnect', '--json', '--help', '--version' }
                    else { 'list', 'profiles', 'create', 'stop', 'reconnect', 'attach', 'resume', 'share', 'unshare', 'kill', 'forget', '--json', '--ids-only', '--ssh-target', '--port', '--upgrade', '--identity', '--secret-stdin', '--layout', '--profile', '--title', '--working-directory', '--env', '--retention', '--help', '--version' }
                }
                elseif ($noMux -and $words[2] -notin 'list', 'reconnect', 'attach') { @() }
                elseif ($words[2] -eq 'stop') { '--force', '--help' }
                elseif ($words[2] -eq 'profiles') { '--json', '--ssh-target', '--port', '--help', '--version' }
                elseif ($words[2] -eq 'create') { '--json', '--secret-stdin', '--layout', '--profile', '--title', '--working-directory', '--env', '--retention', '--ssh-target', '--port', '--help', '--version' }
                elseif ($words[2] -eq 'attach') { '--ssh-target', '--port', '--protocol', '--keep-alive', '--identity', '--help' }
                elseif ($words[2] -in 'resume', 'reconnect') { '--identity', '--help' }
                else { '--json', '--ids-only', '--ssh-target', '--port', '--help' }
            }
# ZETTA_ZMUX_INTEGRATION_END
            'splits' { '--help' }
            'pane' {
                if ($words.Count -gt 2 -and $words[2] -eq 'wait') {
                    '--allow-failure', '--help', '--'
                } else {
                    'wait', '--direction', '--label', '--pane', '--overlay', '--overlay-size', '--overlay-opacity', '--overlay-color', '--stack', '--list', '--help'
                }
            }
            'profile' {
                if ([string]::IsNullOrEmpty($profileOperation) -or ($profileOperationIndex -eq ($words.Count - 1) -and -not [string]::IsNullOrEmpty($wordToComplete)) -or $profileOperation -notin 'list', 'themes', 'disable', 'enable', 'theme', 'dark-theme', 'icon', 'default', 'add', 'remove') {
                    'list', 'themes', 'disable', 'enable', 'theme', 'dark-theme', 'icon', 'default', 'add', 'remove', '--config', '--help'
                } elseif ($profileOperation -in 'disable', 'enable', 'default', 'remove') {
                    if ($previous -eq $profileOperation -or $last -eq $profileOperation) { & $zettaProfiles $configArguments } else { '--config', '--help' }
                } elseif ($profileOperation -in 'theme', 'dark-theme') {
                    if ($wordToComplete -like '-*') { '--reset', '--config', '--help' }
                    elseif ($previous -eq $profileOperation -or $last -eq $profileOperation) { & $zettaProfiles $configArguments }
                    elseif ($previous -in '--reset', '-r' -or $last -in '--reset', '-r') { '--config', '--help' }
                    else { & $zettaProfileThemes $configArguments }
                } elseif ($profileOperation -eq 'icon') {
                    if ($wordToComplete -like '-*') { '--reset', '--config', '--help' }
                    elseif ($previous -eq 'icon' -or $last -eq 'icon') { & $zettaProfiles $configArguments }
                    elseif ($previous -in '--reset', '-r' -or $last -in '--reset', '-r') { '--config', '--help' }
                    else { 'auto', 'zetta', 'bash', 'zsh', 'fish' }
                } elseif ($profileOperation -eq 'add') {
                    '--program', '--arg', '--theme', '--dark-theme', '--icon', '--config', '--help'
                } else { '--config', '--help' }
            }
            'project' {
                $operation = if ($words.Count -gt 2) { $words[2] } else { '' }
                if ([string]::IsNullOrEmpty($operation) -or $operation -notin 'add', 'list', 'remove', 'open') {
                    'add', 'list', 'remove', 'open', '--help'
                } elseif ($operation -eq 'add' -and $wordToComplete -notlike '-*') {
                    @(Get-ChildItem -Directory -Name -Path "$wordToComplete*" -ErrorAction SilentlyContinue)
                } elseif ($operation -in 'open', 'remove' -and $wordToComplete -notlike '-*') {
                    & $zettaProjects
                } elseif ($operation -in 'add', 'open', 'remove') {
                    '--path', '--help'
                } else {
                    '--help'
                }
            }
            'cmd' {
                $delimiter = $false
                for ($index = 2; $index -lt $words.Count; $index++) {
                    if ($words[$index] -eq '--') {
                        $delimiter = $true
                        break
                    }
                }
                if ($delimiter) {
                    @()
                } elseif ($words.Count -ge 3 -and $words[2] -in '--list', '--help') {
                    @()
                } elseif ($words.Count -le 2) {
                    @(& $zettaProjectCommands) + '--help', '--list', '--'
                } elseif ($words.Count -eq 3 -and $wordToComplete -notlike '-*' -and -not [string]::IsNullOrEmpty($wordToComplete)) {
                    & $zettaProjectCommands
                } elseif ($wordToComplete -like '-*') {
                    '--help', '--'
                } elseif ($words.Count -eq 3 -and [string]::IsNullOrEmpty($wordToComplete)) {
                    '--help', '--'
                } else {
                    @()
                }
            }
            'init' { $zettaInitCandidates }
            'serial' {
                if ($words.Count -le 2) { 'console', 'list', '--help' }
                elseif ($words[2] -eq 'console') { '--device', '--baud-rate', '--data-bits', '--parity', '--stop-bits', '--flow-control', '--help' }
            }
            'http' {
                if ($words.Count -le 2) { 'server', '--help' } else { '--root', '--port', '--config', '--help' }
            }
            'tftp' {
                if ($words.Count -le 2) { 'get', 'put', 'server', '--help' }
                elseif ($words[2] -eq 'server') { '--root', '--port', '--config', '--writable', '--help' }
                else { '--port', '--help' }
            }
            'notify' {
                if ($words.Count -gt 2 -and $words[2] -eq 'cleanup') {
                    '--dry-run', '--help'
                } else {
                    'cleanup', '--app-name', '--icon', '--sound', '--timeout', '--help'
                }
            }
            'attention' { '--notify', '--app-name', '--icon', '--sound', '--timeout', '--help' }
            'copy' { '--pboard', '--help' }
            'paste' { '--pboard', '--prefer', '--help' }
            'tabicon' { '--icon', '--reset', '--queue', '--list', '--help' }
            'theme' {
                if ($words.Count -le 2) {
                    'pane', 'tab', '--help'
                } elseif ($words[2] -in 'pane', 'tab') {
                    '--theme', '--reset', '--list', '--help'
                }
            }
            'overlay' { '--text', '--size', '--opacity', '--color', '--reset', '--help' }
# ZETTA_WORKTREE_INTEGRATION_BEGIN
            ZETTA_WORKTREE_SWITCH_CASE {
                if ([string]::IsNullOrEmpty($worktreeOperation)) {
                    'new', 'done', 'abort', 'status', 'sync', 'config', '--help'
                } elseif ($worktreeOperation -eq 'new') {
                    '--copy', '--path-only', '--help'
                } elseif ($worktreeOperation -eq 'done' -or $worktreeOperation -eq 'abort') {
                    '--path-only', '--help'
                } elseif ($worktreeOperation -eq 'sync') {
                    $operationIndex = if ($commandName -eq 'zwt') { 1 } else { 2 }
                    $targetEnd = $words.Count
                    if ($targetEnd -gt ($operationIndex + 1) -and $words[$targetEnd - 1] -eq $wordToComplete) {
                        $targetEnd--
                    }
                    $targetCount = 0
                    for ($index = $operationIndex + 1; $index -lt $targetEnd; $index++) {
                        if ($words[$index] -notlike '-*' -and -not [string]::IsNullOrEmpty($words[$index])) {
                            $targetCount++
                        }
                    }
                    if ($targetCount -eq 0 -and $wordToComplete -notlike '-*') {
                        & $zettaWorktreeCommits
                    } elseif ($wordToComplete -like '-*') {
                        '--help'
                    } else {
                        @()
                    }
                } else {
                    '--help'
                }
            }
# ZETTA_WORKTREE_INTEGRATION_END
        }
    }

    foreach ($candidate in $candidates) {
        if ($candidate -like '-*' -and -not (ZETTA_WORKTREE_POWERSHELL_REPEATABLE_COPY) -and $candidate -in $words) { continue }
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
Register-ArgumentCompleter -Native -CommandName zetta -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -Native -CommandName zosh -ScriptBlock $zettaCompletions
# ZETTA_ZMUX_INTEGRATION_BEGIN
Register-ArgumentCompleter -Native -CommandName zmux -ScriptBlock $zettaCompletions
# ZETTA_ZMUX_INTEGRATION_END
Register-ArgumentCompleter -CommandName ztftp -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zntfy -ScriptBlock $zettaCompletions
# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
Register-ArgumentCompleter -CommandName zcopy -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zpaste -ScriptBlock $zettaCompletions
# ZETTA_CLIPBOARD_INTEGRATION_END
Register-ArgumentCompleter -CommandName zvi -ScriptBlock $zettaCompletions
# ZETTA_WORKTREE_INTEGRATION_BEGIN
Register-ArgumentCompleter -CommandName zwt -ScriptBlock $zettaCompletions
# ZETTA_WORKTREE_INTEGRATION_END
if ($zettaViMissing) {
    Register-ArgumentCompleter -CommandName vi -ScriptBlock $zettaCompletions
}
if ($zettaMoshMissing) {
    Register-ArgumentCompleter -CommandName mosh -ScriptBlock $zettaCompletions
}
# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
if (-not $IsMacOS) {
    Register-ArgumentCompleter -CommandName pbcopy -ScriptBlock $zettaCompletions
    Register-ArgumentCompleter -CommandName pbpaste -ScriptBlock $zettaCompletions
}
# ZETTA_CLIPBOARD_INTEGRATION_END
