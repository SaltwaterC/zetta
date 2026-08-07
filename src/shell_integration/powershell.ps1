# Zetta shell integration for PowerShell.
if (-not (Test-Path Env:EDITOR)) {
    $env:EDITOR = 'zetta vi'
}

$zettaViMissing = -not (Get-Command vi -ErrorAction SilentlyContinue)
if ($zettaViMissing) {
    function vi { & zetta vi @args }
}

function zvi { & zetta vi @args }

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

$zettaProfiles = @(ZETTA_PROFILES)
$zettaTabIcons = { @(& zetta tabicon --list 2>$null) }
$zettaPaneThemes = { @(& zetta panetheme --list 2>$null) }

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
    $subcommand = $words | Where-Object {
        $_ -in 'benchmark', 'benchmark-output', 'terminal-size', 'sessions', 'edit', 'vi', 'init', 'serial', 'http', 'tftp', 'notify', 'copy', 'paste', 'tabicon', 'panetheme', 'overlay'
    } | Select-Object -First 1

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
        $previous -eq '--profile' -or $last -eq '--profile' -or
        (($previous -eq '-p' -or $last -eq '-p') -and $null -eq $subcommand)
    ) {
        $zettaProfiles
    } elseif ($previous -eq '--timeout') {
        'default', 'never'
    } elseif ($previous -in '--output-type', '-t', '--theme', '--text') {
        if ($subcommand -eq 'panetheme' -or $null -eq $subcommand) { & $zettaPaneThemes }
        elseif ($subcommand -eq 'notify') { 'default', 'never' }
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
        elseif ($subcommand -eq 'notify') { $zettaSoundNames }
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
    } elseif ($previous -eq '--color') {
        @()
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
    } elseif ($subcommand -eq 'sessions' -and $words.Count -ge 3 -and $words[2] -eq 'reconnect') {
        if ($previous -in '--session', '-s') { @() } else { & $zettaSessionIds }
    } elseif ($null -eq $subcommand) {
        'benchmark', 'benchmark-output', 'terminal-size', 'sessions', 'edit', 'vi', 'init', 'serial', 'http', 'tftp', 'notify', 'copy', 'paste', 'tabicon', 'panetheme', 'overlay', '--help', '--version', '--config', '--keymap', '--profile', '--theme'
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
            'copy' { '--pboard', '--help' }
            'paste' { '--pboard', '--prefer', '--help' }
            'tabicon' { '--icon', '--list', '--help' }
            'panetheme' { '--theme', '--reset', '--list', '--help' }
            'overlay' { '--text', '--size', '--opacity', '--color', '--reset', '--help' }
        }
    }

    $candidates = @($candidates | Where-Object {
        if ($_ -like '-*') { $_ -notin $words } else { $true }
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
if ($zettaViMissing) {
    Register-ArgumentCompleter -CommandName vi -ScriptBlock $zettaCompletions
}
if (-not $IsMacOS) {
    Register-ArgumentCompleter -CommandName pbcopy -ScriptBlock $zettaCompletions
    Register-ArgumentCompleter -CommandName pbpaste -ScriptBlock $zettaCompletions
}
