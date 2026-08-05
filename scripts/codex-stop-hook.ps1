$ErrorActionPreference = "Stop"

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
Set-Location -LiteralPath $repositoryRoot

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
    & $zettaCommand.Source notify --sound $Sound $Summary $Body 2>&1 | ForEach-Object {
        [Console]::Error.WriteLine($_.ToString())
    }
    if ($LASTEXITCODE -ne 0) {
        [Console]::Error.WriteLine("warning: could not show Zetta desktop notification")
    }
    $ErrorActionPreference = $previousErrorActionPreference
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
