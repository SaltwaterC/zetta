$ErrorActionPreference = 'Stop'

$wrapper = Join-Path $PSScriptRoot 'cargo-windows.ps1'
$directory = Join-Path ([IO.Path]::GetTempPath()) ('zetta cargo priority ' + [Guid]::NewGuid())
[IO.Directory]::CreateDirectory($directory) | Out-Null
$probe = Join-Path $directory 'probe.ps1'
$previousCargo = $env:CARGO
$previousVsVersion = $env:VSCMD_VER
$previousVcTools = $env:VCToolsInstallDir
$previousPath = $env:PATH
try {
    @'
param([string] $Value, [int] $ExitCode)
$ErrorActionPreference = 'Stop'
$priority = [Diagnostics.Process]::GetCurrentProcess().PriorityClass.ToString()
$childPriority = & powershell.exe -NoProfile -Command '[Diagnostics.Process]::GetCurrentProcess().PriorityClass.ToString()'
@{ priority = $priority; childPriority = $childPriority; value = $Value; linker = (Get-Command link.exe -CommandType Application | Select-Object -First 1).Source } | ConvertTo-Json -Compress
exit $ExitCode
'@ | Set-Content -LiteralPath $probe -Encoding UTF8

    # Fake Cargo isolates the wrapper's argument, priority and exit handling
    # from the Rust toolchain and Visual Studio installation.
    $env:CARGO = 'powershell.exe'
    $env:VSCMD_VER = 'priority-test'
    # Git's shell can prepend its tools after vcvars64 has already run. Model
    # an initialized MSVC environment whose PATH now finds Git's linker first.
    $tools = Join-Path $directory 'VC tools'
    $msvcBin = Join-Path $tools 'bin/Hostx64/x64'
    $gitBin = Join-Path $directory 'Git tools'
    [IO.Directory]::CreateDirectory($msvcBin) | Out-Null
    [IO.Directory]::CreateDirectory($gitBin) | Out-Null
    [IO.File]::WriteAllText((Join-Path $msvcBin 'link.exe'), '')
    [IO.File]::WriteAllText((Join-Path $gitBin 'link.exe'), '')
    $env:VCToolsInstallDir = $tools + [IO.Path]::DirectorySeparatorChar
    $env:PATH = $gitBin + ';' + $previousPath
    foreach ($exitCode in @(0, 37)) {
        $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $wrapper `
            -NoProfile -ExecutionPolicy Bypass -File $probe 'argument with spaces' $exitCode
        $actualExitCode = $LASTEXITCODE
        if ($actualExitCode -ne $exitCode) {
            throw "Expected exit $exitCode, got $actualExitCode"
        }
        $result = $output | ConvertFrom-Json
        if ($result.priority -ne 'BelowNormal' -or $result.childPriority -ne 'BelowNormal') {
            throw "Cargo and its child must both run BelowNormal: $output"
        }
        if ($result.value -ne 'argument with spaces') {
            throw "Quoted argument was not preserved: $output"
        }
        if ($result.linker -ne (Join-Path $msvcBin 'link.exe')) {
            throw "The wrapper selected a non-MSVC linker: $($result.linker)"
        }
    }
    # Exercise the stop hook's actual launcher without running its validation
    # stages or notifications. Substitute only the Make executable.
    $fakeMake = Join-Path $directory 'fake make.cmd'
    function Get-Command {
        [CmdletBinding()]
        param([string] $Name, [string] $CommandType)
        if ($Name -eq 'make') {
            [pscustomobject] @{ Source = $fakeMake }
        }
    }
    function Write-HookProgress { param([string] $Message) }
    $hookLogPath = Join-Path $directory 'hook.log'
    $hookSource = Get-Content (Join-Path $PSScriptRoot 'agent-stop-hook.ps1') -Raw
    $hookAst = [Management.Automation.Language.Parser]::ParseInput($hookSource, [ref] $null, [ref] $null)
    $launcher = $hookAst.Find({
        param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Invoke-MakeTarget'
    }, $false)
    . ([scriptblock]::Create($launcher.Extent.Text))
    foreach ($exitCode in @(0, 37)) {
        '@echo off' + "`r`npowershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$probe`" `"argument with spaces`" $exitCode`r`nexit /b %errorlevel%" |
            Set-Content -LiteralPath $fakeMake -Encoding ASCII
        $actualExitCode = Invoke-MakeTarget test
        if ($actualExitCode -ne $exitCode) {
            throw "Hook expected exit $exitCode, got $actualExitCode"
        }
        $result = Get-Content ([IO.Path]::ChangeExtension($hookLogPath, 'test.output.log')) -Raw | ConvertFrom-Json
        if ($result.priority -ne 'BelowNormal' -or $result.childPriority -ne 'BelowNormal') {
            throw 'Make and its child must both run BelowNormal'
        }
    }
    Write-Output 'Cargo and hook launchers: linker selection, priority, quoting and exit status passed'
} finally {
    $env:CARGO = $previousCargo
    $env:VSCMD_VER = $previousVsVersion
    $env:VCToolsInstallDir = $previousVcTools
    $env:PATH = $previousPath
    # The only recursive cleanup target is the unique test directory above.
    $resolvedDirectory = [IO.Path]::GetFullPath($directory)
    $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if (-not $resolvedDirectory.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Test cleanup escaped the temporary directory: $resolvedDirectory"
    }
    Remove-Item -LiteralPath $resolvedDirectory -Recurse -Force
}
exit 0
