$ErrorActionPreference = "Stop"

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
Set-Location -LiteralPath $repositoryRoot

# Import MSVC once for the whole hook. The Makefile's Cargo wrapper provides
# the same environment for interactive targets, while this avoids repeatedly
# running vcvars64 for each target and standalone crate in this process.
function Import-VisualStudioEnvironment {
    if (-not [string]::IsNullOrWhiteSpace($env:VSCMD_VER)) {
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
            # A Windows environment inherited from PowerShell can contain
            # both PATH and Path. vcvars64.bat prepends its entries to the
            # first one, so keep the first case-insensitive match.
            if ($null -eq $path) {
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
}

if ($env:OS -eq "Windows_NT") {
    Import-VisualStudioEnvironment
}

function Invoke-MakeTarget {
    param(
        [Parameter(Mandatory)]
        [string] $Target
    )

    if ($env:OS -eq "Windows_NT") {
        $makeCommand = Get-Command make -CommandType Application -ErrorAction Stop |
            Select-Object -First 1
        $processStartInfo = [Diagnostics.ProcessStartInfo]::new()
        $processStartInfo.FileName = $env:ComSpec
        $processStartInfo.UseShellExecute = $false
        $makeArguments = @()
        $gitCommand = Get-Command git -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($null -ne $gitCommand) {
            $gitExecPath = & $gitCommand.Source --exec-path 2>$null
            if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($gitExecPath)) {
                $gitRoot = Split-Path (Split-Path (Split-Path ([string] $gitExecPath).Trim() -Parent) -Parent) -Parent
                $bashPath = Join-Path $gitRoot "usr\bin\bash.exe"
                if (Test-Path -LiteralPath $bashPath) {
                    $makeArguments += ('SHELL="{0}"' -f $bashPath)
                }
            }
        }
        $makeArguments += $Target
        $processStartInfo.Arguments = '/d /c ""{0}" {1} 1>&2"' -f `
            $makeCommand.Source, `
            ($makeArguments -join " ")

        # GNU Make uses a POSIX shell on Windows. Give it a normalized
        # environment so case-variant PATH entries cannot make its shell
        # resolve Git or Coreutils' link.exe instead of MSVC's linker.
        $processStartInfo.Environment.Clear()
        foreach ($entry in [Environment]::GetEnvironmentVariables("Process").GetEnumerator()) {
            if ($entry.Key -ine "PATH" -and $entry.Key -ine "Path") {
                $processStartInfo.Environment[$entry.Key] = [string] $entry.Value
            }
        }
        $processStartInfo.Environment["PATH"] = $env:PATH

        $process = [Diagnostics.Process]::Start($processStartInfo)
        $process.WaitForExit()
        return $process.ExitCode
    }

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
