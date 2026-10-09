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

$zettaMoshMissing = $null -eq (Get-Command mosh -ErrorAction SilentlyContinue)
if ($zettaMoshMissing) {
    function global:mosh { & zetta mosh @args }
}

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

# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
Remove-Item -Path Function:zcopy,Function:zpaste -ErrorAction SilentlyContinue
# Real pbcopy/pbpaste already exist on macOS, so Zetta leaves them alone
# there. Elsewhere, Zetta's pbcopy/pbpaste keep the muscle memory working;
# any preexisting pbcopy/pbpaste alias (eg. one pointing at a third-party
# tool) is removed first so Zetta's functions take priority over it. As
# above, $IsMacOS is unset (falsy) on Windows PowerShell 5.1.
if (-not $IsMacOS) {
    Remove-Item -Path Alias:pbcopy,Alias:pbpaste -ErrorAction SilentlyContinue
    function pbcopy { & zcopy @args }
    function pbpaste { & zpaste @args }
}
# ZETTA_CLIPBOARD_INTEGRATION_END


if (-not (Get-Variable __ZettaCompletionState -Scope Global -ErrorAction SilentlyContinue)) {
    $global:__ZettaCompletionState = @{ Handler = $null; Loading = $false }
}
$zettaLazyCompletions = {
    param($wordToComplete, $commandAst, $cursorPosition)
    $state = $global:__ZettaCompletionState
    if ($state.Loading) { return }
    if ($null -eq $state.Handler) {
        $state.Loading = $true
        try {
            $payload = & zetta init powershell --completions | Out-String
            if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($payload)) {
                Invoke-Expression $payload
            }
        } finally { $state.Loading = $false }
    }
    if ($null -ne $state.Handler) {
        & $state.Handler $wordToComplete $commandAst $cursorPosition
    }
}
Register-ArgumentCompleter -Native -CommandName zetta -ScriptBlock $zettaLazyCompletions
Register-ArgumentCompleter -Native -CommandName zosh -ScriptBlock $zettaLazyCompletions
# ZETTA_ZMUX_INTEGRATION_BEGIN
Register-ArgumentCompleter -Native -CommandName zmux -ScriptBlock $zettaLazyCompletions
# ZETTA_ZMUX_INTEGRATION_END
Register-ArgumentCompleter -CommandName ztftp -ScriptBlock $zettaLazyCompletions
Register-ArgumentCompleter -CommandName zntfy -ScriptBlock $zettaLazyCompletions
# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
Register-ArgumentCompleter -CommandName zcopy -ScriptBlock $zettaLazyCompletions
Register-ArgumentCompleter -CommandName zpaste -ScriptBlock $zettaLazyCompletions
# ZETTA_CLIPBOARD_INTEGRATION_END
Register-ArgumentCompleter -CommandName zvi -ScriptBlock $zettaLazyCompletions
# ZETTA_WORKTREE_INTEGRATION_BEGIN
Register-ArgumentCompleter -CommandName zwt -ScriptBlock $zettaLazyCompletions
# ZETTA_WORKTREE_INTEGRATION_END
if ($zettaViMissing) {
    Register-ArgumentCompleter -CommandName vi -ScriptBlock $zettaLazyCompletions
}
if ($zettaMoshMissing) {
    Register-ArgumentCompleter -CommandName mosh -ScriptBlock $zettaLazyCompletions
}
# ZETTA_CLIPBOARD_INTEGRATION_BEGIN
if (-not $IsMacOS) {
    Register-ArgumentCompleter -CommandName pbcopy -ScriptBlock $zettaLazyCompletions
    Register-ArgumentCompleter -CommandName pbpaste -ScriptBlock $zettaLazyCompletions
}
# ZETTA_CLIPBOARD_INTEGRATION_END
