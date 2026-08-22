$terminalTrackerActive = Get-Variable -Name __ZettaCwdTrackerInstalled -Scope Global -ErrorAction SilentlyContinue
if (-not $terminalTrackerActive) {
    $global:__ZettaCwdTrackerInstalled = $true
    $global:__ZettaOriginalPrompt = $function:prompt
    function global:prompt {
        try {
            $zettaDirectory = $ExecutionContext.SessionState.Path.CurrentFileSystemLocation.ProviderPath
            [Console]::Write("$([char]27)]2;zetta-cwd:$zettaDirectory$([char]27)\")
        } catch {}
        if ($null -ne $global:__ZettaOriginalPrompt) {
            & $global:__ZettaOriginalPrompt
        } else {
            "PS $($ExecutionContext.SessionState.Path.CurrentLocation)> "
        }
    }
}
