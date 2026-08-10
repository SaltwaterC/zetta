$ErrorActionPreference = "Stop"

function Write-HookWarning {
    param(
        [Parameter(Mandatory)]
        [string] $Message
    )

    [Console]::Error.WriteLine("warning: request-user-input hook: $Message")
}

function Get-DisplayText {
    param(
        $Value,

        [Parameter(Mandatory)]
        [string] $Fallback
    )

    if ($null -eq $Value) {
        return $Fallback
    }

    if ($Value -is [string]) {
        if ([string]::IsNullOrEmpty($Value)) {
            return $Fallback
        }
        return $Value
    }

    return [string] $Value
}

function Get-QuestionField {
    param(
        $Question,

        [Parameter(Mandatory)]
        [string] $Name,

        [Parameter(Mandatory)]
        [string] $Fallback
    )

    if ($null -eq $Question) {
        return $Fallback
    }

    $property = $Question.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $Fallback
    }

    return Get-DisplayText $property.Value $Fallback
}

function Get-OptionLabel {
    param($Option)

    if ($null -eq $Option) {
        return "(no label)"
    }

    if ($Option -is [string]) {
        return Get-DisplayText $Option "(no label)"
    }

    $property = $Option.PSObject.Properties["label"]
    if ($null -eq $property) {
        return "(no label)"
    }

    return Get-DisplayText $property.Value "(no label)"
}

function Format-Question {
    param(
        $Question,

        [Parameter(Mandatory)]
        [int] $Index
    )

    $lines = [Collections.Generic.List[string]]::new()
    [void] $lines.Add("Question $($Index + 1)")
    [void] $lines.Add("Header: $(Get-QuestionField $Question 'header' '(no header)')")
    [void] $lines.Add("Question: $(Get-QuestionField $Question 'question' '(no question text)')")
    [void] $lines.Add("Options:")

    $optionValues = $null
    if ($null -ne $Question) {
        $optionsProperty = $Question.PSObject.Properties["options"]
        if ($null -ne $optionsProperty) {
            $optionValues = $optionsProperty.Value
        }
    }
    $options = @($optionValues)

    if ($null -eq $optionValues -or $options.Count -eq 0) {
        [void] $lines.Add("  (no options)")
    } else {
        foreach ($option in $options) {
            [void] $lines.Add("  - $(Get-OptionLabel $option)")
        }
    }

    return ($lines -join [Environment]::NewLine)
}

$rawInput = [Console]::In.ReadToEnd()
try {
    $payload = $rawInput | ConvertFrom-Json
} catch {
    Write-HookWarning "could not parse the hook input as JSON"
    exit 0
}

$questions = $null
if ($null -ne $payload -and $null -ne $payload.tool_input) {
    $questions = $payload.tool_input.questions
}

if ($null -eq $questions) {
    $body = "No question details were provided."
} else {
    $questionItems = @($questions)
    if ($questionItems.Count -eq 0) {
        $body = "No question details were provided."
    } else {
        $blocks = for ($index = 0; $index -lt $questionItems.Count; $index++) {
            Format-Question $questionItems[$index] $index
        }
        $body = $blocks -join ([Environment]::NewLine + [Environment]::NewLine)
    }
}

$zettaCommand = Get-Command zetta -CommandType Application -ErrorAction SilentlyContinue
if ($null -eq $zettaCommand) {
    Write-HookWarning "could not find zetta on PATH"
    exit 0
}

$previousErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $zettaCommand.Source attention --notify --sound zetta-alarm "Codex input required" $body 2>&1 | ForEach-Object {
    [Console]::Error.WriteLine($_.ToString())
}
$zettaExitCode = $LASTEXITCODE
$ErrorActionPreference = $previousErrorActionPreference

if ($zettaExitCode -ne 0) {
    Write-HookWarning "could not show Zetta desktop notification"
}

exit 0
