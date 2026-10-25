$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows-process.ps1')

Import-VisualStudioEnvironment
Repair-MsvcLinkerPath

$cargo = $env:CARGO
if ([string]::IsNullOrWhiteSpace($cargo)) {
    $cargo = 'cargo'
}

$exitCode = Invoke-WindowsProcess -FilePath $cargo -Arguments $args
exit $exitCode
