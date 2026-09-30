# Exercise the real file transaction with fixtures, without changing user PATH,
# registering shell integration, or touching the installed application.
[CmdletBinding()]
param([string]$Installer)

$ErrorActionPreference = "Stop"
if (-not $Installer) { $Installer = Join-Path $PSScriptRoot "install-windows.ps1" }
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($installer, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw "Installer parse failed: $parseErrors" }
$names = @("Get-VersionedPath", "Get-RollbackPaths", "Get-AvailableRollbackPath", "Test-InstallFilesCurrent", "Install-Binary")
foreach ($definition in $ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false)) {
    if ($definition.Name -in $names) {
        . ([scriptblock]::Create($definition.Extent.Text))
    }
}

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function Get-InstallFiles { return $fixtureFiles }
function Add-InstallDirectoryToUserPath {}
function Remove-DisabledWorktreeFiles {}
function Remove-DisabledNotifyFiles {}
function Remove-DisabledClipboardFiles {}
function Remove-DisabledMuxFiles {}
function Remove-DisabledZoshFiles {}
function Remove-DisabledZoshServerFiles {}
function Remove-LegacyMoshServerFiles {}

# Inject one activation failure, then allow the actual rollback moves.
function Move-Item([string]$LiteralPath, [string]$Destination) {
    if ($script:failActivation -and $LiteralPath -eq (Get-VersionedPath $console "new")) {
        $script:failActivation = $false
        throw "Injected activation failure"
    }
    Microsoft.PowerShell.Management\Move-Item -LiteralPath $LiteralPath -Destination $Destination
}

$testRoot = Join-Path $PSScriptRoot (".installer-test-" + [Guid]::NewGuid().ToString("N"))
$InstallDirectory = Join-Path $testRoot "install"
$sourceDirectory = Join-Path $testRoot "source"
$muxEnabled = $false
$installedPtyVersionMarker = Join-Path $InstallDirectory "zmux-pty.version"
$console = Join-Path $InstallDirectory "OpenConsole.exe"
$application = Join-Path $InstallDirectory "zetta.exe"
$fixtureFiles = @(
    [pscustomobject]@{ Source = Join-Path $sourceDirectory "zetta.exe"; Destination = $application },
    [pscustomobject]@{ Source = Join-Path $sourceDirectory "OpenConsole.exe"; Destination = $console }
)
$locks = @()
$script:failActivation = $false
try {
    New-Item -ItemType Directory -Path $InstallDirectory, $sourceDirectory | Out-Null
    foreach ($file in $fixtureFiles) {
        [IO.File]::WriteAllText($file.Source, "first")
        [IO.File]::WriteAllText($file.Destination, "previous")
    }
    $oldConsole = Get-VersionedPath $console "old"
    [IO.File]::WriteAllText($oldConsole, "older-running")
    $locks += [IO.File]::Open($oldConsole, 'Open', 'Read', 'Read')
    # Delete sharing permits rename, as for a running image. After rename the
    # transaction chooses a distinct archive even while the older one is locked.
    $locks += [IO.File]::Open($console, 'Open', 'Read', 'ReadWrite, Delete')
    Install-Binary
    Assert-True ([IO.File]::ReadAllText($console) -eq "first") "locked old runtime blocked installation"
    Assert-True ([IO.File]::ReadAllText($oldConsole) -eq "older-running") "locked backup changed"

    # Hold another generation without delete sharing across repeated upgrades.
    $extraBackup = Get-VersionedPath $console ("old." + [Guid]::NewGuid().ToString("N"))
    [IO.File]::WriteAllText($extraBackup, "another-running")
    $locks += [IO.File]::Open($extraBackup, 'Open', 'Read', 'Read')
    $unrelated = Get-VersionedPath $console "old.notes"
    [IO.File]::WriteAllText($unrelated, "unrelated")
    foreach ($file in $fixtureFiles) { [IO.File]::WriteAllText($file.Source, "second") }
    Install-Binary
    Assert-True ([IO.File]::ReadAllText($console) -eq "second") "repeated upgrade failed"
    Assert-True ([IO.File]::ReadAllText($extraBackup) -eq "another-running") "locked unique backup changed"

    foreach ($file in $fixtureFiles) { [IO.File]::WriteAllText($file.Source, "third") }
    $script:failActivation = $true
    $failed = $false
    try { Install-Binary } catch {
        if ($_ -notmatch "Injected activation failure") { throw }
        $failed = $true
    }
    Assert-True $failed "activation failure was not exercised"
    foreach ($file in $fixtureFiles) {
        Assert-True ([IO.File]::ReadAllText($file.Destination) -eq "second") "rollback did not restore $($file.Destination)"
        Assert-True (-not (Test-Path -LiteralPath (Get-VersionedPath $file.Destination "new"))) "rollback left staging files"
    }
    foreach ($lock in $locks) { $lock.Dispose() }
    $locks = @()
    Install-Binary
    Assert-True (@(Get-RollbackPaths $console).Count -eq 0) "released backups were not cleaned"
    Assert-True ([IO.File]::ReadAllText($unrelated) -eq "unrelated") "cleanup removed an unrelated file"
    Write-Host "Windows installer generation tests passed."
} finally {
    foreach ($lock in $locks) { $lock.Dispose() }
    # This absolute directory was created by this test inside the scripts folder.
    $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
    Assert-True ($resolvedRoot.StartsWith([IO.Path]::GetFullPath($PSScriptRoot) + [IO.Path]::DirectorySeparatorChar)) "test cleanup escaped scripts"
    if (Test-Path -LiteralPath $resolvedRoot) { Remove-Item -LiteralPath $resolvedRoot -Recurse -Force }
}
