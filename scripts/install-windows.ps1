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
    $SourceBinary = Join-Path $repositoryRoot "target\debug\zetta.exe"
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

function Get-VersionedPath([string]$Path, [string]$Version) {
    $directory = Split-Path -Parent $Path
    $fileName = [System.IO.Path]::GetFileNameWithoutExtension($Path)
    $extension = [System.IO.Path]::GetExtension($Path)
    return Join-Path $directory "$fileName.$Version$extension"
}

function Get-InstallFiles {
    $files = @(
        [pscustomobject]@{ Source = $SourceBinary; Destination = $installedBinary },
        [pscustomobject]@{ Source = $SourceGuiBinary; Destination = $installedGuiBinary }
    )
    foreach ($fileName in $runtimeFileNames) {
        $files += [pscustomobject]@{
            Source = Join-Path $sourceDirectory $fileName
            Destination = Join-Path $InstallDirectory $fileName
        }
    }
    return $files
}

function Test-InstallFilesCurrent($InstallFiles) {
    foreach ($file in $InstallFiles) {
        if (-not (Test-Path -LiteralPath $file.Destination -PathType Leaf)) {
            return $false
        }
        $sourceInfo = Get-Item -LiteralPath $file.Source
        $destinationInfo = Get-Item -LiteralPath $file.Destination
        if ($sourceInfo.Length -ne $destinationInfo.Length) {
            return $false
        }
        $sourceHash = (Get-FileHash -LiteralPath $file.Source -Algorithm SHA256).Hash
        $destinationHash = (Get-FileHash -LiteralPath $file.Destination -Algorithm SHA256).Hash
        if ($sourceHash -ne $destinationHash) {
            return $false
        }
    }
    return $true
}

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
    $installFiles = @(Get-InstallFiles)
    foreach ($file in $installFiles) {
        if (-not (Test-Path -LiteralPath $file.Source -PathType Leaf)) {
            throw "Required Windows file not found at $($file.Source). Run 'make build' first."
        }
    }

    if (Test-InstallFilesCurrent $installFiles) {
        Add-InstallDirectoryToUserPath
        Write-Host "Zetta and its Windows runtime are already current at $InstallDirectory"
        return
    }

    New-Item -ItemType Directory -Force -Path $InstallDirectory | Out-Null

    # A running Windows image cannot be overwritten, but it can be renamed.
    # Remove the previous generation before staging so a failed cleanup leaves
    # the current installation untouched.
    foreach ($file in $installFiles) {
        foreach ($version in @("new", "old")) {
            $versionedPath = Get-VersionedPath $file.Destination $version
            if (Test-Path -LiteralPath $versionedPath) {
                Remove-Item -LiteralPath $versionedPath -Force
            }
        }
    }

    try {
        foreach ($file in $installFiles) {
            $stagedPath = Get-VersionedPath $file.Destination "new"
            Copy-Item -LiteralPath $file.Source -Destination $stagedPath
        }
    } catch {
        foreach ($file in $installFiles) {
            $stagedPath = Get-VersionedPath $file.Destination "new"
            if (Test-Path -LiteralPath $stagedPath) {
                Remove-Item -LiteralPath $stagedPath -Force
            }
        }
        throw
    }

    $archivedFiles = @()
    $activatedFiles = @()
    try {
        foreach ($file in $installFiles) {
            if (Test-Path -LiteralPath $file.Destination) {
                $oldPath = Get-VersionedPath $file.Destination "old"
                Move-Item -LiteralPath $file.Destination -Destination $oldPath
                $archivedFiles += $file
            }
        }
        foreach ($file in $installFiles) {
            $stagedPath = Get-VersionedPath $file.Destination "new"
            Move-Item -LiteralPath $stagedPath -Destination $file.Destination
            $activatedFiles += $file
        }
    } catch {
        $installError = $_
        foreach ($file in $activatedFiles) {
            try {
                Remove-Item -LiteralPath $file.Destination -Force
            } catch {
                Write-Warning "Could not remove partially installed $($file.Destination): $_"
            }
        }
        foreach ($file in $archivedFiles) {
            $oldPath = Get-VersionedPath $file.Destination "old"
            if (Test-Path -LiteralPath $oldPath) {
                try {
                    Move-Item -LiteralPath $oldPath -Destination $file.Destination
                } catch {
                    Write-Warning "Could not restore $($file.Destination): $_"
                }
            }
        }
        foreach ($file in $installFiles) {
            $stagedPath = Get-VersionedPath $file.Destination "new"
            if (Test-Path -LiteralPath $stagedPath) {
                Remove-Item -LiteralPath $stagedPath -Force
            }
        }
        throw $installError
    }

    foreach ($file in $archivedFiles) {
        $oldPath = Get-VersionedPath $file.Destination "old"
        try {
            Remove-Item -LiteralPath $oldPath -Force
        } catch {
            Write-Host "Retained running previous version at $oldPath"
        }
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
    & $installedBinary --register-windows-shell $ShortcutPath
    if ($LASTEXITCODE -ne 0) {
        throw "Zetta failed to register its Windows shell integration (exit code $LASTEXITCODE)."
    }
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
    foreach ($file in @(Get-InstallFiles)) {
        foreach ($installedFile in @(
            $file.Destination,
            (Get-VersionedPath $file.Destination "new"),
            (Get-VersionedPath $file.Destination "old")
        )) {
            if (Test-Path -LiteralPath $installedFile) {
                Remove-Item -LiteralPath $installedFile -Force
                Write-Host "Removed $installedFile"
            }
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
