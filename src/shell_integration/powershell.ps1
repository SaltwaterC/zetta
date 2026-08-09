# Zetta shell integration for PowerShell.
if (-not (Test-Path Env:EDITOR)) {
    $env:EDITOR = 'zetta vi'
}

$zettaViMissing = -not (Get-Command vi -ErrorAction SilentlyContinue)
if ($zettaViMissing) {
    function vi { & zetta vi @args }
}

function zvi { & zetta vi @args }

function zwt {
    switch ($args[0]) {
        'new' {
            $operationArgs = @($args | Select-Object -Skip 1)
            if ($operationArgs -contains '--help' -or $operationArgs -contains '-h') {
                & zetta wt new @operationArgs
                return
            } elseif ($operationArgs -contains '--path-only' -or $operationArgs -contains '-P') {
                $path = @(& zetta wt new @operationArgs)
            } else {
                $path = @(& zetta wt new --path-only @operationArgs)
            }
            if ($LASTEXITCODE -ne 0 -or $path.Count -ne 1) { return }
            Set-Location -LiteralPath $path[0]
        }
        'done' {
            $operationArgs = @($args | Select-Object -Skip 1)
            if ($operationArgs -contains '--help' -or $operationArgs -contains '-h') {
                & zetta wt done @operationArgs
                return
            } elseif ($operationArgs -contains '--path-only' -or $operationArgs -contains '-P') {
                $path = @(& zetta wt done @operationArgs)
            } else {
                $path = @(& zetta wt done --path-only @operationArgs)
            }
            if ($LASTEXITCODE -ne 0 -or $path.Count -ne 1) { return }
            Set-Location -LiteralPath $path[0]
        }
        default {
            & zetta wt @args
        }
    }
}

function ztftp { & zetta tftp @args }
function zntfy { & zetta notify @args }
function zcopy { & zetta copy @args }
function zpaste { & zetta paste @args }

# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone
# there. Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working;
# any preexisting pbcopy/pbpaste alias (eg. one pointing at a third-party
# tool) is removed first so Zetta's functions take priority over it. As
# above, $IsMacOS is unset (falsy) on Windows PowerShell 5.1.
if (-not $IsMacOS) {
    Remove-Item -Path Alias:pbcopy,Alias:pbpaste -ErrorAction SilentlyContinue
    function pbcopy { & zetta copy @args }
    function pbpaste { & zetta paste @args }
}

$zettaProfiles = { param($configArguments) @(& zetta profile list @configArguments 2>$null) }
$zettaProfileThemes = { param($configArguments) @(& zetta profile themes @configArguments 2>$null) }
$zettaOverlayColors = @(ZETTA_OVERLAY_COLORS)
$zettaTabIcons = { @(& zetta tabicon --list 2>$null) }
$zettaPaneThemes = { @(& zetta panetheme --list 2>$null) }
$zettaSplits = { @(& zetta splits 2>$null) }

# zetta-default/zetta-ok/zetta-alarm are bundled tones Zetta plays itself, so
# they always work; the rest are the current platform's own system sound
# names, which only work on that platform, so only that platform's names are
# offered. $IsMacOS/$IsLinux are unset on Windows PowerShell 5.1, which only
# runs on Windows, so the Windows branch is also the correct fallback there.
$zettaSoundNames = @('zetta-default', 'zetta-ok', 'zetta-alarm') + $(
    if ($IsMacOS) {
        'Basso', 'Blow', 'Bottle', 'Frog', 'Funk', 'Glass', 'Hero', 'Morse', 'Ping', 'Pop', 'Purr', 'Sosumi', 'Submarine', 'Tink'
    } elseif ($IsLinux) {
        'bell', 'complete', 'message', 'message-new-instant', 'dialog-information', 'dialog-warning', 'dialog-error', 'trash-empty'
    } else {
        'Default', 'IM', 'Mail', 'Reminder', 'SMS'
    }
)

$zettaSessionIds = {
    try {
        $catalogs = @(zetta sessions --json 2>$null | ConvertFrom-Json)
        foreach ($catalog in $catalogs) {
            foreach ($session in @($catalog.sessions)) {
                "{0}:{1}:{2}" -f $catalog.process_id, $catalog.runner_id, $session.id
            }
        }
    } catch {}
}

$zettaCompletions = {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandName = $commandAst.CommandElements[0].Value
    $words = @($commandAst.CommandElements | ForEach-Object { $_.Value })
    $previous = if ($words.Count -gt 1) { $words[$words.Count - 2] } else { '' }
    $last = if ($words.Count -gt 1) { $words[$words.Count - 1] } else { '' }

    $configArguments = @()
    for ($index = 1; $index -lt $words.Count; $index++) {
        if ($words[$index] -in '--config', '-c' -and $index + 1 -lt $words.Count) {
            $configArguments += '--config'
            $configArguments += $words[$index + 1]
            $index++
        }
    }
    $profileOperation = ''
    $profileOperationIndex = -1
    $profileIndex = -1
    for ($index = 1; $index -lt $words.Count; $index++) {
        if ($words[$index] -in '--config', '-c', '--keymap', '-k', '--profile', '-p', '--split', '-s', '--theme', '-t') {
            $index++
        } elseif ($words[$index] -eq 'profile') {
            $profileIndex = $index
            break
        }
    }
    $subcommand = $words | Where-Object {
        $_ -in 'benchmark', 'benchmark-output', 'terminal-size', 'sessions', 'splits', 'edit', 'vi', 'init', 'serial', 'http', 'tftp', 'notify', 'attention', 'copy', 'paste', 'tabicon', 'panetheme', 'overlay', 'wt'
    } | Select-Object -First 1
    $worktreeCommand = $false
    $worktreeOperation = ''
    if ($commandName -eq 'zwt') {
        $worktreeCommand = $true
        if ($words.Count -gt 1) { $worktreeOperation = $words[1] }
    } elseif ($subcommand -eq 'wt') {
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

    $candidates = if ($commandName -eq 'ztftp') {
        if ($words.Count -le 1) { 'get', 'put', '--help' } else { '--port', '--help' }
    } elseif ($commandName -eq 'zntfy') {
        if ($previous -in '--timeout', '-t') { 'default', 'never' }
        elseif ($previous -in '--sound', '-s') { $zettaSoundNames }
        else { '--app-name', '--icon', '--sound', '--timeout', '--help' }
    } elseif ($commandName -in 'zcopy', 'pbcopy') {
        if ($previous -in '--pboard', '-pboard') { 'general', 'ruler', 'find', 'font' }
        else { '--pboard', '--help' }
    } elseif ($commandName -in 'zpaste', 'pbpaste') {
        if ($previous -in '--pboard', '-pboard') { 'general', 'ruler', 'find', 'font' }
        elseif ($previous -in '--prefer', '-prefer', '--Prefer', '-Prefer') { 'txt', 'rtf', 'ps' }
        else { '--pboard', '--prefer', '--help' }
    } elseif (
        $previous -eq '--split' -or $last -eq '--split' -or
        (($previous -eq '-s' -or $last -eq '-s') -and $null -eq $subcommand)
    ) {
        & $zettaSplits
    } elseif ($previous -eq '--replace-pane' -or ($previous -eq '-r' -and $null -eq $subcommand)) {
        if ($wordToComplete -like '-*' -or [string]::IsNullOrEmpty($wordToComplete)) {
            '--help', '--version', '--config', '--keymap', '--profile', '--split', '--theme'
        } else {
            @()
        }
    } elseif (
        $previous -eq '--profile' -or $last -eq '--profile' -or
        (($previous -eq '-p' -or $last -eq '-p') -and $null -eq $subcommand)
    ) {
        & $zettaProfiles $configArguments
    } elseif ($previous -in '--timeout', '-t') {
        'default', 'never'
    } elseif ($previous -in '--output-type', '-t', '--theme', '--text') {
        if ($subcommand -eq 'profile' -or $null -eq $subcommand) { & $zettaProfileThemes $configArguments }
        elseif ($subcommand -eq 'panetheme') { & $zettaPaneThemes }
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
    } elseif ($previous -in '--opacity', '-o') {
        @()
    } elseif ($worktreeCommand -and $worktreeOperation -eq 'new' -and $previous -in '--copy', '-c') {
        @(Get-ChildItem -Name -Path "$wordToComplete*" -ErrorAction SilentlyContinue)
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
    } elseif ($subcommand -eq 'tabicon' -and (
        $previous -in '--icon', '-i' -or $wordToComplete -notlike '-*'
    )) {
        & $zettaTabIcons
    } elseif ($subcommand -eq 'panetheme' -and $wordToComplete -notlike '-*') {
        & $zettaPaneThemes
    } elseif ($subcommand -eq 'profile' -and $profileOperation -eq 'theme' -and $wordToComplete -notlike '-*' -and $words -notcontains '--reset' -and $words -notcontains '-r' -and -not ($profileOperationIndex -eq ($words.Count - 1) -and -not [string]::IsNullOrEmpty($wordToComplete))) {
        $profileArguments = @($words | Select-Object -Skip ($profileIndex + 2) | Where-Object { $_ -notlike '-*' -and -not [string]::IsNullOrEmpty($_) })
        if ($profileArguments.Count -ge 2 -or ($profileArguments.Count -eq 1 -and [string]::IsNullOrEmpty($wordToComplete))) { & $zettaProfileThemes $configArguments }
        else { & $zettaProfiles $configArguments }
    } elseif ($subcommand -eq 'sessions' -and $words.Count -ge 3 -and $words[2] -eq 'reconnect') {
        if ($previous -in '--session', '-s') { @() } else { & $zettaSessionIds }
    } elseif ($worktreeCommand) {
        if ([string]::IsNullOrEmpty($worktreeOperation)) {
            'new', 'done', 'status', 'rerere', '--help'
        } elseif ($worktreeOperation -eq 'new') {
            '--copy', '--path-only', '--help'
        } elseif ($worktreeOperation -eq 'done') {
            '--path-only', '--help'
        } else {
            '--help'
        }
    } elseif ($null -eq $subcommand) {
        'benchmark', 'benchmark-output', 'terminal-size', 'sessions', 'profile', 'splits', 'edit', 'vi', 'init', 'serial', 'http', 'tftp', 'notify', 'attention', 'copy', 'paste', 'tabicon', 'panetheme', 'overlay', 'wt', '--help', '--version', '--config', '--keymap', '--profile', '--split', '--replace-pane', '--theme'
    } else {
        switch ($subcommand) {
            'benchmark' { '--terminal-render-workload', '--terminal-checkerboard-workload', '--terminal-sparse-update-workload', '--profile-report', '--profile-duration', '--profile-pane-stress', '--profile-background-stress', '--profile-sparse-updates', '--profile-external-terminal', '--help' }
            'benchmark-output' { '--size', '--output-type', '--help' }
            'terminal-size' { '--json', '--resize', '--columns', '--rows', '--help' }
            'edit' { '--delete-after', '--help' }
            'vi' { '--help' }
            'sessions' {
                if ($words.Count -le 2 -or ($words.Count -eq 3 -and $words[2] -ne 'reconnect')) {
                    'reconnect', '--json', '--help'
                } elseif ($words[2] -eq 'reconnect') {
                    if ($last -eq 'reconnect') { & $zettaSessionIds } else { '--session', '--help' }
                } else { '--json', '--help' }
            }
            'splits' { '--help' }
            'profile' {
                if ([string]::IsNullOrEmpty($profileOperation) -or ($profileOperationIndex -eq ($words.Count - 1) -and -not [string]::IsNullOrEmpty($wordToComplete)) -or $profileOperation -notin 'list', 'themes', 'disable', 'enable', 'theme', 'default', 'add', 'remove') {
                    'list', 'themes', 'disable', 'enable', 'theme', 'default', 'add', 'remove', '--config', '--help'
                } elseif ($profileOperation -in 'disable', 'enable', 'default', 'remove') {
                    if ($previous -eq $profileOperation -or $last -eq $profileOperation) { & $zettaProfiles $configArguments } else { '--config', '--help' }
                } elseif ($profileOperation -eq 'theme') {
                    if ($wordToComplete -like '-*') { '--reset', '--config', '--help' }
                    elseif ($previous -eq 'theme' -or $last -eq 'theme') { & $zettaProfiles $configArguments }
                    elseif ($previous -in '--reset', '-r' -or $last -in '--reset', '-r') { '--config', '--help' }
                    else { & $zettaProfileThemes $configArguments }
                } elseif ($profileOperation -eq 'add') {
                    '--program', '--arg', '--theme', '--config', '--help'
                } else { '--config', '--help' }
            }
            'init' { 'bash', 'fish', 'powershell', 'pwsh', 'zsh', '--help' }
            'serial' {
                if ($words.Count -le 2) { 'console', 'list', '--help' }
                elseif ($words[2] -eq 'console') { '--device', '--baud-rate', '--data-bits', '--parity', '--stop-bits', '--flow-control', '--help' }
            }
            'http' {
                if ($words.Count -le 2) { 'server', '--help' } else { '--root', '--port', '--config', '--help' }
            }
            'tftp' {
                if ($words.Count -le 2) { 'get', 'put', 'server', '--help' }
                elseif ($words[2] -eq 'server') { '--root', '--port', '--config', '--help' }
                else { '--port', '--help' }
            }
            'notify' { '--app-name', '--icon', '--sound', '--timeout', '--help' }
            'attention' { '--notify', '--app-name', '--icon', '--sound', '--timeout', '--help' }
            'copy' { '--pboard', '--help' }
            'paste' { '--pboard', '--prefer', '--help' }
            'tabicon' { '--icon', '--list', '--help' }
            'panetheme' { '--theme', '--reset', '--list', '--help' }
            'overlay' { '--text', '--size', '--opacity', '--color', '--reset', '--help' }
            'wt' {
                if ([string]::IsNullOrEmpty($worktreeOperation)) {
                    'new', 'done', 'status', 'rerere', '--help'
                } elseif ($worktreeOperation -eq 'new') {
                    '--copy', '--path-only', '--help'
                } elseif ($worktreeOperation -eq 'done') {
                    '--path-only', '--help'
                } else {
                    '--help'
                }
            }
        }
    }

    $candidates = @($candidates | Where-Object {
        if ($_ -like '-*') { $_ -eq '--copy' -or $_ -notin $words } else { $true }
    })
    $candidates | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
        $value = $_
        $text = if ($value -match '\s' -or $value.Contains("'") -or $value.Contains('"')) {
            "'" + $value.Replace("'", "''") + "'"
        } else {
            $value
        }
        [System.Management.Automation.CompletionResult]::new($text, $value, 'ParameterValue', $value)
    }
}

Register-ArgumentCompleter -Native -CommandName zetta -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName ztftp -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zntfy -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zcopy -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zpaste -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zvi -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zwt -ScriptBlock $zettaCompletions
if ($zettaViMissing) {
    Register-ArgumentCompleter -CommandName vi -ScriptBlock $zettaCompletions
}
if (-not $IsMacOS) {
    Register-ArgumentCompleter -CommandName pbcopy -ScriptBlock $zettaCompletions
    Register-ArgumentCompleter -CommandName pbpaste -ScriptBlock $zettaCompletions
}
