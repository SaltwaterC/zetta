# Exercise upgrade exit handling without installing files or changing user PATH.
# Run with powershell.exe to cover Windows PowerShell's native stderr behavior.
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$installer = Join-Path $PSScriptRoot "install-windows.ps1"
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($installer, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw "Installer parse failed: $parseErrors" }
$definition = $ast.Find({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq "Invoke-MuxUpgrade"
}, $false)
if (-not $definition) { throw "Invoke-MuxUpgrade was not found" }
. ([scriptblock]::Create($definition.Extent.Text))

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

$testRoot = Join-Path ([IO.Path]::GetTempPath()) ("zetta-mux-upgrade-" + [Guid]::NewGuid().ToString("N"))
$installedMuxBinary = Join-Path $testRoot "mux.cmd"
$UpgradeMux = $true
$muxEnabled = $true
try {
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    [IO.File]::WriteAllText($installedMuxBinary, "@echo off`r`necho zmux: no multiplexer is running 1>&2`r`nexit /b 1`r`n")
    Invoke-MuxUpgrade
    Assert-True ($ErrorActionPreference -eq "Stop") "no-daemon handling changed error preference"

    [IO.File]::WriteAllText($installedMuxBinary, "@echo off`r`necho zmux: upgrade refused 1>&2`r`nexit /b 1`r`n")
    $failed = $false
    try { Invoke-MuxUpgrade } catch {
        Assert-True ($_ -match "Could not upgrade the installed multiplexer:.*upgrade refused") "upgrade error lost its diagnostic: $_"
        $failed = $true
    }
    Assert-True $failed "a real upgrade failure was ignored"
    Assert-True ($ErrorActionPreference -eq "Stop") "failed upgrade changed error preference"

    [IO.File]::WriteAllText($installedMuxBinary, "@echo off`r`necho upgraded`r`nexit /b 0`r`n")
    Invoke-MuxUpgrade
    Assert-True ($ErrorActionPreference -eq "Stop") "successful upgrade changed error preference"
    Write-Host "Windows installer multiplexer upgrade tests passed."
} finally {
    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
