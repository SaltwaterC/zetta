[CmdletBinding()]
param(
    [ValidateSet(
        "Install",
        "InstallBinary",
        "InstallShortcut",
        "Uninstall",
        "UninstallBinary",
        "UninstallShortcut"
    )]
    [string]$Action = "Install",
    [string]$SourceBinary,
    [string]$SourceGuiBinary,
    [string]$InstallDirectory,
    [string]$ShortcutPath
)

$ErrorActionPreference = "Stop"

if (-not $env:LOCALAPPDATA) {
    throw "LOCALAPPDATA is not set"
}
if (-not $env:APPDATA) {
    throw "APPDATA is not set"
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
if (-not $SourceBinary) {
    $SourceBinary = Join-Path $repositoryRoot "target\release\zetta.exe"
}
if (-not $SourceGuiBinary) {
    $SourceGuiBinary = Join-Path (Split-Path -Parent $SourceBinary) "zetta-gui.exe"
}
if (-not $InstallDirectory) {
    $InstallDirectory = Join-Path $env:LOCALAPPDATA "Programs\Zetta"
}
if (-not $ShortcutPath) {
    $ShortcutPath = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Zetta.lnk"
}

$installedBinary = Join-Path $InstallDirectory "zetta.exe"
$installedGuiBinary = Join-Path $InstallDirectory "zetta-gui.exe"
$runtimeFileNames = @("conpty.dll", "OpenConsole.exe")
$sourceDirectory = Split-Path -Parent $SourceBinary
$pathMarker = Join-Path $InstallDirectory ".zetta-path-managed"

function Normalize-PathEntry([string]$PathEntry) {
    return [System.IO.Path]::GetFullPath($PathEntry).TrimEnd([char[]]@('\', '/'))
}

function Add-InstallDirectoryToUserPath {
    $normalizedInstallDirectory = Normalize-PathEntry $InstallDirectory
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @($userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $alreadyPresent = $entries | Where-Object {
        (Normalize-PathEntry $_).Equals(
            $normalizedInstallDirectory,
            [StringComparison]::OrdinalIgnoreCase
        )
    }
    if ($alreadyPresent) {
        return
    }

    $newUserPath = (@($entries) + $normalizedInstallDirectory) -join ';'
    [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
    Set-Content -LiteralPath $pathMarker -Value "Managed by the Zetta installer." -NoNewline
    Write-Host "Added $normalizedInstallDirectory to the user PATH (open a new console to use it)"
}

function Remove-InstallDirectoryFromUserPath {
    if (-not (Test-Path -LiteralPath $pathMarker -PathType Leaf)) {
        return
    }
    $normalizedInstallDirectory = Normalize-PathEntry $InstallDirectory
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @($userPath -split ';' | Where-Object {
        -not [string]::IsNullOrWhiteSpace($_) -and
        -not (Normalize-PathEntry $_).Equals(
            $normalizedInstallDirectory,
            [StringComparison]::OrdinalIgnoreCase
        )
    })
    [Environment]::SetEnvironmentVariable("Path", ($entries -join ';'), "User")
    Remove-Item -LiteralPath $pathMarker -Force
    Write-Host "Removed $normalizedInstallDirectory from the user PATH"
}

function Install-Binary {
    foreach ($applicationBinary in @($SourceBinary, $SourceGuiBinary)) {
        if (-not (Test-Path -LiteralPath $applicationBinary -PathType Leaf)) {
            throw "Release binary not found at $applicationBinary. Run 'make build' first."
        }
    }
    foreach ($fileName in $runtimeFileNames) {
        $source = Join-Path $sourceDirectory $fileName
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
            throw "Required Windows runtime not found at $source. Run 'make build' first."
        }
    }

    New-Item -ItemType Directory -Force -Path $InstallDirectory | Out-Null
    Copy-Item -LiteralPath $SourceBinary -Destination $installedBinary -Force
    Copy-Item -LiteralPath $SourceGuiBinary -Destination $installedGuiBinary -Force
    foreach ($fileName in $runtimeFileNames) {
        Copy-Item -LiteralPath (Join-Path $sourceDirectory $fileName) `
            -Destination (Join-Path $InstallDirectory $fileName) -Force
    }
    Add-InstallDirectoryToUserPath
    Write-Host "Installed Zetta and its Windows runtime to $InstallDirectory"
}

function Install-Shortcut {
    if (-not (Test-Path -LiteralPath $installedGuiBinary -PathType Leaf)) {
        throw "Installed GUI launcher not found at $installedGuiBinary. Install the binaries first."
    }

    $shortcutDirectory = Split-Path -Parent $ShortcutPath
    New-Item -ItemType Directory -Force -Path $shortcutDirectory | Out-Null

    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($ShortcutPath)
    $shortcut.TargetPath = $installedGuiBinary
    $shortcut.WorkingDirectory = $env:USERPROFILE
    $shortcut.IconLocation = "$installedGuiBinary,0"
    $shortcut.Description = "Zetta terminal emulator"
    $shortcut.Save()
    Write-Host "Created Start Menu shortcut at $ShortcutPath"
}

function Uninstall-Shortcut {
    if (Test-Path -LiteralPath $ShortcutPath) {
        Remove-Item -LiteralPath $ShortcutPath -Force
        Write-Host "Removed Start Menu shortcut at $ShortcutPath"
    }
}

function Uninstall-Binary {
    Remove-InstallDirectoryFromUserPath
    foreach ($fileName in @("zetta.exe", "zetta-gui.exe") + $runtimeFileNames) {
        $installedFile = Join-Path $InstallDirectory $fileName
        if (Test-Path -LiteralPath $installedFile) {
            Remove-Item -LiteralPath $installedFile -Force
            Write-Host "Removed $installedFile"
        }
    }
    if ((Test-Path -LiteralPath $InstallDirectory -PathType Container) -and
        -not (Get-ChildItem -LiteralPath $InstallDirectory -Force | Select-Object -First 1)) {
        Remove-Item -LiteralPath $InstallDirectory -Force
    }
}

switch ($Action) {
    "Install" {
        Install-Binary
        Install-Shortcut
    }
    "InstallBinary" { Install-Binary }
    "InstallShortcut" { Install-Shortcut }
    "Uninstall" {
        Uninstall-Shortcut
        Uninstall-Binary
    }
    "UninstallBinary" { Uninstall-Binary }
    "UninstallShortcut" { Uninstall-Shortcut }
}
