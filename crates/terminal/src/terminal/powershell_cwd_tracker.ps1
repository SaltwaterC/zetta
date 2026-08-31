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
