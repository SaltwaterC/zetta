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
if (-not $InstallDirectory) {
    $InstallDirectory = Join-Path $env:LOCALAPPDATA "Programs\Zetta"
}
if (-not $ShortcutPath) {
    $ShortcutPath = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Zetta.lnk"
}

$installedBinary = Join-Path $InstallDirectory "zetta.exe"

function Install-Binary {
    if (-not (Test-Path -LiteralPath $SourceBinary -PathType Leaf)) {
        throw "Release binary not found at $SourceBinary. Run 'make build' first."
    }

    New-Item -ItemType Directory -Force -Path $InstallDirectory | Out-Null
    Copy-Item -LiteralPath $SourceBinary -Destination $installedBinary -Force
    Write-Host "Installed Zetta to $installedBinary"
}

function Install-Shortcut {
    if (-not (Test-Path -LiteralPath $installedBinary -PathType Leaf)) {
        throw "Installed binary not found at $installedBinary. Install the binary first."
    }

    $shortcutDirectory = Split-Path -Parent $ShortcutPath
    New-Item -ItemType Directory -Force -Path $shortcutDirectory | Out-Null

    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($ShortcutPath)
    $shortcut.TargetPath = $installedBinary
    $shortcut.WorkingDirectory = $env:USERPROFILE
    $shortcut.IconLocation = "$installedBinary,0"
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
    if (Test-Path -LiteralPath $installedBinary) {
        Remove-Item -LiteralPath $installedBinary -Force
        Write-Host "Removed $installedBinary"
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
