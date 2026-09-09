param(
    [string] $State = ""
)

$ErrorActionPreference = "Stop"

function Write-HookWarning {
    param(
        [Parameter(Mandatory)]
        [string] $Message
    )

    [Console]::Error.WriteLine("warning: codex tab icon hook: $Message")
}

function Get-PropertyValue {
    param(
        $Object,

        [Parameter(Mandatory)]
        [string] $Name
    )

    if ($null -eq $Object) {
        return $null
    }

    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $null
    }

    return $property.Value
}

# UserPromptSubmit does not expose collaboration mode. Use the transcript as
# a best-effort bridge, and only trust a plan record for the current turn.
function Test-TranscriptPlanMode {
    param(
        [string] $TranscriptPath,

        [string] $TurnId
    )

    if ([string]::IsNullOrWhiteSpace($TranscriptPath) -or
        [string]::IsNullOrWhiteSpace($TurnId)) {
        return $false
    }

    try {
        if (-not (Test-Path -LiteralPath $TranscriptPath -PathType Leaf)) {
            return $false
        }

        foreach ($line in [System.IO.File]::ReadLines($TranscriptPath)) {
            if ([string]::IsNullOrWhiteSpace($line)) {
                continue
            }

            try {
                $entry = $line | ConvertFrom-Json -ErrorAction Stop
            } catch {
                continue
            }

            $task = Get-PropertyValue $entry "payload"
            if ($null -eq $task) {
                continue
            }

            $taskType = [string] (Get-PropertyValue $task "type")
            $taskTurnId = [string] (Get-PropertyValue $task "turn_id")
            $collaborationMode = [string] (Get-PropertyValue $task "collaboration_mode_kind")
            if ($taskType -eq "task_started" -and
                $taskTurnId -eq $TurnId -and
                $collaborationMode -eq "plan") {
                return $true
            }
        }
    } catch {
        return $false
    }

    return $false
}

$validStates = @("", "idle", "prompt", "reset")
if ($State -notin $validStates) {
    Write-HookWarning "unknown hook state: $State"
    exit 0
}

# Lifecycle calls pass their state explicitly, so they can update the tab
# without waiting for the hook payload to arrive or be parsed.
if ($State -eq "idle") {
    $icon = "ai_open_ai"
} elseif ($State -eq "reset") {
    # Reset does not need a hook payload.
} else {
    $rawInput = [Console]::In.ReadToEnd()
    if ([string]::IsNullOrWhiteSpace($rawInput)) {
        Write-HookWarning "could not parse the hook input as JSON"
        exit 0
    }

    try {
        $payload = $rawInput | ConvertFrom-Json
    } catch {
        Write-HookWarning "could not parse the hook input as JSON"
        exit 0
    }

    $eventName = [string] (Get-PropertyValue $payload "hook_event_name")
    $source = [string] (Get-PropertyValue $payload "source")
    $permissionMode = [string] (Get-PropertyValue $payload "permission_mode")
    $transcriptPath = [string] (Get-PropertyValue $payload "transcript_path")
    $turnId = [string] (Get-PropertyValue $payload "turn_id")

    if ($State -eq "prompt") {
        if ($permissionMode -eq "plan" -or
            (Test-TranscriptPlanMode $transcriptPath $turnId)) {
            $icon = "ai_open_ai_gpt_sub"
        } else {
            $icon = "ai_open_ai_compat"
        }
    } elseif ($eventName -in @("SessionStart", "Stop", "Interrupt") -or
        $source -in @("startup", "resume", "clear")) {
        $icon = "ai_open_ai"
    } elseif ($permissionMode -eq "plan") {
        $icon = "ai_open_ai_gpt_sub"
    } else {
        $icon = "ai_open_ai_compat"
    }
}

$zettaExecutable = $env:ZETTA_HOST_EXECUTABLE
if ([string]::IsNullOrWhiteSpace($zettaExecutable) -or
    -not (Test-Path -LiteralPath $zettaExecutable -PathType Leaf)) {
    $zettaCommand = Get-Command zetta -CommandType Application -ErrorAction SilentlyContinue
    if ($null -eq $zettaCommand) {
        Write-HookWarning "could not find zetta on PATH"
        exit 0
    }
    $zettaExecutable = $zettaCommand.Source
}

$previousErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
$zettaExitCode = 1
for ($attempt = 1; $attempt -le 5; $attempt++) {
    try {
        if ($State -eq "reset") {
            & $zettaExecutable tabicon --reset 2>&1 | ForEach-Object {
                [Console]::Error.WriteLine($_.ToString())
            }
        } else {
            & $zettaExecutable tabicon $icon 2>&1 | ForEach-Object {
                [Console]::Error.WriteLine($_.ToString())
            }
        }
        $zettaExitCode = $LASTEXITCODE
    } catch {
        $zettaExitCode = 1
    }
    if ($zettaExitCode -eq 0) {
        break
    }
    if ($attempt -lt 5) {
        Start-Sleep -Milliseconds 100
    }
}
$ErrorActionPreference = $previousErrorActionPreference

if ($zettaExitCode -ne 0) {
    Write-HookWarning "could not update the Zetta tab icon"
}

exit 0
