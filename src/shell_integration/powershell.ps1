# Zetta shell integration for PowerShell.
$terminalTrackerActive = Get-Variable -Name __ZettaCwdTrackerInstalled -Scope Global -ErrorAction SilentlyContinue
$zettaPaneIdentityAvailable =
    (-not [string]::IsNullOrEmpty($env:ZETTA_PANE_ROUTING_ID)) -or
    (-not [string]::IsNullOrEmpty($env:ZETTA_PANE_ID))
if (-not $terminalTrackerActive) {
    $global:__ZettaCwdTrackerInstalled = $true
    $global:__ZettaOriginalPrompt = $function:prompt
}

# Install, or upgrade, the lifecycle half of the tracker. Keeping the prompt
# implementation in one place is important: sourcing a newer integration must
# not create a second CWD marker or wrap the saved prompt recursively.
function global:__zetta_install_lifecycle_tracking([bool] $trackingEnabled) {
    $global:__ZettaLifecycleTrackerInstalled = $true
    $global:__ZettaLifecycleTrackingEnabled = $trackingEnabled
    $global:__ZettaCommandStarted = $false
    if ($null -eq (Get-Variable -Name __ZettaOriginalPrompt -Scope Global -ErrorAction SilentlyContinue)) {
        $global:__ZettaOriginalPrompt = $function:prompt
    }
    $global:__ZettaOriginalCommandValidationHandler = $null
    function global:__zetta_report_tracking_ready {
        if (-not $global:__ZettaLifecycleTrackingEnabled) { return }
        [Console]::Write("$([char]27)]2;zetta-event:tracking-ready$([char]27)\")
    }
    function global:__zetta_report_command_started([string] $command) {
        if (-not $global:__ZettaLifecycleTrackingEnabled) { return }
        if ($global:__ZettaCommandStarted) { return }
        $global:__ZettaCommandStarted = $true
        [Console]::Write("$([char]27)]2;zetta-event:command-started:$command$([char]27)\")
    }
    function global:prompt {
        $promptSucceeded = $?
        try {
            if ($global:__ZettaLifecycleTrackingEnabled -and $global:__ZettaCommandStarted) {
                $status = if ($promptSucceeded) {
                    0
                } elseif ($null -ne $global:LASTEXITCODE -and $global:LASTEXITCODE -ne 0) {
                    [int]$global:LASTEXITCODE
                } else {
                    1
                }
                [Console]::Write("$([char]27)]2;zetta-event:command-finished:$status$([char]27)\")
                $global:__ZettaCommandStarted = $false
            }
            $zettaDirectory = $ExecutionContext.SessionState.Path.CurrentFileSystemLocation.ProviderPath
            [Console]::Write("$([char]27)]2;zetta-cwd:$zettaDirectory$([char]27)\")
            [Console]::Write("$([char]27)[0m")
        } catch {}
        if ($null -ne $global:__ZettaOriginalPrompt) {
            & $global:__ZettaOriginalPrompt
        } else {
            "PS $($ExecutionContext.SessionState.Path.CurrentLocation)> "
        }
    }
    __zetta_report_tracking_ready
    if ($global:__ZettaLifecycleTrackingEnabled) {
        $zettaReadLine = Get-Command Set-PSReadLineOption -ErrorAction SilentlyContinue
        if ($null -ne $zettaReadLine -and $zettaReadLine.Parameters.ContainsKey('CommandValidationHandler')) {
            try {
                $global:__ZettaOriginalCommandValidationHandler = (Get-PSReadLineOption).CommandValidationHandler
                $global:__ZettaCommandValidationHandler = {
                    param([System.Management.Automation.Language.CommandAst] $commandAst)
                    $command = $commandAst.Extent.Text
                    if (-not [string]::IsNullOrWhiteSpace($command)) {
                        __zetta_report_command_started $command
                    }
                    if ($null -ne $global:__ZettaOriginalCommandValidationHandler) {
                        return & $global:__ZettaOriginalCommandValidationHandler $commandAst
                    }
                    return $true
                }
                Set-PSReadLineOption -CommandValidationHandler $global:__ZettaCommandValidationHandler
            } catch {}
        }
    }
}

$zettaLifecycleTracker = Get-Variable -Name __ZettaLifecycleTrackerInstalled -Scope Global -ErrorAction SilentlyContinue
if ($null -eq $zettaLifecycleTracker -or
    ($zettaPaneIdentityAvailable -and -not $global:__ZettaLifecycleTrackingEnabled)) {
    __zetta_install_lifecycle_tracking $zettaPaneIdentityAvailable
}

if (-not (Test-Path Env:EDITOR)) {
    $env:EDITOR = 'zetta vi'
}

$zettaViMissing = -not (Get-Command vi -ErrorAction SilentlyContinue)
if ($zettaViMissing) {
    function vi { & zetta vi @args }
}

function zvi { & zetta vi @args }

# ZETTA_WORKTREE_INTEGRATION_BEGIN
function zwt {
    $zwtApplication = Get-Command zwt -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1 -ExpandProperty Path
    if (-not $zwtApplication) {
        Write-Error "The zwt application was not found on PATH"
        return
    }
    switch ($args[0]) {
        'new' {
            $operationArgs = @($args | Select-Object -Skip 1)
            if ($operationArgs -contains '--help' -or $operationArgs -contains '-h') {
                & $zwtApplication new @operationArgs
                return
            } elseif ($operationArgs -contains '--path-only' -or $operationArgs -contains '-P') {
                $path = @(& $zwtApplication new @operationArgs)
            } else {
                $path = @(& $zwtApplication new --path-only @operationArgs)
            }
            if ($LASTEXITCODE -ne 0 -or $path.Count -ne 1) { return }
            Set-Location -LiteralPath $path[0]
        }
        'done' {
            $operationArgs = @($args | Select-Object -Skip 1)
            if ($operationArgs -contains '--help' -or $operationArgs -contains '-h') {
                & $zwtApplication done @operationArgs
                return
            } elseif ($operationArgs -contains '--path-only' -or $operationArgs -contains '-P') {
                $path = @(& $zwtApplication done @operationArgs)
            } else {
                $path = @(& $zwtApplication done --path-only @operationArgs)
            }
            if ($LASTEXITCODE -ne 0 -or $path.Count -ne 1) { return }
            Set-Location -LiteralPath $path[0]
        }
        'abort' {
            $operationArgs = @($args | Select-Object -Skip 1)
            if ($operationArgs -contains '--help' -or $operationArgs -contains '-h') {
                & $zwtApplication abort @operationArgs
                return
            } elseif ($operationArgs -contains '--path-only' -or $operationArgs -contains '-P') {
                $path = @(& $zwtApplication abort @operationArgs)
            } else {
                $path = @(& $zwtApplication abort --path-only @operationArgs)
            }
            if ($LASTEXITCODE -ne 0 -or $path.Count -ne 1) { return }
            Set-Location -LiteralPath $path[0]
        }
        default {
            & $zwtApplication @args
        }
    }
}
# ZETTA_WORKTREE_INTEGRATION_END

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

$zmuxSessionIds = {
    try {
        $lines = if ($commandName -eq 'zmux') { @(zmux list 2>$null) } else { @(zetta mux list 2>$null) }
        foreach ($line in $lines) {
            if ($line -match '^\s*reconnect\s+id:\s+(\d+:\d+:\d+)(?:\s+\(short:\s+\d+\))?\s*$') { $matches[1] }
        }
    } catch {}
}

$zmuxRestorableIds = {
    try {
        $lines = if ($commandName -eq 'zmux') { @(zmux list 2>$null) } else { @(zetta mux list 2>$null) }
        foreach ($line in $lines) {
            if ($line -match '^\s*resume\s+id:\s+(\d+)\s*$') { $matches[1] }
        }
    } catch {}
}

$zettaCompletions = {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandName = $commandAst.CommandElements[0].Value
    $noMux = $env:ZETTA_NO_MUX -eq '1'
    $words = @($commandAst.CommandElements | ForEach-Object { $_.Value })
    if ($commandName -eq 'zmux') {
        # `zmux` takes the same arguments as `zetta mux`, so completing it
        # reuses that logic by inserting the subcommand it stands in for.
        $words = @($words[0], 'mux') + @($words | Select-Object -Skip 1)
    }
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
        $_ -in 'benchmark', 'terminal-size', 'mux', 'splits', 'pane', 'profile', 'project', 'cmd', 'edit', 'vi', 'init', 'serial', 'http', 'tftp', 'notify', 'attention', 'copy', 'paste', 'tabicon', 'theme', 'overlay'ZETTA_WORKTREE_ROOT_COMMANDS
    } | Select-Object -First 1
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
    } elseif (
        $previous -eq '--split' -or $last -eq '--split' -or
        (($previous -eq '-s' -or $last -eq '-s') -and $null -eq $subcommand)
    ) {
        & $zettaSplits
    } elseif ($previous -eq '--replace-pane' -or ($previous -eq '-r' -and $null -eq $subcommand)) {
        if ($wordToComplete -like '-*' -or [string]::IsNullOrEmpty($wordToComplete)) {
            '--help', '--version', '--config', '--keymap', '--profile', '--split', '--theme', '--no-mux', '--new-window', '--command'
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
    } elseif ($subcommand -eq 'tabicon' -and (
        $previous -in '--icon', '-i' -or $wordToComplete -notlike '-*'
    )) {
        & $zettaTabIcons
    } elseif ($subcommand -eq 'theme' -and $words.Count -gt 2 -and $words[2] -in 'pane', 'tab' -and $wordToComplete -notlike '-*') {
        & $zettaThemes $words[2]
    } elseif ($subcommand -eq 'profile' -and $profileOperation -in 'theme', 'dark-theme' -and $wordToComplete -notlike '-*' -and $words -notcontains '--reset' -and $words -notcontains '-r' -and -not ($profileOperationIndex -eq ($words.Count - 1) -and -not [string]::IsNullOrEmpty($wordToComplete))) {
        $profileArguments = @($words | Select-Object -Skip ($profileIndex + 2) | Where-Object { $_ -notlike '-*' -and -not [string]::IsNullOrEmpty($_) })
        if ($profileArguments.Count -ge 2 -or ($profileArguments.Count -eq 1 -and [string]::IsNullOrEmpty($wordToComplete))) { & $zettaProfileThemes $configArguments }
        else { & $zettaProfiles $configArguments }
    } elseif ($subcommand -eq 'mux' -and $words.Count -ge 3 -and (
        $words[2] -eq 'reconnect' -or
        $words[2] -eq 'resume' -or
        (-not $noMux -and $words[2] -in 'share', 'unshare', 'kill', 'forget')
    ) -and $wordToComplete -notlike '-*') {
        if ($words[2] -eq 'resume') { & $zmuxRestorableIds } else { & $zmuxSessionIds }
# ZETTA_WORKTREE_INTEGRATION_BEGIN
    } elseif ZETTA_WORKTREE_COMPLETION_CHECK {
        if ([string]::IsNullOrEmpty($worktreeOperation)) {
            'new', 'done', 'abort', 'status', 'rerere', '--help'
        } elseif ($worktreeOperation -eq 'new') {
            '--copy', '--path-only', '--help'
        } elseif ($worktreeOperation -eq 'done' -or $worktreeOperation -eq 'abort') {
            '--path-only', '--help'
        } else {
            '--help'
        }
# ZETTA_WORKTREE_INTEGRATION_END
    } elseif ($null -eq $subcommand) {
        'benchmark', 'terminal-size', 'mux', 'profile', 'project', 'cmd', 'splits', 'pane', 'edit', 'vi', 'init', 'serial', 'http', 'tftp', 'notify', 'attention', 'copy', 'paste', 'tabicon', 'theme', 'overlay'ZETTA_WORKTREE_ROOT_COMMANDS, '--help', '--version', '--config', '--keymap', '--profile', '--split', '--replace-pane', '--theme', '--no-mux', '--new-window', '--command'
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
            'mux' {
                if ($words.Count -le 2) {
                    if ($noMux) { 'list', 'reconnect', '--json', '--help', '--version' }
                    else { 'list', 'stop', 'reconnect', 'resume', 'share', 'unshare', 'kill', 'forget', '--json', '--upgrade', '--identity', '--help', '--version' }
                }
                elseif ($noMux -and $words[2] -notin 'list', 'reconnect') { @() }
                elseif ($words[2] -eq 'stop') { '--force', '--help' }
                elseif ($words[2] -in 'resume', 'reconnect') { '--identity', '--help' }
                else { '--json', '--help' }
            }
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
            'tabicon' { '--icon', '--list', '--help' }
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
                    'new', 'done', 'abort', 'status', 'rerere', '--help'
                } elseif ($worktreeOperation -eq 'new') {
                    '--copy', '--path-only', '--help'
                } elseif ($worktreeOperation -eq 'done' -or $worktreeOperation -eq 'abort') {
                    '--path-only', '--help'
                } else {
                    '--help'
                }
            }
# ZETTA_WORKTREE_INTEGRATION_END
        }
    }

    $candidates = @($candidates | Where-Object {
        if ($_ -like '-*') { ZETTA_WORKTREE_POWERSHELL_REPEATABLE_COPY -or $_ -notin $words } else { $true }
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
Register-ArgumentCompleter -Native -CommandName zmux -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName ztftp -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zntfy -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zcopy -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zpaste -ScriptBlock $zettaCompletions
Register-ArgumentCompleter -CommandName zvi -ScriptBlock $zettaCompletions
# ZETTA_WORKTREE_INTEGRATION_BEGIN
Register-ArgumentCompleter -CommandName zwt -ScriptBlock $zettaCompletions
# ZETTA_WORKTREE_INTEGRATION_END
if ($zettaViMissing) {
    Register-ArgumentCompleter -CommandName vi -ScriptBlock $zettaCompletions
}
if (-not $IsMacOS) {
    Register-ArgumentCompleter -CommandName pbcopy -ScriptBlock $zettaCompletions
    Register-ArgumentCompleter -CommandName pbpaste -ScriptBlock $zettaCompletions
}
