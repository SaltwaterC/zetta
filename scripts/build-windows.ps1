$ErrorActionPreference = 'Stop'
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Set-Location -LiteralPath $repositoryRoot
. (Join-Path $PSScriptRoot 'windows-process.ps1')

Import-VisualStudioEnvironment
Repair-MsvcLinkerPath

function Set-DefaultEnvironmentVariable([string] $Name, [string] $Value) {
    if ($null -eq [Environment]::GetEnvironmentVariable($Name, 'Process')) {
        [Environment]::SetEnvironmentVariable($Name, $Value, 'Process')
    }
}

function Test-FeatureEnabled([string] $Value) {
    return $Value -notin @('0', 'false', 'no', 'off')
}

function Invoke-CargoBuild([string[]] $Arguments) {
    $displayArguments = $Arguments | ForEach-Object {
        if ([string] $_ -match '\s') { '"{0}"' -f $_ } else { [string] $_ }
    }
    Write-Output ("  [RUNNING] Windows Cargo: {0} {1}" -f $cargo, ($displayArguments -join ' '))
    $exitCode = Invoke-WindowsProcess `
        -FilePath $cargo `
        -Arguments $Arguments `
        -WorkingDirectory $repositoryRoot
    if ($exitCode -ne 0) {
        exit $exitCode
    }
    Write-Output '  [DONE] Windows Cargo command'
}

function Invoke-StandaloneCargoBuild([string] $ManifestPath, [string[]] $BinaryNames) {
    $buildArguments = @('build')
    if ($targetProfile -eq 'release') {
        $buildArguments += '--release'
    }
    $buildArguments += @(
        '--jobs', $buildJobs, '--locked', '--manifest-path', $ManifestPath,
        '--target-dir', 'target'
    )
    foreach ($binaryName in $BinaryNames) {
        $buildArguments += @('--bin', $binaryName)
    }
    Invoke-CargoBuild $buildArguments
}

$defaults = @{
    SERIAL = '1'
    HTTP = '1'
    TFTP = '1'
    CLIPBOARD = '1'
    NOTIFY = '1'
    SYNTAX_HIGHLIGHTING = '1'
    ZMUX = '1'
    SESSION_PERSISTENCE = '1'
    WORKTREE = '1'
    ZDIFF = '1'
    ZOSH = '1'
}
foreach ($entry in $defaults.GetEnumerator()) {
    Set-DefaultEnvironmentVariable $entry.Key $entry.Value
}
Set-DefaultEnvironmentVariable 'TFTP_SERVER' $env:TFTP
Set-DefaultEnvironmentVariable 'TFTP_CLIENT' $env:TFTP
Set-DefaultEnvironmentVariable 'ZOSH_CLIENT' $env:ZOSH
Set-DefaultEnvironmentVariable 'ZOSH_SERVER' $env:ZOSH

$cargo = $env:CARGO
if ([string]::IsNullOrWhiteSpace($cargo)) {
    $cargo = 'cargo'
}
$buildJobs = $env:CARGO_BUILD_JOBS
if ([string]::IsNullOrWhiteSpace($buildJobs)) {
    $buildJobs = [Environment]::ProcessorCount.ToString()
}

$features = [Collections.Generic.List[string]]::new()
$features.Add('windows-gui')
foreach ($feature in @(
    @{ Variable = 'SERIAL'; Name = 'serial-console' },
    @{ Variable = 'HTTP'; Name = 'http-server' },
    @{ Variable = 'TFTP_SERVER'; Name = 'tftp-server' },
    @{ Variable = 'TFTP_CLIENT'; Name = 'tftp-client' },
    @{ Variable = 'CLIPBOARD'; Name = 'clipboard' },
    @{ Variable = 'NOTIFY'; Name = 'notifications' },
    @{ Variable = 'SYNTAX_HIGHLIGHTING'; Name = 'syntax-highlighting' },
    @{ Variable = 'ZMUX'; Name = 'zmux' },
    @{ Variable = 'WORKTREE'; Name = 'worktree' },
    @{ Variable = 'ZDIFF'; Name = 'zdiff' },
    @{ Variable = 'ZOSH_CLIENT'; Name = 'zosh-client' },
    @{ Variable = 'ZOSH_SERVER'; Name = 'zosh-server' }
)) {
    if (Test-FeatureEnabled ([Environment]::GetEnvironmentVariable($feature.Variable, 'Process'))) {
        $features.Add($feature.Name)
        if ($feature.Variable -eq 'ZMUX' -and (Test-FeatureEnabled $env:SESSION_PERSISTENCE)) {
            $features.Add('session-persistence')
        }
    }
}

$binaries = [Collections.Generic.List[string]]::new()
$binaries.Add('zetta')
$binaries.Add('zetta-gui')
$binaries.Add('wslx')
$verificationArguments = @()
$targetProfile = 'debug'
if ($args -contains '--release') {
    $targetProfile = 'release'
}
$targetDirectory = Join-Path $repositoryRoot (Join-Path 'target' $targetProfile)

if (Test-FeatureEnabled $env:ZMUX) {
    $binaries.Add('zmux')
    $binaries.Add('zmux-pty')
    $verificationArguments += @(
        '-MuxBinaryPath', (Join-Path $targetDirectory 'zmux.exe'),
        '-PtyBinaryPath', (Join-Path $targetDirectory 'zmux-pty.exe')
    )
}
if (Test-FeatureEnabled $env:ZOSH_CLIENT) {
    $binaries.Add('zosh')
    $verificationArguments += @('-ZoshBinaryPath', (Join-Path $targetDirectory 'zosh.exe'))
}
if (Test-FeatureEnabled $env:ZOSH_SERVER) {
    $binaries.Add('zosh-server')
    $verificationArguments += @('-ZoshServerBinaryPath', (Join-Path $targetDirectory 'zosh-server.exe'))
}
if (Test-FeatureEnabled $env:WORKTREE) {
    $binaries.Add('zwt')
    $verificationArguments += @('-WorktreeBinaryPath', (Join-Path $targetDirectory 'zwt.exe'))
}
if (Test-FeatureEnabled $env:ZDIFF) {
    $binaries.Add('zpager')
    $verificationArguments += @('-PagerBinaryPath', (Join-Path $targetDirectory 'zpager.exe'))
}
if (Test-FeatureEnabled $env:NOTIFY) {
    $verificationArguments += @('-NotifyBinaryPath', (Join-Path $targetDirectory 'zntfy.exe'))
}
if (Test-FeatureEnabled $env:CLIPBOARD) {
    $verificationArguments += @(
        '-CopyBinaryPath', (Join-Path $targetDirectory 'zcopy.exe'),
        '-PasteBinaryPath', (Join-Path $targetDirectory 'zpaste.exe')
    )
}
if (Test-FeatureEnabled $env:HTTP) {
    $verificationArguments += @('-HttpBinaryPath', (Join-Path $targetDirectory 'zhttp.exe'))
}
if ((Test-FeatureEnabled $env:TFTP_SERVER) -or (Test-FeatureEnabled $env:TFTP_CLIENT)) {
    $verificationArguments += @('-TftpBinaryPath', (Join-Path $targetDirectory 'ztftp.exe'))
}
$verificationArguments += @('-WslxBinaryPath', (Join-Path $targetDirectory 'wslx.exe'))

$rootBuildArguments = @('build')
if ($targetProfile -eq 'release') {
    $rootBuildArguments += '--release'
}
$rootBuildArguments += @('--jobs', $buildJobs, '--locked', '--no-default-features', '--features', ($features -join ','))
foreach ($binary in $binaries) {
    $rootBuildArguments += @('--bin', $binary)
}
Invoke-CargoBuild $rootBuildArguments

if (Test-FeatureEnabled $env:NOTIFY) {
    Invoke-StandaloneCargoBuild 'crates/zntfy/Cargo.toml' @('zntfy')
}
if (Test-FeatureEnabled $env:CLIPBOARD) {
    Invoke-StandaloneCargoBuild 'crates/zclip/Cargo.toml' @('zcopy', 'zpaste')
}
if (Test-FeatureEnabled $env:HTTP) {
    Invoke-StandaloneCargoBuild 'crates/zhttp/Cargo.toml' @('zhttp')
}
if ((Test-FeatureEnabled $env:TFTP_SERVER) -or (Test-FeatureEnabled $env:TFTP_CLIENT)) {
    Invoke-StandaloneCargoBuild 'crates/ztftp/Cargo.toml' @('ztftp')
}

$verificationScript = Join-Path $PSScriptRoot 'verify-windows-binary.ps1'
Write-Output '  [RUNNING] Windows binary verification'
$verificationExitCode = Invoke-WindowsProcess `
    -FilePath 'powershell.exe' `
    -Arguments (@(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $verificationScript,
        '-ConsoleBinaryPath', (Join-Path $targetDirectory 'zetta.exe'),
        '-GuiBinaryPath', (Join-Path $targetDirectory 'zetta-gui.exe')
    ) + $verificationArguments) `
    -WorkingDirectory $repositoryRoot
if ($verificationExitCode -eq 0) {
    Write-Output '  [DONE] Windows binary verification'
}
exit $verificationExitCode
