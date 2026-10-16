# The nonce Zetta gave this shell. Lifecycle and working-directory markers
# carry it so that a `command-finished` or a directory printed by anything else
# cannot satisfy a `zetta pane wait` or choose where a split, a file server or a
# project starts. Kept in a global, not the environment, which programs
# inherit. A tracker that ran first may already hold it, so an absent
# variable keeps it.
if (Test-Path Env:__ZETTA_COMMAND_MARKER_NONCE) {
    $global:__ZettaMarkerNonce = $env:__ZETTA_COMMAND_MARKER_NONCE
    Remove-Item Env:__ZETTA_COMMAND_MARKER_NONCE
} elseif ($null -eq (Get-Variable -Name __ZettaMarkerNonce -Scope Global -ErrorAction SilentlyContinue)) {
    $global:__ZettaMarkerNonce = ''
}
$terminalTrackerActive = Get-Variable -Name __ZettaCwdTrackerInstalled -Scope Global -ErrorAction SilentlyContinue
if (-not $terminalTrackerActive) {
    $global:__ZettaCwdTrackerInstalled = $true
    $global:__ZettaLifecycleTrackerInstalled = $true
    $global:__ZettaLifecycleTrackingEnabled =
        (-not [string]::IsNullOrEmpty($env:ZETTA_PANE_ROUTING_ID)) -or
        (-not [string]::IsNullOrEmpty($env:ZETTA_PANE_ID))
    $global:__ZettaCommandStarted = $false
    $global:__ZettaOriginalPrompt = $function:prompt
    $global:__ZettaOriginalCommandValidationHandler = $null
    function global:__zetta_report_tracking_ready {
        if (-not $global:__ZettaLifecycleTrackingEnabled) { return }
        [Console]::Write("$([char]27)]2;zetta-event;$($global:__ZettaMarkerNonce):tracking-ready$([char]27)\")
    }
    function global:__zetta_report_command_started([string] $command) {
        if (-not $global:__ZettaLifecycleTrackingEnabled) { return }
        if ($global:__ZettaCommandStarted) { return }
        $global:__ZettaCommandStarted = $true
        [Console]::Write("$([char]27)]2;zetta-event;$($global:__ZettaMarkerNonce):command-started:$command$([char]27)\")
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
                [Console]::Write("$([char]27)]2;zetta-event;$($global:__ZettaMarkerNonce):command-finished:$status$([char]27)\")
                $global:__ZettaCommandStarted = $false
            }
            $zettaDirectory = $ExecutionContext.SessionState.Path.CurrentFileSystemLocation.ProviderPath
            # Set-Location changes PowerShell's provider location, not the native
            # process CWD queried by zmux when a remote viewer splits this pane.
            [Environment]::CurrentDirectory = $zettaDirectory
            if ($global:__ZettaMarkerNonce) {
                [Console]::Write("$([char]27)]2;zetta-cwd;$($global:__ZettaMarkerNonce):$zettaDirectory$([char]27)\")
            } else {
                [Console]::Write("$([char]27)]2;zetta-cwd:$zettaDirectory$([char]27)\")
            }
            [Console]::Write("$([char]27)[0m")
        } catch {}
        if ($null -ne $global:__ZettaOriginalPrompt) {
            & $global:__ZettaOriginalPrompt
        } else {
            "PS $($ExecutionContext.SessionState.Path.CurrentLocation)> "
        }
    }
    __zetta_report_tracking_ready
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

# This script runs through -NoExit -Command before PowerShell's first prompt.
# Load the rest of Zetta's integration here so no setup command is typed into
# the terminal and echoed by PSReadLine. The executable path comes from the
# pane environment, not PATH, which may still name an older Zetta build.
if (-not [string]::IsNullOrEmpty($env:ZETTA_HOST_EXECUTABLE)) {
    & $env:ZETTA_HOST_EXECUTABLE init powershell | Out-String | Invoke-Expression
}
