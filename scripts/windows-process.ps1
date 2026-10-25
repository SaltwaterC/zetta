function Import-VisualStudioEnvironment {
    if (-not [string]::IsNullOrWhiteSpace($env:VSCMD_VER)) {
        return
    }

    $vswherePath = Join-Path `
        ([Environment]::GetEnvironmentVariable('ProgramFiles(x86)')) `
        'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswherePath)) {
        throw "Visual Studio Installer's vswhere.exe was not found. Install the Visual Studio Desktop development with C++ workload."
    }

    $installPaths = & $vswherePath `
        -latest `
        -products '*' `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath
    $vswhereExitCode = $LASTEXITCODE
    $installPath = $installPaths | Select-Object -First 1
    if ($vswhereExitCode -ne 0 -or [string]::IsNullOrWhiteSpace($installPath)) {
        throw 'A Visual Studio installation with the C++ build tools was not found.'
    }

    $vcvarsPath = Join-Path ([string] $installPath).Trim() 'VC\Auxiliary\Build\vcvars64.bat'
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
        $separator = $line.IndexOf('=')
        if ($separator -le 0) {
            continue
        }
        $name = $line.Substring(0, $separator)
        $value = $line.Substring($separator + 1)
        if ($name -ieq 'PATH') {
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
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, 'Process')
    }
    if ($null -ne $path) {
        [Environment]::SetEnvironmentVariable('PATH', $path, 'Process')
    }
}

function Repair-MsvcLinkerPath {
    if ([string]::IsNullOrWhiteSpace($env:VCToolsInstallDir)) {
        return
    }

    $msvcLinkerDirectory = Join-Path $env:VCToolsInstallDir 'bin/Hostx64/x64'
    if (-not (Test-Path -LiteralPath (Join-Path $msvcLinkerDirectory 'link.exe'))) {
        throw "The initialized MSVC environment has no x64 linker at $msvcLinkerDirectory."
    }
    $env:PATH = $msvcLinkerDirectory + [IO.Path]::PathSeparator + $env:PATH
}

function ConvertTo-WindowsArgument([string] $Value) {
    if ($Value.Length -gt 0 -and $Value -notmatch '[\s"]') {
        return $Value
    }

    $builder = [Text.StringBuilder]::new()
    [void] $builder.Append('"')
    $backslashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') {
            $backslashes++
            continue
        }
        if ($character -eq '"') {
            [void] $builder.Append(('\' * (2 * $backslashes + 1)))
            [void] $builder.Append('"')
            $backslashes = 0
            continue
        }
        if ($backslashes -gt 0) {
            [void] $builder.Append(('\' * $backslashes))
            $backslashes = 0
        }
        [void] $builder.Append($character)
    }
    if ($backslashes -gt 0) {
        [void] $builder.Append(('\' * (2 * $backslashes)))
    }
    [void] $builder.Append('"')
    return $builder.ToString()
}

function Stop-WindowsProcessTree([Diagnostics.Process] $Process) {
    if ($Process.HasExited) {
        return
    }
    try {
        $Process.Kill($true)
    } catch {
        if ($Process.HasExited) {
            return
        }
        $taskkill = Join-Path $env:SystemRoot 'System32/taskkill.exe'
        & $taskkill /F /T /PID $Process.Id 2>$null | Out-Null
        if (-not $Process.HasExited) {
            $Process.Kill()
        }
    }
    $Process.WaitForExit()
}

function Invoke-WindowsProcess {
    param(
        [Parameter(Mandatory)]
        [string] $FilePath,
        [string[]] $Arguments = @(),
        [string] $WorkingDirectory = (Get-Location).Path
    )

    # Children inherit this priority even before their process handle is opened.
    [Diagnostics.Process]::GetCurrentProcess().PriorityClass = [Diagnostics.ProcessPriorityClass]::BelowNormal

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.Arguments = (($Arguments | ForEach-Object {
        ConvertTo-WindowsArgument ([string] $_)
    }) -join ' ')
    $startInfo.WorkingDirectory = $WorkingDirectory
    $startInfo.UseShellExecute = $false

    $cancelRequested = [hashtable]::Synchronized(@{ Value = $false })
    $cancelHandler = [ConsoleCancelEventHandler] {
        param($sender, $eventArgs)
        $eventArgs.Cancel = $true
        $cancelRequested['Value'] = $true
    }
    $process = $null
    $completedNormally = $false
    [Console]::add_CancelKeyPress($cancelHandler)
    try {
        $process = [Diagnostics.Process]::Start($startInfo)
        $process.PriorityClass = [Diagnostics.ProcessPriorityClass]::BelowNormal
        while (-not $process.WaitForExit(200)) {
            if ($cancelRequested.Value) {
                Stop-WindowsProcessTree $process
                $completedNormally = $true
                return 130
            }
        }
        $completedNormally = $true
        return $process.ExitCode
    } finally {
        [Console]::remove_CancelKeyPress($cancelHandler)
        if ($null -ne $process) {
            if (-not $completedNormally) {
                Stop-WindowsProcessTree $process
            }
            $process.Dispose()
        }
    }
}
