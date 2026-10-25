$ErrorActionPreference = 'Stop'
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Set-Location -LiteralPath $repositoryRoot
. (Join-Path $PSScriptRoot 'windows-process.ps1')

Import-VisualStudioEnvironment
Repair-MsvcLinkerPath

$makeCommand = Get-Command make -CommandType Application -ErrorAction Stop |
    Select-Object -First 1
$makeArguments = @()
$gitCommand = Get-Command git -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($null -ne $gitCommand) {
    $gitExecPath = & $gitCommand.Source --exec-path 2>$null
    if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($gitExecPath)) {
        $gitRoot = Split-Path (Split-Path (Split-Path ([string] $gitExecPath).Trim() -Parent) -Parent) -Parent
        $bashPath = Join-Path $gitRoot 'usr\bin\bash.exe'
        if (Test-Path -LiteralPath $bashPath) {
            $makeArguments += "SHELL=$bashPath"
        }
    }
}
$makeArguments += $args

$exitCode = Invoke-WindowsProcess `
    -FilePath $makeCommand.Source `
    -Arguments $makeArguments `
    -WorkingDirectory $repositoryRoot
exit $exitCode
