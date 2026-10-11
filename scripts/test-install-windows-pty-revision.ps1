# Exercise live helper installation in fixtures without changing user PATH.
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$tokens = $null
$parseErrors = $null
$installer = Join-Path $PSScriptRoot "install-windows.ps1"
$ast = [System.Management.Automation.Language.Parser]::ParseFile($installer, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw "Installer parse failed: $parseErrors" }
$names = @("Get-PtyInstallRevision", "Test-PtyInstallCurrent", "Get-VersionedPath", "Get-RollbackPaths", "Get-AvailableRollbackPath", "Test-InstallFilesCurrent", "Install-Binary")
foreach ($definition in $ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false)) {
    if ($definition.Name -in $names) { . ([scriptblock]::Create($definition.Extent.Text)) }
}

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

# Replace only host-system side effects; keep the real installation transaction.
function Get-InstallFiles { return @() }
function Get-PtyInstallFile { return [pscustomobject]@{ Source = $sourcePtyBinary; Destination = $installedPtyBinary } }
function Add-InstallDirectoryToUserPath {}
function Remove-DisabledWorktreeFiles {}
function Remove-DisabledNotifyFiles {}
function Remove-DisabledClipboardFiles {}
function Remove-DisabledMuxFiles {}
function Remove-DisabledZoshFiles {}
function Remove-DisabledZoshServerFiles {}
function Remove-LegacyMoshServerFiles {}

# Inject an activation failure while leaving the actual rollback moves intact.
function Move-Item([string]$LiteralPath, [string]$Destination) {
    if ($script:failActivation -and $LiteralPath -eq (Get-VersionedPath $installedPtyBinary "new")) {
        $script:failActivation = $false
        throw "Injected helper activation failure"
    }
    Microsoft.PowerShell.Management\Move-Item -LiteralPath $LiteralPath -Destination $Destination
}

$testRoot = Join-Path $PSScriptRoot (".pty-revision-test-" + [Guid]::NewGuid().ToString("N"))
$installedPtyBinary = Join-Path $testRoot "zmux-pty.exe"
$installedPtyVersionMarker = Join-Path $testRoot "zmux-pty.version"
$legacyPtyInstallRevision = "1"
$ptyHostEndpoint = Join-Path $testRoot "zmux-host.json"
$InstallDirectory = $testRoot
$muxEnabled = $true
$sourcePtyBinary = Join-Path $testRoot "source-pty.exe"
$sourcePtyVersionMarker = Join-Path $testRoot "source.version"
$runningImage = $null
$script:failActivation = $false
$sourceMarker = Join-Path (Split-Path $PSScriptRoot -Parent) "resources\windows\zmux-pty.version"
try {
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    $revision = Get-PtyInstallRevision $sourceMarker
    [IO.File]::WriteAllText($sourcePtyVersionMarker, $revision)
    [IO.File]::WriteAllText($sourcePtyBinary, "new helper")
    Assert-True ([int]$revision -gt 1) "Ctrl+C fix must replace legacy revision 1"
    Assert-True (-not (Test-PtyInstallCurrent $revision)) "missing helper was treated as current"
    [IO.File]::WriteAllText($installedPtyBinary, "legacy helper")
    Assert-True (-not (Test-PtyInstallCurrent $revision)) "unmarked legacy helper would retain the Ctrl+C bug"
    [IO.File]::WriteAllText($installedPtyVersionMarker, "1")
    Assert-True (-not (Test-PtyInstallCurrent $revision)) "marked legacy helper would retain the Ctrl+C bug"
    # Use an actual mapped executable: a normal file handle does not model
    # Windows preventing deletion of a running image after rename.
    Copy-Item -LiteralPath "$env:SystemRoot\System32\cmd.exe" -Destination $installedPtyBinary -Force
    $runningImage = Start-Process -FilePath $installedPtyBinary -ArgumentList '/d', '/c', 'ping 127.0.0.1 -n 60 >NUL' -WindowStyle Hidden -PassThru
    Start-Sleep -Milliseconds 200
    Assert-True (-not $runningImage.HasExited) "fixture process did not start"
    [IO.File]::WriteAllText($ptyHostEndpoint, (@{ process_id = $runningImage.Id } | ConvertTo-Json))
    Install-Binary
    Assert-True (Test-PtyInstallCurrent $revision) "helper revision was not activated"
    Assert-True ([IO.File]::ReadAllText($installedPtyBinary) -eq "new helper") "new helper was not activated"
    $archives = @(Get-RollbackPaths $installedPtyBinary)
    Assert-True ($archives.Count -eq 1) "running helper generation was not retained"
    Assert-True (-not $runningImage.HasExited) "old host image was disturbed"

    # A second upgrade while the first archived image is live must also work.
    [IO.File]::WriteAllText($sourcePtyVersionMarker, ([int]$revision + 1).ToString())
    [IO.File]::WriteAllText($sourcePtyBinary, "newer helper")
    Install-Binary
    Assert-True ([IO.File]::ReadAllText($installedPtyBinary) -eq "newer helper") "repeated helper upgrade failed"
    Assert-True (Test-Path -LiteralPath $archives[0]) "live archive was removed on repeated upgrade"

    $previousRevision = [IO.File]::ReadAllText($installedPtyVersionMarker)
    [IO.File]::WriteAllText($sourcePtyVersionMarker, ([int]$revision + 2).ToString())
    [IO.File]::WriteAllText($sourcePtyBinary, "failed helper")
    $script:failActivation = $true
    $failed = $false
    try { Install-Binary } catch {
        if ($_ -notmatch "Injected helper activation failure") { throw }
        $failed = $true
    }
    Assert-True $failed "helper activation failure was not exercised"
    Assert-True ([IO.File]::ReadAllText($installedPtyVersionMarker) -eq $previousRevision) "rollback changed the helper revision"
    Assert-True ([IO.File]::ReadAllText($installedPtyBinary) -eq "newer helper") "rollback did not restore the active helper"
    Assert-True (-not $runningImage.HasExited) "rollback disturbed the running previous image"
    Write-Host "Windows helper revision tests passed."
} finally {
    if ($null -ne $runningImage -and -not $runningImage.HasExited) { $runningImage.Kill(); $runningImage.WaitForExit() }
    $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
    Assert-True ($resolvedRoot.StartsWith([IO.Path]::GetFullPath($PSScriptRoot) + [IO.Path]::DirectorySeparatorChar)) "test cleanup escaped scripts"
    if (Test-Path -LiteralPath $resolvedRoot) { Remove-Item -LiteralPath $resolvedRoot -Recurse -Force }
}
