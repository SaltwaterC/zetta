[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) {
        throw $Message
    }
}

function Assert-Equal($Expected, $Actual, [string]$Message) {
    if ($Expected -ne $Actual) {
        throw "$Message (expected '$Expected', got '$Actual')"
    }
}

function Write-TestFile([string]$Path, [string]$Contents) {
    [System.IO.File]::WriteAllBytes(
        $Path,
        [System.Text.Encoding]::ASCII.GetBytes($Contents)
    )
}

function Read-TestFile([string]$Path) {
    return [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($Path))
}

function Assert-FileContents([string]$Path, [string]$Expected, [string]$Message) {
    Assert-True (Test-Path -LiteralPath $Path -PathType Leaf) "${Message}: $Path is missing"
    Assert-Equal $Expected (Read-TestFile $Path) $Message
}

function Invoke-Installer([string]$Action = "InstallBinary") {
    $arguments = @(
        "-NoProfile",
        "-ExecutionPolicy", "Bypass",
        "-File", $installer,
        "-Action", $Action,
        "-SourceBinary", (Join-Path $sourceDirectory "zetta.exe"),
        "-SourceGuiBinary", (Join-Path $sourceDirectory "zetta-gui.exe"),
        "-SourceMuxBinary", (Join-Path $sourceDirectory "zmux.exe"),
        "-SourcePtyBinary", (Join-Path $sourceDirectory "zmux-pty.exe"),
        "-InstallDirectory", $installDirectory
    )
    $output = @(& powershell.exe @arguments 2>&1)
    return [pscustomobject]@{
        ExitCode = [int]$LASTEXITCODE
        Output = $output
    }
}

function Assert-InstallerSucceeded($Result, [string]$Message) {
    if ($Result.ExitCode -ne 0) {
        throw "$Message (exit code $($Result.ExitCode)): $($Result.Output -join [Environment]::NewLine)"
    }
}

function Set-SourceGeneration([string]$Generation) {
    Write-TestFile (Join-Path $sourceDirectory "zetta.exe") "zetta-$Generation"
    Write-TestFile (Join-Path $sourceDirectory "zetta-gui.exe") "gui-$Generation"
    Write-TestFile (Join-Path $sourceDirectory "zmux.exe") "mux-$Generation"
    Write-TestFile (Join-Path $sourceDirectory "zmux-pty.exe") "pty-$Generation"
    Write-TestFile (Join-Path $sourceDirectory "conpty.dll") "conpty-$Generation"
    Write-TestFile (Join-Path $sourceDirectory "OpenConsole.exe") "console-$Generation"
}

$installer = Join-Path $PSScriptRoot "install-windows.ps1"
$testId = [Guid]::NewGuid().ToString("N")
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) "zetta-install-windows-$PID-$testId"
$sourceDirectory = Join-Path $testRoot "source"
$installDirectory = Join-Path $testRoot "install"
$appData = Join-Path $testRoot "appdata"
$localAppData = Join-Path $testRoot "localappdata"
$installedPty = Join-Path $installDirectory "zmux-pty.exe"
$installedPtyVersion = Join-Path $installDirectory "zmux-pty.version"

$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$oldUserPath = [Environment]::GetEnvironmentVariable("Path", "User")

try {
    New-Item -ItemType Directory -Force -Path $sourceDirectory, $appData, $localAppData | Out-Null
    $env:APPDATA = $appData
    $env:LOCALAPPDATA = $localAppData
    Set-SourceGeneration "first"

    Assert-InstallerSucceeded (Invoke-Installer) "initial install failed"
    Assert-FileContents $installedPty "pty-first" "initial helper was not installed"
    Assert-FileContents $installedPtyVersion "1" "initial helper marker is wrong"

    # A rebuilt helper with the same host protocol is compatible even when its
    # bytes differ. Its current image and every generation must remain intact.
    Set-SourceGeneration "different-helper"
    $initialHelper = Read-TestFile $installedPty
    Assert-InstallerSucceeded (Invoke-Installer) "same-marker install failed"
    Assert-FileContents $installedPty $initialHelper "same-marker install replaced the helper"

    # An old installation has no sidecar yet. Version 1 is the known legacy
    # value, so the marker is backfilled without replacing its helper.
    Remove-Item -LiteralPath $installedPtyVersion -Force
    Set-SourceGeneration "migration-helper"
    Assert-InstallerSucceeded (Invoke-Installer) "marker migration failed"
    Assert-FileContents $installedPty $initialHelper "marker migration replaced the helper"
    Assert-FileContents $installedPtyVersion "1" "marker migration did not backfill the marker"

    # A marker mismatch is the deliberate replacement path.
    Set-Content -LiteralPath $installedPtyVersion -Value "0" -NoNewline -Encoding ASCII
    Set-SourceGeneration "replacement"
    Assert-InstallerSucceeded (Invoke-Installer) "marker-change install failed"
    Assert-FileContents $installedPty "pty-replacement" "marker-change install kept the old helper"
    Assert-FileContents $installedPtyVersion "1" "marker-change install wrote the wrong marker"

    # A live host must be detected before staging or changing any installed
    # file. The test process is the endpoint's live process, which makes this
    # independent of a real pseudoconsole host.
    $hostDirectory = Join-Path $appData "Zetta\sessions"
    New-Item -ItemType Directory -Force -Path $hostDirectory | Out-Null
    $hostEndpoint = Join-Path $hostDirectory "zmux-host.json"
    $endpoint = [ordered]@{
        version = 1
        protocol_version = 1
        process_id = $PID
        socket_path = Join-Path $hostDirectory "zmux-host.sock"
        token = "installer-test"
    }
    Set-Content -LiteralPath $hostEndpoint -Value ($endpoint | ConvertTo-Json -Compress) -Encoding ASCII

    $beforeRefusal = @{
        pty = Read-TestFile $installedPty
        marker = Read-TestFile $installedPtyVersion
        zetta = Read-TestFile (Join-Path $installDirectory "zetta.exe")
    }
    Set-Content -LiteralPath $installedPtyVersion -Value "0" -NoNewline -Encoding ASCII
    Set-SourceGeneration "live-host"
    $refused = Invoke-Installer
    Assert-True ($refused.ExitCode -ne 0) "live-host install unexpectedly succeeded"
    Assert-True (($refused.Output -join [Environment]::NewLine) -match "pseudoconsole host") "live-host refusal was not actionable"
    Assert-FileContents $installedPty $beforeRefusal.pty "live-host refusal changed the helper"
    Assert-FileContents $installedPtyVersion "0" "live-host refusal changed the marker"
    Assert-FileContents (Join-Path $installDirectory "zetta.exe") $beforeRefusal.zetta "live-host refusal changed an application file"
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $installDirectory "zmux-pty.new.exe"))) "live-host refusal left a staged helper"
    Remove-Item -LiteralPath $hostEndpoint -Force

    # A locked stale helper generation must not be touched by an ordinary
    # install while the active helper's marker remains compatible.
    Set-Content -LiteralPath $installedPtyVersion -Value "1" -NoNewline -Encoding ASCII
    Set-SourceGeneration "locked-generation"
    $staleOld = Join-Path $installDirectory "zmux-pty.old.exe"
    $staleNew = Join-Path $installDirectory "zmux-pty.new.exe"
    Write-TestFile $staleOld "locked-old-helper"
    Write-TestFile $staleNew "locked-new-helper"
    $staleOldLock = $null
    $staleNewLock = $null
    try {
        $staleOldLock = [System.IO.File]::Open(
            $staleOld,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::None
        )
        $staleNewLock = [System.IO.File]::Open(
            $staleNew,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::None
        )
        Set-SourceGeneration "ordinary-change"
        Assert-InstallerSucceeded (Invoke-Installer) "locked-generation install failed"
        Assert-FileContents $installedPty "pty-replacement" "ordinary install replaced a compatible helper"
        Assert-FileContents $staleOld "locked-old-helper" "ordinary install touched a locked .old helper"
        Assert-FileContents $staleNew "locked-new-helper" "ordinary install touched a locked .new helper"
        Assert-FileContents (Join-Path $installDirectory "zetta.exe") "zetta-ordinary-change" "ordinary install did not update the application"
    } finally {
        if ($null -ne $staleOldLock) {
            $staleOldLock.Dispose()
        }
        if ($null -ne $staleNewLock) {
            $staleNewLock.Dispose()
        }
    }

    # The helper marker is installer state and is removed even though it is not
    # part of the hash-checked application file list.
    Remove-Item -LiteralPath (Join-Path $installDirectory "zetta.exe") -Force
    Assert-InstallerSucceeded (Invoke-Installer "UninstallBinary") "uninstall failed"
    Assert-True (-not (Test-Path -LiteralPath $installedPtyVersion)) "uninstall left the helper marker"
    Write-Host "Windows installer tests passed."
} finally {
    if ($null -eq $oldAppData) {
        Remove-Item Env:APPDATA -ErrorAction SilentlyContinue
    } else {
        $env:APPDATA = $oldAppData
    }
    if ($null -eq $oldLocalAppData) {
        Remove-Item Env:LOCALAPPDATA -ErrorAction SilentlyContinue
    } else {
        $env:LOCALAPPDATA = $oldLocalAppData
    }
    [Environment]::SetEnvironmentVariable("Path", $oldUserPath, "User")
    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
