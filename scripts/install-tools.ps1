[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SourceDirectory,
    [Parameter(Mandatory = $true)]
    [string[]]$Binaries,
    [string]$InstallDirectory
)

# Copies standalone tool executables (`make install-tools`) into
# InstallDirectory, which defaults to the directory `make install` uses so the
# two do not scatter Zetta's executables across the machine.

$ErrorActionPreference = "Stop"

if (-not $InstallDirectory) {
    if (-not $env:LOCALAPPDATA) {
        throw "LOCALAPPDATA is not set"
    }
    $InstallDirectory = Join-Path $env:LOCALAPPDATA "Programs\Zetta"
}

New-Item -ItemType Directory -Force -Path $InstallDirectory | Out-Null
foreach ($binary in $Binaries) {
    $source = Join-Path $SourceDirectory $binary
    if (-not (Test-Path -LiteralPath $source)) {
        throw "$source is missing; run 'make build-tools' first"
    }
    Copy-Item -LiteralPath $source -Destination (Join-Path $InstallDirectory $binary) -Force
    Write-Host "Installed $binary to $InstallDirectory"
}
