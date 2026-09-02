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
    [string]$SourceMuxBinary,
    [string]$SourcePtyBinary,
    [string]$SourceZwtBinary,
    [switch]$WorktreeEnabled,
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
if (-not $SourceMuxBinary) {
    $SourceMuxBinary = Join-Path (Split-Path -Parent $SourceBinary) "zmux.exe"
}
if (-not $SourcePtyBinary) {
    $SourcePtyBinary = Join-Path (Split-Path -Parent $SourceBinary) "zmux-pty.exe"
}
if ($SourceZwtBinary) {
    $WorktreeEnabled = $true
}
if ($WorktreeEnabled -and -not $SourceZwtBinary) {
    $SourceZwtBinary = Join-Path (Split-Path -Parent $SourceBinary) "zwt.exe"
}
if (-not $InstallDirectory) {
    $InstallDirectory = Join-Path $env:LOCALAPPDATA "Programs\Zetta"
}
if (-not $ShortcutPath) {
    $ShortcutPath = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Zetta.lnk"
}

$installedBinary = Join-Path $InstallDirectory "zetta.exe"
$installedGuiBinary = Join-Path $InstallDirectory "zetta-gui.exe"
$installedMuxBinary = Join-Path $InstallDirectory "zmux.exe"
$installedPtyBinary = Join-Path $InstallDirectory "zmux-pty.exe"
$installedZwtBinary = Join-Path $InstallDirectory "zwt.exe"
$runtimeFileNames = @("conpty.dll", "OpenConsole.exe")
$sourceDirectory = Split-Path -Parent $SourceBinary
$pathMarker = Join-Path $InstallDirectory ".zetta-path-managed"
$sourcePtyVersionMarker = Join-Path $repositoryRoot "resources\windows\zmux-pty.version"
$installedPtyVersionMarker = Join-Path $InstallDirectory "zmux-pty.version"
$ptyHostEndpoint = Join-Path $env:APPDATA "Zetta\sessions\zmux-host.json"
# An installation made before the marker existed contains the version-1 host.
# Keep this explicit: an unmarked helper must not be treated as compatible after
# the host protocol marker is deliberately bumped.
$legacyPtyProtocolVersion = "1"

function Get-VersionedPath([string]$Path, [string]$Version) {
    $directory = Split-Path -Parent $Path
    $fileName = [System.IO.Path]::GetFileNameWithoutExtension($Path)
    $extension = [System.IO.Path]::GetExtension($Path)
    return Join-Path $directory "$fileName.$Version$extension"
}

function Get-InstallFiles {
    $files = @(
        [pscustomobject]@{ Source = $SourceBinary; Destination = $installedBinary },
        [pscustomobject]@{ Source = $SourceGuiBinary; Destination = $installedGuiBinary },
        [pscustomobject]@{ Source = $SourceMuxBinary; Destination = $installedMuxBinary }
    )
    if ($WorktreeEnabled) {
        $files += [pscustomobject]@{ Source = $SourceZwtBinary; Destination = $installedZwtBinary }
    }
    foreach ($fileName in $runtimeFileNames) {
        $files += [pscustomobject]@{
            Source = Join-Path $sourceDirectory $fileName
            Destination = Join-Path $InstallDirectory $fileName
        }
    }
    return $files
}

function Get-PtyInstallFile {
    return [pscustomobject]@{ Source = $SourcePtyBinary; Destination = $installedPtyBinary }
}

function Remove-DisabledWorktreeFiles {
    if ($WorktreeEnabled) {
        return
    }
    foreach ($path in @(
        $installedZwtBinary,
        (Get-VersionedPath $installedZwtBinary "new"),
        (Get-VersionedPath $installedZwtBinary "old")
    )) {
        if (Test-Path -LiteralPath $path) {
            try {
                Remove-Item -LiteralPath $path -Force
                Write-Host "Removed $path"
            } catch {
                Write-Warning "Could not remove disabled worktree executable ${path}: $_"
            }
        }
    }
}

function Get-PtyProtocolVersion([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Required Windows file not found at $Path. Run 'make build' first."
    }
    $version = [string]((Get-Content -LiteralPath $Path -Raw -ErrorAction Stop).Trim())
    if ([string]::IsNullOrWhiteSpace($version) -or $version -notmatch "^[0-9]+$") {
        throw "The Windows pseudoconsole host version marker at $Path is not a decimal version."
    }
    return $version
}

function Test-PtyInstallCurrent([string]$SourceVersion) {
    if (-not (Test-Path -LiteralPath $installedPtyBinary -PathType Leaf)) {
        return $false
    }

    if (Test-Path -LiteralPath $installedPtyVersionMarker -PathType Leaf) {
        $installedVersion = [string](
            (Get-Content -LiteralPath $installedPtyVersionMarker -Raw -ErrorAction Stop).Trim()
        )
    } else {
        $installedVersion = $legacyPtyProtocolVersion
    }
    return $installedVersion -eq $SourceVersion
}

function Ensure-PtyVersionMarker([string]$SourceVersion) {
    if (Test-Path -LiteralPath $installedPtyVersionMarker -PathType Leaf) {
        return
    }
    Set-Content -LiteralPath $installedPtyVersionMarker -Value $SourceVersion -NoNewline -Encoding ASCII
    Write-Host "Recorded the Windows pseudoconsole host protocol version at $installedPtyVersionMarker"
}

function Test-LivePtyHost {
    if (-not (Test-Path -LiteralPath $ptyHostEndpoint -PathType Leaf)) {
        return $false
    }

    try {
        $endpoint = Get-Content -LiteralPath $ptyHostEndpoint -Raw -ErrorAction Stop | ConvertFrom-Json
        $processId = [uint32]$endpoint.process_id
    } catch {
        throw "Could not safely inspect the Windows pseudoconsole host endpoint at $ptyHostEndpoint. Stop Zetta and remove the stale endpoint only after confirming that no sessions are running: $_"
    }
    if ($processId -eq 0) {
        throw "The Windows pseudoconsole host endpoint at $ptyHostEndpoint has no usable process ID. Stop Zetta and remove the stale endpoint only after confirming that no sessions are running."
    }

    return $null -ne (Get-Process -Id ([int]$processId) -ErrorAction SilentlyContinue)
}

function Assert-PtyHostStopped {
    if (Test-LivePtyHost) {
        throw "Cannot replace zmux-pty.exe while a Windows pseudoconsole host is running (reported by $ptyHostEndpoint). Stop Zetta and its multiplexer sessions, then run make install again."
    }
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
    $ptyInstallFile = Get-PtyInstallFile
    foreach ($file in @($installFiles) + @($ptyInstallFile)) {
        if (-not (Test-Path -LiteralPath $file.Source -PathType Leaf)) {
            throw "Required Windows file not found at $($file.Source). Run 'make build' first."
        }
    }

    $sourcePtyVersion = Get-PtyProtocolVersion $sourcePtyVersionMarker
    $replacePty = -not (Test-PtyInstallCurrent $sourcePtyVersion)
    if ($replacePty) {
        # This check must happen before Remove-DisabledWorktreeFiles, creating
        # the install directory, or touching any staged/rollback generation.
        Assert-PtyHostStopped
    }
    Remove-DisabledWorktreeFiles

    if (-not $replacePty -and (Test-InstallFilesCurrent $installFiles)) {
        Ensure-PtyVersionMarker $sourcePtyVersion
        Add-InstallDirectoryToUserPath
        Write-Host "Zetta and its Windows runtime are already current at $InstallDirectory"
        return
    }

    New-Item -ItemType Directory -Force -Path $InstallDirectory | Out-Null
    if (-not $replacePty) {
        Ensure-PtyVersionMarker $sourcePtyVersion
    }

    $filesToInstall = @($installFiles)
    if ($replacePty) {
        $filesToInstall += $ptyInstallFile
    }

    $previousPtyVersionMarkerExists = Test-Path -LiteralPath $installedPtyVersionMarker -PathType Leaf
    $previousPtyVersionMarker = $null
    if ($replacePty -and $previousPtyVersionMarkerExists) {
        $previousPtyVersionMarker = [System.IO.File]::ReadAllBytes($installedPtyVersionMarker)
    }

    # A running Windows image cannot be overwritten, but it can be renamed.
    # Remove the previous generation before staging so a failed cleanup leaves
    # the current installation untouched.
    foreach ($file in $filesToInstall) {
        foreach ($version in @("new", "old")) {
            $versionedPath = Get-VersionedPath $file.Destination $version
            if (Test-Path -LiteralPath $versionedPath) {
                Remove-Item -LiteralPath $versionedPath -Force
            }
        }
    }

    try {
        foreach ($file in $filesToInstall) {
            $stagedPath = Get-VersionedPath $file.Destination "new"
            Copy-Item -LiteralPath $file.Source -Destination $stagedPath
        }
    } catch {
        foreach ($file in $filesToInstall) {
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
        foreach ($file in $filesToInstall) {
            if (Test-Path -LiteralPath $file.Destination) {
                $oldPath = Get-VersionedPath $file.Destination "old"
                Move-Item -LiteralPath $file.Destination -Destination $oldPath
                $archivedFiles += $file
            }
        }
        foreach ($file in $filesToInstall) {
            $stagedPath = Get-VersionedPath $file.Destination "new"
            Move-Item -LiteralPath $stagedPath -Destination $file.Destination
            $activatedFiles += $file
        }
        if ($replacePty) {
            Set-Content -LiteralPath $installedPtyVersionMarker -Value $sourcePtyVersion -NoNewline -Encoding ASCII
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
        foreach ($file in $filesToInstall) {
            $stagedPath = Get-VersionedPath $file.Destination "new"
            if (Test-Path -LiteralPath $stagedPath) {
                Remove-Item -LiteralPath $stagedPath -Force
            }
        }
        if ($replacePty) {
            try {
                if ($previousPtyVersionMarkerExists) {
                    [System.IO.File]::WriteAllBytes(
                        $installedPtyVersionMarker,
                        $previousPtyVersionMarker
                    )
                } elseif (Test-Path -LiteralPath $installedPtyVersionMarker) {
                    Remove-Item -LiteralPath $installedPtyVersionMarker -Force
                }
            } catch {
                Write-Warning "Could not restore ${installedPtyVersionMarker}: $_"
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

function Unregister-WindowsIntegration {
    if (-not (Test-Path -LiteralPath $installedBinary -PathType Leaf)) {
        return
    }
    & $installedBinary --unregister-windows-shell
    if ($LASTEXITCODE -ne 0) {
        throw "Zetta failed to unregister its Windows shell integration (exit code $LASTEXITCODE)."
    }
    Write-Host "Removed Zetta-owned Windows terminal registration"
}

function Uninstall-Binary {
    Unregister-WindowsIntegration
    Remove-InstallDirectoryFromUserPath
    $filesToRemove = @(Get-InstallFiles)
    $filesToRemove += Get-PtyInstallFile
    if (-not $WorktreeEnabled) {
        $filesToRemove += [pscustomobject]@{ Source = $null; Destination = $installedZwtBinary }
    }
    foreach ($file in $filesToRemove) {
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
    if (Test-Path -LiteralPath $installedPtyVersionMarker) {
        Remove-Item -LiteralPath $installedPtyVersionMarker -Force
        Write-Host "Removed $installedPtyVersionMarker"
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
