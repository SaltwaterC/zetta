$ErrorActionPreference = 'Stop'

$hook = Join-Path $PSScriptRoot 'codex-tab-icon-hook.ps1'
$temporaryFile = [IO.Path]::GetTempFileName()
$fakeZetta = [IO.Path]::ChangeExtension($temporaryFile, '.cmd')
$output = [IO.Path]::GetTempFileName()
$originalExecutable = $env:ZETTA_HOST_EXECUTABLE
$originalProcessId = $env:ZETTA_PROCESS_ID
$originalOutput = $env:ZETTA_HOOK_TEST_OUTPUT

try {
    Set-Content -LiteralPath $fakeZetta -Value @'
@echo off
echo %ZETTA_PROCESS_ID%^|%* > "%ZETTA_HOOK_TEST_OUTPUT%"
exit /b 0
'@
    $env:ZETTA_HOST_EXECUTABLE = $fakeZetta
    $env:ZETTA_HOOK_TEST_OUTPUT = $output

    $env:ZETTA_PROCESS_ID = [string] [int]::MaxValue
    & $hook idle
    if ((Get-Content -LiteralPath $output -Raw).Trim() -ne '|tabicon ai_open_ai') {
        throw 'The hook did not discard a stale Zetta process ID.'
    }

    $env:ZETTA_PROCESS_ID = [string] $PID
    & $hook idle
    if ((Get-Content -LiteralPath $output -Raw).Trim() -ne "$PID|tabicon ai_open_ai") {
        throw 'The hook did not preserve a live Zetta process ID.'
    }
} finally {
    $env:ZETTA_HOST_EXECUTABLE = $originalExecutable
    $env:ZETTA_PROCESS_ID = $originalProcessId
    $env:ZETTA_HOOK_TEST_OUTPUT = $originalOutput
    Remove-Item -LiteralPath $temporaryFile, $fakeZetta, $output -ErrorAction SilentlyContinue
}
