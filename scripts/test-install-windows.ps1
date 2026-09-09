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

function Get-UserPathEntries {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ([string]::IsNullOrWhiteSpace($userPath)) {
        return @()
    }
    return @($userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
}

function Assert-UserPathEntries([string[]]$Expected, [string]$Message) {
    $actual = @(Get-UserPathEntries)
    Assert-Equal $Expected.Count $actual.Count $Message
    for ($index = 0; $index -lt $Expected.Count; $index++) {
        Assert-Equal $Expected[$index] $actual[$index] "$Message (entry $index)"
    }
}

function Assert-UserPathContainsOnce([string]$ExpectedEntry, [string]$Message) {
    $matches = @(Get-UserPathEntries | Where-Object {
        $_.Equals($ExpectedEntry, [StringComparison]::OrdinalIgnoreCase)
    })
    Assert-Equal 1 $matches.Count $Message
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
        "-SourceZoshBinary", (Join-Path $sourceDirectory "zosh.exe"),
        "-SourceZoshServerBinary", (Join-Path $sourceDirectory "zosh-server.exe"),
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
    Write-TestFile (Join-Path $sourceDirectory "zosh.exe") "zosh-$Generation"
    Write-TestFile (Join-Path $sourceDirectory "zosh-server.exe") "zosh-server-$Generation"
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
$unrelatedUserPathEntries = @(
    (Join-Path $testRoot "unrelated-first"),
    (Join-Path $testRoot "unrelated-second")
)

try {
    New-Item -ItemType Directory -Force -Path $sourceDirectory, $appData, $localAppData | Out-Null
    $env:APPDATA = $appData
    $env:LOCALAPPDATA = $localAppData
    [Environment]::SetEnvironmentVariable(
        "Path",
        ($unrelatedUserPathEntries -join ';'),
        "User"
    )
    Set-SourceGeneration "first"
    New-Item -ItemType Directory -Force -Path $installDirectory | Out-Null
    Write-TestFile (Join-Path $installDirectory "mosh-server.exe") "legacy-mosh-server"

    Assert-InstallerSucceeded (Invoke-Installer) "initial install failed"
    Assert-FileContents $installedPty "pty-first" "initial helper was not installed"
    Assert-FileContents (Join-Path $installDirectory "zosh.exe") "zosh-first" "zosh was not installed"
    Assert-FileContents (Join-Path $installDirectory "zosh-server.exe") "zosh-server-first" "zosh-server was not installed"
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $installDirectory "mosh-server.exe"))) "initial install left legacy mosh-server"
    Assert-FileContents $installedPtyVersion "1" "initial helper marker is wrong"
    Assert-UserPathEntries ($unrelatedUserPathEntries + $installDirectory) "initial install disturbed or omitted user PATH entries"
    Assert-UserPathContainsOnce $installDirectory "initial install did not add its executable directory to the user PATH"

    # A rebuilt helper with the same host protocol is compatible even when its
    # bytes differ. Its current image and every generation must remain intact.
    Set-SourceGeneration "different-helper"
    $initialHelper = Read-TestFile $installedPty
    Assert-InstallerSucceeded (Invoke-Installer) "same-marker install failed"
    Assert-FileContents $installedPty $initialHelper "same-marker install replaced the helper"
    Assert-UserPathEntries ($unrelatedUserPathEntries + $installDirectory) "same-marker install changed user PATH entries"

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
    Assert-UserPathEntries $unrelatedUserPathEntries "uninstall removed or changed unrelated user PATH entries"
    Assert-True (-not (Get-UserPathEntries | Where-Object {
        $_.Equals($installDirectory, [StringComparison]::OrdinalIgnoreCase)
    })) "uninstall left the installed directory in the user PATH"
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $installDirectory "zosh.exe"))) "uninstall left zosh"
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $installDirectory "zosh-server.exe"))) "uninstall left zosh-server"
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $installDirectory "mosh-server.exe"))) "uninstall left legacy mosh-server"
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
