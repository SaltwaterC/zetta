$ErrorActionPreference = "Stop"

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
Set-Location -LiteralPath $repositoryRoot

# Native build scripts track MSVC environment variables. Import the same
# environment used by build-windows.cmd before lint and test so Cargo does not
# alternate between incompatible fingerprints on successive stop-hook runs.
function Set-VisualStudioLinker {
    $vcToolsInstallPath = [Environment]::GetEnvironmentVariable("VCToolsInstallDir")
    if ([string]::IsNullOrWhiteSpace($vcToolsInstallPath)) {
        throw "The Visual Studio C++ tools environment was not initialized."
    }

    $linkerPath = Join-Path $vcToolsInstallPath "bin\HostX64\x64\link.exe"
    if (-not (Test-Path -LiteralPath $linkerPath)) {
        throw "The Visual Studio x64 linker was not found at $linkerPath."
    }

    # GNU Make selects a POSIX shell on Windows, whose PATH can put Git or
    # Coreutils' link.exe ahead of MSVC's linker. Give Cargo an absolute path.
    [Environment]::SetEnvironmentVariable(
        "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER",
        $linkerPath,
        "Process"
    )
}

function Import-VisualStudioEnvironment {
    if (-not [string]::IsNullOrWhiteSpace($env:VSCMD_VER)) {
        Set-VisualStudioLinker
        return
    }

    $vswherePath = Join-Path `
        ([Environment]::GetEnvironmentVariable("ProgramFiles(x86)")) `
        "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswherePath)) {
        throw "Visual Studio Installer's vswhere.exe was not found. Install the Visual Studio Desktop development with C++ workload."
    }

    $vsInstallPaths = & $vswherePath `
        -latest `
        -products "*" `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath
    $vswhereExitCode = $LASTEXITCODE
    $vsInstallPath = $vsInstallPaths | Select-Object -First 1
    if ($vswhereExitCode -ne 0 -or [string]::IsNullOrWhiteSpace($vsInstallPath)) {
        throw "A Visual Studio installation with the C++ build tools was not found."
    }

    $vcvarsPath = Join-Path ([string] $vsInstallPath).Trim() "VC\Auxiliary\Build\vcvars64.bat"
    if (-not (Test-Path -LiteralPath $vcvarsPath)) {
        throw "The Visual Studio x64 environment script was not found at $vcvarsPath."
    }

    $environmentLines = & cmd.exe /d /c ('call "{0}" >nul 2>&1 && set' -f $vcvarsPath)
    if ($LASTEXITCODE -ne 0) {
        throw "Visual Studio's vcvars64.bat failed to initialize the x64 build environment."
    }

    $environment = @{}
    $path = $null
    foreach ($line in $environmentLines) {
        $separator = $line.IndexOf("=")
        if ($separator -le 0) {
            continue
        }

        $name = $line.Substring(0, $separator)
        $value = $line.Substring($separator + 1)
        if ($name -ieq "PATH") {
            if ($name.StartsWith("PATH", [StringComparison]::Ordinal)) {
                $path = $value
            }
            continue
        }

        if (-not $environment.ContainsKey($name)) {
            $environment[$name] = $value
        }
    }

    foreach ($entry in $environment.GetEnumerator()) {
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, "Process")
    }
    if ($null -ne $path) {
        [Environment]::SetEnvironmentVariable("PATH", $path, "Process")
    }

    Set-VisualStudioLinker
}

if ($env:OS -eq "Windows_NT") {
    Import-VisualStudioEnvironment
}

function Invoke-MakeTarget {
    param(
        [Parameter(Mandatory)]
        [string] $Target
    )

    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & make $Target 2>&1 | ForEach-Object {
        [Console]::Error.WriteLine($_.ToString())
    }
    $targetExitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousErrorActionPreference
    return $targetExitCode
}

function Show-ZettaNotification {
    param(
        [Parameter(Mandatory)]
        [string] $Sound,

        [Parameter(Mandatory)]
        [string] $Summary,

        [Parameter(Mandatory)]
        [string] $Body
    )

    $zettaCommand = Get-Command zetta -CommandType Application -ErrorAction SilentlyContinue
    if ($null -eq $zettaCommand) {
        [Console]::Error.WriteLine("warning: could not show Zetta desktop notification")
        return
    }

    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & $zettaCommand.Source attention --notify --sound $Sound $Summary $Body 2>&1 | ForEach-Object {
        [Console]::Error.WriteLine($_.ToString())
    }
    if ($LASTEXITCODE -ne 0) {
        [Console]::Error.WriteLine("warning: could not show Zetta desktop notification")
    }
    $ErrorActionPreference = $previousErrorActionPreference
}

$lintExitCode = Invoke-MakeTarget lint
if ($lintExitCode -ne 0) {
    Show-ZettaNotification `
        -Sound "zetta-alarm" `
        -Summary "Zetta lint failed" `
        -Body "The stop-hook lint step failed."
    exit $lintExitCode
}

$testExitCode = Invoke-MakeTarget test
if ($testExitCode -ne 0) {
    Show-ZettaNotification `
        -Sound "zetta-alarm" `
        -Summary "Zetta tests failed" `
        -Body "The stop-hook test step failed."
    exit $testExitCode
}

$buildExitCode = Invoke-MakeTarget build
if ($buildExitCode -ne 0) {
    Show-ZettaNotification `
        -Sound "zetta-alarm" `
        -Summary "Zetta build failed" `
        -Body "Tests passed, but the stop-hook build step failed."
    exit $buildExitCode
}

Show-ZettaNotification `
    -Sound "zetta-ok" `
    -Summary "Zetta checks succeeded" `
    -Body "Tests and the development build completed successfully."

[Console]::Out.WriteLine('{"continue":true}')
exit 0
