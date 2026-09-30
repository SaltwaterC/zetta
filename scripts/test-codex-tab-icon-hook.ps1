param(
    [string] $HookPath = (Join-Path $PSScriptRoot 'codex-tab-icon-hook.ps1'),
    [string] $WslDistribution = ""
)

$ErrorActionPreference = 'Stop'

$hook = $HookPath
$temporaryFile = [IO.Path]::GetTempFileName()
$fakeZetta = [IO.Path]::ChangeExtension($temporaryFile, '.cmd')
$output = [IO.Path]::GetTempFileName()
$transcript = [IO.Path]::GetTempFileName()
$originalExecutable = $env:ZETTA_HOST_EXECUTABLE
$originalProcessId = $env:ZETTA_PROCESS_ID
$originalOutput = $env:ZETTA_HOOK_TEST_OUTPUT
$wslTemporary = $null

function Assert-PromptIcon {
    param(
        [hashtable] $Payload,
        [string] $Icon,
        [string] $Description,
        [string] $Distribution = ""
    )

    # Clear the last result so a hook that never invokes Zetta cannot pass.
    Set-Content -LiteralPath $output -Value ''
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $hook, 'prompt')
    if (-not [string]::IsNullOrWhiteSpace($Distribution)) {
        $arguments += @('-WslDistribution', $Distribution)
    }
    $Payload | ConvertTo-Json -Compress |
        powershell.exe @arguments
    if ($LASTEXITCODE -ne 0 -or
        (Get-Content -LiteralPath $output -Raw).Trim() -ne "$PID|tabicon --queue $Icon") {
        throw $Description
    }
}

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
    if ((Get-Content -LiteralPath $output -Raw).Trim() -ne '|tabicon --queue ai_open_ai') {
        throw 'The hook did not discard a stale Zetta process ID.'
    }

    $env:ZETTA_PROCESS_ID = [string] $PID
    & $hook idle
    if ((Get-Content -LiteralPath $output -Raw).Trim() -ne "$PID|tabicon --queue ai_open_ai") {
        throw 'The hook did not preserve a live Zetta process ID.'
    }
    $prompt = @{ hook_event_name = 'UserPromptSubmit'; permission_mode = 'default' }
    Assert-PromptIcon $prompt 'ai_open_ai_compat' 'Missing transcript information did not select the working icon.'
    $prompt.permission_mode = 'plan'
    Assert-PromptIcon $prompt 'ai_open_ai_gpt_sub' 'Explicit Plan mode did not select the planning icon.'
    $prompt.permission_mode = 'default'
    $prompt.transcript_path = $transcript
    Assert-PromptIcon $prompt 'ai_open_ai_compat' 'A missing turn ID did not select the working icon.'
    $prompt.turn_id = 'current-turn'
    $prompt.transcript_path = "$transcript.missing"
    Assert-PromptIcon $prompt 'ai_open_ai_compat' 'A missing transcript did not select the working icon.'
    $prompt.transcript_path = $transcript

    Set-Content -LiteralPath $transcript -Encoding UTF8 -Value @(
        '{"payload":{"type":"task_started","turn_id":"earlier-turn","collaboration_mode_kind":"plan"}}'
        ''
        'malformed JSON'
        '{}'
        '{"payload":{"type":"task_started","turn_id":"current-turn","collaboration_mode_kind":"default"}}'
        '{"payload":'
    )
    Assert-PromptIcon $prompt 'ai_open_ai_compat' 'An earlier Plan turn or malformed line changed the current regular turn icon.'

    $writer = $null
    try {
        $writer = [IO.FileStream]::new(
            $transcript, [IO.FileMode]::Append, [IO.FileAccess]::Write, [IO.FileShare]::Read
        )
        $record = '{"payload":{"type":"task_started","turn_id":"current-turn","collaboration_mode_kind":"plan"}}' + "`n" + '{"payload":'
        $bytes = [Text.Encoding]::UTF8.GetBytes($record)
        $writer.Write($bytes, 0, $bytes.Length)
        $writer.Flush()
        Assert-PromptIcon $prompt 'ai_open_ai_gpt_sub' 'A transcript held open for writing did not select the planning icon.'
    } finally {
        if ($null -ne $writer) {
            $writer.Dispose()
        }
    }
    if (-not [string]::IsNullOrWhiteSpace($WslDistribution)) {
        $directoryName = 'zetta-hook-' + [Guid]::NewGuid().ToString('N')
        $wslTemporary = "\\wsl.localhost\$WslDistribution\tmp\$directoryName"
        New-Item -ItemType Directory -Path $wslTemporary | Out-Null
        $wslTranscript = Join-Path $wslTemporary 'planning transcript.jsonl'
        $wslPrompt = @{
            hook_event_name = 'UserPromptSubmit'
            permission_mode = 'default'
            transcript_path = "/tmp/$directoryName/planning transcript.jsonl"
            turn_id = 'current-turn'
        }
        Set-Content -LiteralPath $wslTranscript -Encoding UTF8 -Value @(
            '{"payload":{"type":"task_started","turn_id":"earlier-turn","collaboration_mode_kind":"plan"}}'
            'malformed JSON'
            '{"payload":{"type":"task_started","turn_id":"current-turn","collaboration_mode_kind":"default"}}'
            '{"payload":'
        )
        Assert-PromptIcon $wslPrompt 'ai_open_ai_compat' 'An earlier WSL Plan turn changed the current regular turn icon.' $WslDistribution
        Add-Content -LiteralPath $wslTranscript -Encoding UTF8 -Value @(
            '{"payload":{"type":"task_started","turn_id":"current-turn","collaboration_mode_kind":"plan"}}'
            '{"payload":'
        )
        Assert-PromptIcon $wslPrompt 'ai_open_ai_gpt_sub' 'A Linux transcript path with spaces did not select the planning icon.' $WslDistribution
        $wslPrompt.transcript_path = $wslTranscript
        Assert-PromptIcon $wslPrompt 'ai_open_ai_gpt_sub' 'An existing UNC transcript path was changed by WSL translation.' $WslDistribution
        $wslPrompt.transcript_path = $transcript
        Assert-PromptIcon $wslPrompt 'ai_open_ai_gpt_sub' 'A native Windows transcript path was changed by WSL translation.' $WslDistribution
        $wslPrompt.transcript_path = "/tmp/$directoryName/missing.jsonl"
        Assert-PromptIcon $wslPrompt 'ai_open_ai_compat' 'A missing WSL transcript did not select the regular working icon.' $WslDistribution
        $wslPrompt.transcript_path = "/tmp/$directoryName/planning transcript.jsonl"
        $wslPrompt.turn_id = 'missing-turn'
        Assert-PromptIcon $wslPrompt 'ai_open_ai_compat' 'A WSL transcript without the current turn selected the planning icon.' $WslDistribution
    }
    & $hook reset
    if ((Get-Content -LiteralPath $output -Raw).Trim() -ne "$PID|tabicon --queue --reset") {
        throw 'The session-end hook did not queue the icon reset.'
    }
} finally {
    $env:ZETTA_HOST_EXECUTABLE = $originalExecutable
    $env:ZETTA_PROCESS_ID = $originalProcessId
    $env:ZETTA_HOOK_TEST_OUTPUT = $originalOutput
    Remove-Item -LiteralPath $temporaryFile, $fakeZetta, $output, $transcript -ErrorAction SilentlyContinue
    if ($null -ne $wslTemporary) {
        Remove-Item -LiteralPath $wslTemporary -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Output 'Codex PowerShell tab-icon hook checks passed.'
