<#
.SYNOPSIS
Runs Microsoft's console feature tests (ft_host) against console-server.

.DESCRIPTION
Fetches microsoft/terminal at the pinned release, applies InitTests.patch,
builds the tests and Nihilist.exe with the installed Visual Studio, builds the
fthost example as their OpenConsole.exe, runs the suite with TAEF, and compares
the outcome with expected-pass.txt. Exits 1 when a test on that list does not
pass. Everything it fetches or builds lives under console-server's
target\conformance. See README.md beside this script.

.PARAMETER Backend
console-server (the default) or conpty, the inbox pseudoconsole to compare.

.PARAMETER Select
A TAEF /name: filter, such as 'OutputTests::*'. Only the selected tests are
compared.

.PARAMETER Source
An existing microsoft/terminal checkout at the pinned release to use instead
of cloning one.

.PARAMETER Update
Rewrites expected-pass.txt with the tests that passed. Full console-server
runs only.
#>
param(
    [ValidateSet('console-server', 'conpty')]
    [string]$Backend = 'console-server',
    [string]$Select = '',
    [string]$Source = '',
    [switch]$Update
)
# Cmdlets stop on errors. Native commands report through their exit codes:
# under Windows PowerShell 5.1 a global 'Stop' would also turn their redirected
# stderr into errors.
$PSDefaultParameterValues['*:ErrorAction'] = 'Stop'

$Tag = 'v1.24.12741.0'
$Commit = '7ff11ee107c984e236960ec948b450ccbc10187b'
$Packages = @(
    @{ Id = 'Microsoft.Taef'; Version = '10.93.240607003' },
    @{ Id = 'Microsoft.Windows.ImplementationLibrary'; Version = '1.0.240122.1' }
)

$Kit = $PSScriptRoot
$Crate = Split-Path $Kit -Parent
$Work = Join-Path $Crate 'target\conformance'
$Expected = Join-Path $Kit 'expected-pass.txt'
if ($Update -and ($Select -or $Backend -ne 'console-server')) {
    throw '-Update needs a full console-server run (no -Select, no -Backend conpty).'
}
New-Item -ItemType Directory -Force $Work | Out-Null

function Invoke-Checked([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE." }
}

# 1. The pinned source, patched.
if (-not $Source) {
    $Source = Join-Path $Work 'terminal'
    if (-not (Test-Path $Source)) {
        Write-Host "Cloning microsoft/terminal $Tag..."
        Invoke-Checked 'git clone' { git -c advice.detachedHead=false clone --quiet --depth 1 --branch $Tag https://github.com/microsoft/terminal.git $Source }
    }
}
$head = git -C $Source rev-parse HEAD
if ($head -ne $Commit) { throw "$Source is at $head, not $Tag ($Commit)." }
$patch = Join-Path $Kit 'InitTests.patch'
git -C $Source apply --check --reverse $patch 2>$null
if ($LASTEXITCODE -ne 0) {
    Invoke-Checked 'Applying InitTests.patch' { git -C $Source apply $patch }
}

# 2. The two NuGet packages the tests build against, not the whole solution's,
# from the checkout's own feed: TAEF is not on nuget.org.
$PackageDir = Join-Path $Source 'packages'
foreach ($package in $Packages) {
    if (-not (Test-Path (Join-Path $PackageDir "$($package.Id).$($package.Version)"))) {
        Invoke-Checked "Installing $($package.Id)" {
            & (Join-Path $Source 'dep\nuget\nuget.exe') install $package.Id -Version $package.Version -OutputDirectory $PackageDir `
                -ConfigFile (Join-Path $Source 'NuGet.Config') -NonInteractive -Verbosity quiet
        }
    }
}

# 3. The tests and their client, with whichever Visual Studio is installed.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) { throw 'Visual Studio with the C++ workload is required (vswhere.exe not found).' }
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -format json |
    Out-String | ConvertFrom-Json | Select-Object -First 1
if (-not $vs) { throw 'No Visual Studio with the C++ x64 tools (Microsoft.VisualStudio.Component.VC.Tools.x86.x64).' }
$msbuild = Join-Path $vs.installationPath 'MSBuild\Current\Bin\amd64\MSBuild.exe'
$toolset = @{ '17' = 'v143'; '18' = 'v145' }[$vs.installationVersion.Split('.')[0]]
if (-not $toolset) { throw "Visual Studio $($vs.installationVersion) is not one this script knows the toolset of." }
$sdk = Get-ChildItem (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\Include') -Directory |
    Where-Object { $_.Name -match '^10\.' } | Sort-Object { [version]$_.Name } | Select-Object -Last 1
if (-not $sdk) { throw 'No Windows 10/11 SDK found.' }
# The pinned sources build with /WX; a newer compiler's new warnings must not stop them.
$env:_CL_ = '/WX- /wd4875'
foreach ($project in 'src\tools\nihilist\Nihilist.vcxproj', 'src\host\ft_host\Host.FeatureTests.vcxproj') {
    Write-Host "Building $(Split-Path $project -Leaf) ($toolset, SDK $($sdk.Name))..."
    $log = Join-Path $Work 'msbuild.log'
    & $msbuild (Join-Path $Source $project) /nologo /m /v:minimal /p:Configuration=Release /p:Platform=x64 `
        "/p:SolutionDir=$Source\" "/p:PlatformToolset=$toolset" "/p:WindowsTargetPlatformVersion=$($sdk.Name)" > $log 2>&1
    if ($LASTEXITCODE -ne 0) {
        Get-Content $log | Select-String 'error' | Select-Object -Last 20 | ForEach-Object { Write-Host $_.Line }
        throw "Building $project failed; the full log is $log."
    }
}
$Built = Join-Path $Source 'bin\x64\Release'

# 4. The stand-in host.
Push-Location $Crate
try { Invoke-Checked 'Building fthost' { cargo build --quiet --release --example fthost } }
finally { Pop-Location }

# 5. Deploy and run. The tests toggle HKCU\Console\ForceV2 and restore it.
$Deploy = Join-Path $Work $Backend
New-Item -ItemType Directory -Force $Deploy | Out-Null
Copy-Item (Join-Path $Built 'ConHost.Feature.Tests.dll'), (Join-Path $Built 'Nihilist.exe') $Deploy -Force
Copy-Item (Join-Path $Crate 'target\release\examples\fthost.exe') (Join-Path $Deploy 'OpenConsole.exe') -Force
Set-Content (Join-Path $Deploy 'backend.txt') $Backend
Remove-Item (Join-Path $Deploy 'fthost.log') -ErrorAction SilentlyContinue
$te = Join-Path $PackageDir 'Microsoft.Taef.10.93.240607003\build\Binaries\x64\TE.exe'
$arguments = @((Join-Path $Deploy 'ConHost.Feature.Tests.dll'), '/testTimeout:0:01:00')
if ($Select) { $arguments += "/name:$Select" }
Write-Host "Running the feature tests on $Backend (a full run takes about five minutes)..."
$output = & $te @arguments 2>&1 | ForEach-Object { "$_" }
$Log = Join-Path $Work "$Backend.log"
$output | Set-Content -Encoding UTF8 $Log

# 6. Compare.
$outcomes = @{}
foreach ($line in $output) {
    if ($line -match '^EndGroup: (\S+) \[(\w+)\]') { $outcomes[$Matches[1]] = $Matches[2] }
}
if ($outcomes.Count -eq 0) { throw "TAEF ran no tests; see $Log and $(Join-Path $Deploy 'fthost.log')." }
$passed = @($outcomes.Keys | Where-Object { $outcomes[$_] -eq 'Passed' } | Sort-Object)
if ($Update) {
    $header = Get-Content $Expected | Where-Object { $_ -match '^#' }
    @($header) + $passed | Set-Content -Encoding ASCII $Expected
    Write-Host "Rewrote expected-pass.txt with $($passed.Count) tests."
    exit 0
}
$required = @(Get-Content $Expected | Where-Object { $_ -and $_ -notmatch '^#' })
if ($Select) { $required = @($required | Where-Object { $outcomes.ContainsKey($_) }) }
$regressed = @($required | Where-Object { $outcomes[$_] -ne 'Passed' })
$requiredSet = @{}
foreach ($name in $required) { $requiredSet[$name] = $true }
$gained = @($passed | Where-Object { -not $requiredSet.ContainsKey($_) })
$counts = $outcomes.Values | Group-Object | ForEach-Object { "$($_.Count) $($_.Name.ToLower())" }
Write-Host ("{0} tests: {1}." -f $outcomes.Count, ($counts -join ', '))
foreach ($name in $gained) { Write-Host "  now passes: $name" }
if ($gained) { Write-Host '  Record these with -Update.' }
foreach ($name in $regressed) {
    $outcome = $outcomes[$name]
    if (-not $outcome) { $outcome = 'not run' }
    Write-Host "  REGRESSED ($outcome): $name"
}
if ($regressed) {
    Write-Host "$($regressed.Count) expected tests did not pass; the log is $Log."
    exit 1
}
Write-Host 'Every expected test passed.'
