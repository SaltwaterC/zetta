[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ConsoleBinaryPath,
    [Parameter(Mandatory = $true)]
    [string]$GuiBinaryPath,
    [Parameter(Mandatory = $true)]
    [string]$MuxBinaryPath,
    [Parameter(Mandatory = $true)]
    [string]$PtyBinaryPath
)

$ErrorActionPreference = "Stop"
function Get-PeSubsystem([string]$BinaryPath) {
    $binary = (Resolve-Path -LiteralPath $BinaryPath).Path
    $bytes = [System.IO.File]::ReadAllBytes($binary)
    if ($bytes.Length -lt 96 -or $bytes[0] -ne 0x4d -or $bytes[1] -ne 0x5a) {
        throw "$binary is not a valid PE executable"
    }

    $peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
    $subsystemOffset = $peOffset + 92
    if ($peOffset -lt 0 -or $subsystemOffset + 2 -gt $bytes.Length -or
        $bytes[$peOffset] -ne 0x50 -or $bytes[$peOffset + 1] -ne 0x45) {
        throw "$binary has an invalid PE header"
    }
    return [BitConverter]::ToUInt16($bytes, $subsystemOffset)
}

function Invoke-GuiLauncher([string]$BinaryPath, [string]$Argument) {
    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $BinaryPath
    $startInfo.Arguments = $Argument
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    try {
        $process.Start() | Out-Null
        $stdout = $process.StandardOutput.ReadToEnd()
        $stderr = $process.StandardError.ReadToEnd()
        $process.WaitForExit()
        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdout
            Stderr = $stderr
        }
    }
    finally {
        $process.Dispose()
    }
}

$consoleSubsystem = 3
$guiSubsystem = 2
$consoleBinary = (Resolve-Path -LiteralPath $ConsoleBinaryPath).Path
$guiBinary = (Resolve-Path -LiteralPath $GuiBinaryPath).Path
$muxBinary = (Resolve-Path -LiteralPath $MuxBinaryPath).Path
$ptyBinary = (Resolve-Path -LiteralPath $PtyBinaryPath).Path
$actualConsoleSubsystem = Get-PeSubsystem $consoleBinary
$actualGuiSubsystem = Get-PeSubsystem $guiBinary
$actualMuxSubsystem = Get-PeSubsystem $muxBinary
$actualPtySubsystem = Get-PeSubsystem $ptyBinary
if ($actualConsoleSubsystem -ne $consoleSubsystem) {
    throw "$consoleBinary uses PE subsystem $actualConsoleSubsystem; expected console subsystem $consoleSubsystem"
}
if ($actualGuiSubsystem -ne $guiSubsystem) {
    throw "$guiBinary uses PE subsystem $actualGuiSubsystem; expected GUI subsystem $guiSubsystem"
}
if ($actualMuxSubsystem -ne $consoleSubsystem) {
    throw "$muxBinary uses PE subsystem $actualMuxSubsystem; expected console subsystem $consoleSubsystem"
}
if ($actualPtySubsystem -ne $consoleSubsystem) {
    throw "$ptyBinary uses PE subsystem $actualPtySubsystem; expected console subsystem $consoleSubsystem"
}

$version = ((& $consoleBinary --version | Out-String).Trim() -replace "`r", "")
$versionExitCode = $LASTEXITCODE
if ($versionExitCode -ne 0 -or $version -notmatch '(?m)^Zetta \S+$' -or
    $version -notmatch '(?m)^CONTROL_VERSION=\d+$' -or
    $version -notmatch '(?m)^CATALOG_VERSION=\d+$' -or
    $version -notmatch '(?m)^ZMUX_PROTOCOL_VERSION=\d+$') {
    throw "$consoleBinary --version failed its CLI smoke test"
}
$guiVersion = Invoke-GuiLauncher $guiBinary "--version"
$guiInvalidArgument = Invoke-GuiLauncher $guiBinary "--definitely-invalid"
$guiVersionText = $guiVersion.Stdout.Trim() -replace "`r", ""
if ($guiVersion.ExitCode -ne 0 -or $guiVersionText -ne $version) {
    throw "$guiBinary failed to forward its CLI arguments and output to $consoleBinary"
}
if ($guiInvalidArgument.ExitCode -eq 0 -or
    $guiInvalidArgument.Stderr -notmatch 'unknown argument') {
    throw "$guiBinary failed to propagate errors from $consoleBinary"
}
$muxVersion = & $muxBinary --version
if ($LASTEXITCODE -ne 0 -or $muxVersion -notmatch '^zmux \S+ \(protocol \d+\)$') {
    throw "$muxBinary --version failed its CLI smoke test"
}

Write-Host "Verified Windows console executable: $consoleBinary ($version)"
Write-Host "Verified Windows GUI launcher: $guiBinary"
Write-Host "Verified Windows multiplexer executable: $muxBinary ($muxVersion)"
Write-Host "Verified Windows pseudoconsole host: $ptyBinary"
