# Bootstraps a Zetta development environment on Windows: Git (with the Git
# Bash that make-windows.ps1 runs recipes with), GNU Make, the Visual Studio
# C++ build tools, rustup with the toolchain and targets pinned in
# rust-toolchain.toml, and the Git submodules. Anything already present is
# left alone, so it is safe to run again.
#
# Reached through scripts\init.cmd (which PowerShell and cmd resolve
# `scripts\init` to) or from Git Bash through scripts/init. It deliberately
# targets Windows PowerShell 5.1, which every supported Windows ships.

$ErrorActionPreference = 'Stop'

$usage = @'
Usage: scripts\init [OPTIONS]

Installs what a Zetta development build needs on this machine, skipping
anything already present.

Options:
  -c, --check  Report what is missing without installing anything
  -y, --yes    Accept winget package and source agreements without asking
  -h, --help   Show this help

Installs with winget: Git, GNU Make and the Visual Studio 2022 Build Tools
C++ workload. rustup is downloaded from static.rust-lang.org.
'@

$checkOnly = $false
$assumeYes = $false
foreach ($argument in $args) {
    if ($argument -in '-c', '--check') {
        $checkOnly = $true
    }
    elseif ($argument -in '-y', '--yes') {
        $assumeYes = $true
    }
    elseif ($argument -in '-h', '--help', '/?') {
        Write-Output $usage
        exit 0
    }
    else {
        [Console]::Error.WriteLine("scripts\init: unknown option: $argument")
        [Console]::Error.WriteLine($usage)
        exit 2
    }
}

if ($env:OS -ne 'Windows_NT') {
    [Console]::Error.WriteLine('scripts\init-windows.ps1 bootstraps Windows only; run scripts/init instead.')
    exit 1
}

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Set-Location -LiteralPath $repositoryRoot

# Windows PowerShell 5.1 does not offer TLS 1.2 by default.
[Net.ServicePointManager]::SecurityProtocol =
    [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$script:missing = $false
$script:installedSomething = $false

function Write-Present([string] $Message) { Write-Output "  [ok]      $Message" }
function Write-Absent([string] $Message) {
    Write-Output "  [missing] $Message"
    $script:missing = $true
}
function Write-Note([string] $Message) { Write-Output "  $Message" }
function Write-Step([string] $Message) { Write-Output ''; Write-Output $Message }

function Test-Command([string] $Name) {
    $null -ne (Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue)
}

# Native commands do not throw on a non-zero exit, even with
# ErrorActionPreference set to Stop.
function Invoke-Checked([string] $FilePath, [string[]] $Arguments) {
    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

# Output of a native command whose stderr is only noise. Windows PowerShell
# 5.1 turns any stderr line of a redirected native command into a
# terminating error under ErrorActionPreference Stop, and rustup writes
# progress and "toolchain not installed" there.
function Invoke-Quiet([string] $FilePath, [string[]] $Arguments) {
    $ErrorActionPreference = 'Continue'
    & $FilePath @Arguments 2>$null
}

# winget and installers update the registry PATH, not the PATH of a terminal
# that was already open, which is what this process inherited. Entries are
# appended rather than replacing the inherited PATH, which may carry entries
# of its own (Git Bash's, for one).
function Sync-ProcessPath {
    $entries = [Collections.Generic.List[string]]::new()
    foreach ($scope in 'Process', 'Machine', 'User') {
        $value = [Environment]::GetEnvironmentVariable('Path', $scope)
        if (-not $value) { continue }
        foreach ($entry in $value.Split(';')) {
            if ($entry -and -not $entries.Contains($entry)) { $entries.Add($entry) }
        }
    }
    $env:Path = $entries -join ';'
}

# winget's "already installed, and no newer version available" result.
$wingetAlreadyInstalled = -1978335189 # 0x8A15002B

function Install-WingetPackage([string] $Id, [string[]] $Extra = @()) {
    if (-not (Test-Command 'winget')) {
        throw "winget is required to install $Id. Install App Installer from the Microsoft Store, then run scripts\init again."
    }
    $arguments = @('install', '--exact', '--id', $Id, '--source', 'winget') + $Extra
    if ($assumeYes) {
        $arguments += @('--accept-package-agreements', '--accept-source-agreements')
    }
    & winget @arguments
    if ($LASTEXITCODE -eq $wingetAlreadyInstalled) {
        Write-Note "$Id is already installed but was not found on PATH; check its installation."
    }
    elseif ($LASTEXITCODE -ne 0) {
        throw "winget install $Id failed with exit code $LASTEXITCODE"
    }
    $script:installedSomething = $true
    Sync-ProcessPath
}

# A terminal opened before an earlier run installed something would otherwise
# report those tools missing again, and winget would fail on reinstalling them.
Sync-ProcessPath

# --- Platform and CPU architecture -------------------------------------------

# The registry holds the machine's native architecture. PROCESSOR_ARCHITECTURE
# and RuntimeInformation report the *process* architecture, which is AMD64 for
# an emulated x64 PowerShell on an ARM64 machine.
$nativeArchitecture = (Get-ItemProperty `
        -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment' `
        -Name PROCESSOR_ARCHITECTURE).PROCESSOR_ARCHITECTURE

# Zetta's Windows build scripts initialize the x64 MSVC environment
# (vcvars64.bat, Hostx64\x64), so the Rust host has to be the x64 one on every
# machine: on ARM64 that builds an x64 Zetta, which runs under emulation.
$rustHost = 'x86_64-pc-windows-msvc'
switch ($nativeArchitecture) {
    'AMD64' { $rustupInitArchitecture = 'x86_64-pc-windows-msvc' }
    'ARM64' {
        $rustupInitArchitecture = 'aarch64-pc-windows-msvc'
        Write-Warning ('This is an ARM64 machine. Zetta''s Windows build scripts use the x64 MSVC ' +
            'toolchain, so it builds an x64 Zetta that runs under emulation.')
    }
    default { throw "$nativeArchitecture Windows is not a supported development host; see docs/installation.md" }
}

$windowsVersion = [Environment]::OSVersion.Version
Write-Output "Bootstrapping a Zetta development environment for Windows $windowsVersion $nativeArchitecture"
if ($checkOnly) { Write-Output '(check only: nothing will be installed)' }

# --- Git, Git Bash and GNU Make ----------------------------------------------

function Initialize-Git {
    Write-Step 'Git and GNU Make'
    if (Test-Command 'git') {
        Write-Present 'git'
    }
    else {
        Write-Absent 'git'
        if ($checkOnly) { Write-Note 'would install Git.Git with winget' } else { Install-WingetPackage 'Git.Git' }
    }

    # make-windows.ps1 runs recipes with the bash.exe that ships beside Git.
    if (Test-Command 'git') {
        $gitExecPath = (Invoke-Quiet 'git' @('--exec-path') | Select-Object -First 1)
        if ($gitExecPath) {
            $gitRoot = Split-Path (Split-Path (Split-Path ([string] $gitExecPath).Trim() -Parent) -Parent) -Parent
            if (Test-Path -LiteralPath (Join-Path $gitRoot 'usr\bin\bash.exe')) {
                Write-Present 'Git Bash'
            }
            else {
                Write-Absent "Git Bash (no usr\bin\bash.exe under $gitRoot); install Git for Windows"
            }
        }
    }

    if (Test-Command 'make') {
        Write-Present 'make'
        return
    }
    Write-Absent 'make'
    $isAdministrator = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).
        IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if ((Test-Command 'choco') -and $isAdministrator) {
        if ($checkOnly) { Write-Note 'would run: choco install make'; return }
        $chocoArguments = @('install', 'make')
        if ($assumeYes) { $chocoArguments += '-y' }
        Invoke-Checked 'choco' $chocoArguments
        $script:installedSomething = $true
        Sync-ProcessPath
    }
    elseif ($checkOnly) {
        Write-Note 'would install ezwinports.make with winget'
    }
    else {
        # The same GNU Make 4.4 build Chocolatey's `make` package installs,
        # without needing an elevated Chocolatey.
        Install-WingetPackage 'ezwinports.make'
    }
}

# --- Visual Studio C++ build tools -------------------------------------------

function Initialize-VisualStudio {
    Write-Step 'Visual Studio C++ build tools'
    $vswhere = Join-Path ([Environment]::GetEnvironmentVariable('ProgramFiles(x86)')) `
        'Microsoft Visual Studio\Installer\vswhere.exe'
    # The same query windows-process.ps1 makes before every build.
    $withTools = $null
    $anyInstance = $null
    if (Test-Path -LiteralPath $vswhere) {
        $withTools = & $vswhere -latest -products '*' `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath |
            Select-Object -First 1
        $anyInstance = & $vswhere -latest -products '*' -property installationPath |
            Select-Object -First 1
    }
    if ($withTools) {
        Write-Present "MSVC x64 tools in $withTools"
        return
    }
    Write-Absent 'Visual Studio with the Desktop development with C++ workload'
    if ($anyInstance) {
        # Workload IDs differ between Visual Studio products, and modifying an
        # installation someone else configured is their call.
        Write-Note "Visual Studio is installed at $anyInstance without the C++ tools."
        Write-Note 'Add the "Desktop development with C++" workload in the Visual Studio Installer,'
        Write-Note 'then run scripts\init again.'
        if (-not $checkOnly) { throw 'the MSVC x64 build tools are required' }
        return
    }
    if ($checkOnly) {
        Write-Note 'would install Microsoft.VisualStudio.2022.BuildTools with the VCTools workload'
        return
    }
    # A large download; the installer asks for elevation and shows progress.
    Install-WingetPackage 'Microsoft.VisualStudio.2022.BuildTools' @(
        '--override',
        '--wait --passive --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
    )
}

# --- Rust --------------------------------------------------------------------

$toolchainFile = Get-Content -LiteralPath (Join-Path $repositoryRoot 'rust-toolchain.toml')
$toolchainChannel = ($toolchainFile | Select-String '^channel\s*=\s*"(.*)"').Matches.Groups[1].Value
$toolchainTargets = @()
$targetsLine = $toolchainFile | Select-String '^targets\s*=\s*\[(.*)\]'
if ($targetsLine) {
    $toolchainTargets = @([regex]::Matches($targetsLine.Matches.Groups[1].Value, '"([^"]+)"') |
            ForEach-Object { $_.Groups[1].Value })
}
if (-not $toolchainChannel) { throw 'rust-toolchain.toml has no channel' }

function Get-RustupDefaultHost {
    # Asked from outside the repository, so an older rustup cannot take the
    # question as a cue to install the pinned toolchain.
    Push-Location -LiteralPath $env:TEMP
    try {
        $line = Invoke-Quiet 'rustup' @('show') | Select-String '^Default host:\s*(\S+)' | Select-Object -First 1
    }
    finally {
        Pop-Location
    }
    if ($line) { return $line.Matches.Groups[1].Value }
    return $null
}

function Initialize-Rust {
    Write-Step "Rust $toolchainChannel"
    $cargoHome = $env:CARGO_HOME
    if (-not $cargoHome) { $cargoHome = Join-Path $env:USERPROFILE '.cargo' }
    $cargoBin = Join-Path $cargoHome 'bin'
    if (-not (Test-Command 'rustup') -and (Test-Path -LiteralPath (Join-Path $cargoBin 'rustup.exe'))) {
        $env:Path = "$cargoBin;$env:Path"
    }

    if (Test-Command 'rustup') {
        Write-Present 'rustup'
    }
    else {
        Write-Absent 'rustup'
        if ($checkOnly) {
            Write-Note "would install rustup ($rustupInitArchitecture) with host $rustHost"
            Write-Note "would install $toolchainChannel with: $($toolchainTargets -join ' ')"
            return
        }
        $installer = Join-Path $env:TEMP "rustup-init-$PID.exe"
        $url = "https://static.rust-lang.org/rustup/dist/$rustupInitArchitecture/rustup-init.exe"
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $installer
        try {
            # The pinned toolchain is installed below; rustup's own default
            # would only download a second one.
            Invoke-Checked $installer @('-y', '--default-toolchain', 'none', '--default-host', $rustHost)
        }
        finally {
            Remove-Item -LiteralPath $installer -Force -ErrorAction SilentlyContinue
        }
        $env:Path = "$cargoBin;$env:Path"
        $script:installedSomething = $true
    }

    # The channel in rust-toolchain.toml resolves against rustup's default
    # host, so an ARM64 default would build with a toolchain the x64 MSVC
    # environment cannot link.
    $defaultHost = Get-RustupDefaultHost
    if ($defaultHost -eq $rustHost) {
        Write-Present "default host $rustHost"
    }
    else {
        Write-Absent "default host $rustHost (currently $defaultHost)"
        if (-not $checkOnly) { Invoke-Checked 'rustup' @('set', 'default-host', $rustHost) }
    }

    $toolchains = @(& rustup toolchain list)
    if ($toolchains -match "^$([regex]::Escape("$toolchainChannel-$rustHost"))") {
        Write-Present "toolchain $toolchainChannel"
    }
    else {
        Write-Absent "toolchain $toolchainChannel"
        if (-not $checkOnly) {
            # rustup 1.28 and later install the toolchain the file names;
            # older releases install it implicitly from `rustup show`.
            & rustup toolchain install
            if ($LASTEXITCODE -ne 0) { Invoke-Checked 'rustup' @('show', 'active-toolchain') }
        }
    }

    $installedTargets = @(Invoke-Quiet 'rustup' @('target', 'list', '--installed', '--toolchain', $toolchainChannel))
    foreach ($target in $toolchainTargets) {
        if ($installedTargets -contains $target) {
            Write-Present "target $target"
        }
        else {
            Write-Absent "target $target"
            if (-not $checkOnly) {
                Invoke-Checked 'rustup' @('target', 'add', '--toolchain', $toolchainChannel, $target)
            }
        }
    }
}

# --- Git submodules ----------------------------------------------------------

function Initialize-GitSubmodule {
    Write-Step 'Git submodules'
    if (-not (Test-Command 'git')) {
        Write-Absent 'git, which is needed to inspect them'
        if (-not $checkOnly) { throw 'git is required' }
        return
    }
    # A leading '-' marks a submodule that was never initialized. A '+' (a
    # different commit checked out) is somebody's work in progress and is
    # deliberately left alone.
    $uninitialized = @()
    foreach ($line in @(& git submodule status)) {
        if ($line -match '^(.)[0-9a-f]+ (\S+)') {
            if ($Matches[1] -eq '-') {
                Write-Absent $Matches[2]
                $uninitialized += $Matches[2]
            }
            else {
                Write-Present $Matches[2]
            }
        }
    }
    if ($uninitialized.Count -gt 0 -and -not $checkOnly) {
        Invoke-Checked 'git' (@('submodule', 'update', '--init', '--') + $uninitialized)
    }
}

# --- Run ---------------------------------------------------------------------

Initialize-Git
Initialize-VisualStudio
Initialize-Rust
Initialize-GitSubmodule

Write-Output ''
if ($checkOnly) {
    if (-not $script:missing) {
        Write-Output 'The development environment is complete.'
        exit 0
    }
    Write-Output 'Run scripts\init (or ''zetta cmd init'') to install what is missing.'
    exit 1
}
Write-Output 'The development environment is ready. Build and test with: make'
if ($script:installedSomething) {
    Write-Output 'PATH changed: open a new terminal before building, so it sees the new tools.'
}
