$ErrorActionPreference = 'Stop'
$module = Join-Path $env:FIXTURE 'modules/OptionalShortcuts'
New-Item -ItemType Directory -Path $module -Force | Out-Null
@'
@{
    RootModule = 'OptionalShortcuts.psm1'
    ModuleVersion = '1.0'
    FunctionsToExport = @('vi', 'mosh')
    CmdletsToExport = @()
    AliasesToExport = @()
}
'@ | Set-Content -LiteralPath (Join-Path $module 'OptionalShortcuts.psd1')
@'
function vi { 'module vi' }
function mosh { 'module mosh' }
Export-ModuleMember -Function vi,mosh
'@ | Set-Content -LiteralPath (Join-Path $module 'OptionalShortcuts.psm1')

$env:PSModulePath = Join-Path $env:FIXTURE 'modules'
$env:PATH = $env:FIXTURE
. (Join-Path $env:FIXTURE 'startup.ps1')
if (Get-Module OptionalShortcuts) { throw 'startup imported an optional module' }
function global:zetta { $args -join '|' }
if ((vi first) -ne 'vi|first') { throw 'missing vi did not receive the fallback' }
if ((mosh host) -ne 'mosh|host') { throw 'missing mosh did not receive the fallback' }

Remove-Item Function:vi
Set-Alias -Name vi -Value Write-Output -Scope Global
function global:mosh { 'existing mosh' }
. (Join-Path $env:FIXTURE 'startup.ps1')
if ((vi 'existing vi') -ne 'existing vi') { throw 'startup replaced a vi alias' }
if ((mosh) -ne 'existing mosh') { throw 'startup replaced a mosh function' }

Remove-Item Alias:vi,Function:mosh
Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32/cmd.exe') -Destination (Join-Path $env:FIXTURE 'vi.exe')
Set-Content -LiteralPath (Join-Path $env:FIXTURE 'mosh.ps1') -Value "'native mosh script'"
. (Join-Path $env:FIXTURE 'startup.ps1')
$vi = Get-Command vi
if ($vi.CommandType -ne 'Application') { throw 'startup replaced a native vi' }
if (-not (__zetta_command_available 'VI')) { throw 'lookup is not case insensitive' }
$mosh = Get-Command mosh
if ($mosh.CommandType -ne 'ExternalScript') { throw 'startup replaced a mosh script' }
if (__zetta_command_available 'viking') { throw 'lookup accepted a prefix' }
