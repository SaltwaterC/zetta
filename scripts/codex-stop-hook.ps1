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
    if ($targetExitCode -ne 0) {
        exit $targetExitCode
    }
}

Invoke-MakeTarget test
Invoke-MakeTarget build

[Console]::Out.WriteLine('{"continue":true}')
exit 0
