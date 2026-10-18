param([string] $IntegrationPath, [string] $UpgradeTracker)

function global:prompt { 'TAIL_PROMPT> ' }
Import-Module PSReadLine
Set-PSReadLineOption -HistorySaveStyle SaveNothing
if ($UpgradeTracker -eq 'upgrade') {
    # An already running shell can source the new integration over its old one.
    $global:__ZettaCwdTrackerInstalled = $true
    $global:__ZettaOriginalPrompt = $function:prompt
    $global:__ZettaLifecycleTrackerInstalled = $true
    $global:__ZettaLifecycleTrackingEnabled = $true
}
. $IntegrationPath

# A fresh shell prompt repairs a mode inherited from startup code.
[Console]::TreatControlCAsInput = $true
prompt | Out-Null
if ([Console]::TreatControlCAsInput) {
    throw 'Initial prompt did not restore Ctrl-C'
}

# PSReadLine's InvokePrompt redraws an existing prompt in raw mode.
[Console]::TreatControlCAsInput = $true
prompt | Out-Null
if (-not [Console]::TreatControlCAsInput) {
    throw 'Editing redraw changed raw input'
}

# Returning from a command that leaked raw mode must repair it again.
__zetta_report_command_started 'leaked-mode'
Write-Output 'RECOVERY_READY'

function global:__zetta_leak_mode {
    __zetta_report_command_started 'leaked-mode'
    [Console]::TreatControlCAsInput = $true
    Write-Output 'LEAK_READY'
}
