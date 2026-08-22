#requires -Version 7.0
[CmdletBinding()]
param(
    [string]$ZettaBinary = "target/release/zetta.exe",
    [string]$OutputPath = "artifacts/shell-startup-performance.json",
    [ValidateRange(1, 100)] [int]$ColdRuns = 5,
    [ValidateRange(1, 1000)] [int]$WarmRuns = 20,
    [ValidateRange(1, 300)] [int]$TimeoutSeconds = 30,
    [string]$Msys2Bash,
    [string]$Msys2Zsh,
    [switch]$SkipWsl
)

$ErrorActionPreference = "Stop"
if (-not $IsWindows) {
    throw "This benchmark measures Windows shell startup and must run on Windows."
}

# A job object accounts for the shell and every short-lived process it starts,
# including `zetta init`, after those children have already exited.
if (-not ("ZettaBenchmarkJob" -as [type])) {
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public sealed class ZettaBenchmarkJob : IDisposable
{
    [StructLayout(LayoutKind.Sequential)]
    private struct BasicAccounting
    {
        public long TotalUserTime, TotalKernelTime;
        public long ThisPeriodUserTime, ThisPeriodKernelTime;
        public uint TotalPageFaultCount, TotalProcesses;
        public uint ActiveProcesses, TotalTerminatedProcesses;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct IoCounters
    {
        public ulong ReadOperationCount, WriteOperationCount, OtherOperationCount;
        public ulong ReadTransferCount, WriteTransferCount, OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BasicAndIoAccounting
    {
        public BasicAccounting BasicInfo;
        public IoCounters IoInfo;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateJobObject(IntPtr attributes, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool QueryInformationJobObject(
        IntPtr job, int informationClass, out BasicAndIoAccounting information,
        uint informationLength, IntPtr returnLength);
    [DllImport("kernel32.dll")]
    private static extern bool CloseHandle(IntPtr handle);

    private IntPtr handle;

    public ZettaBenchmarkJob()
    {
        handle = CreateJobObject(IntPtr.Zero, null);
        if (handle == IntPtr.Zero)
            throw new Win32Exception(Marshal.GetLastWin32Error());
    }

    public void Assign(IntPtr processHandle)
    {
        if (!AssignProcessToJobObject(handle, processHandle))
            throw new Win32Exception(Marshal.GetLastWin32Error());
    }

    public ZettaBenchmarkJobMetrics Query()
    {
        BasicAndIoAccounting information;
        if (!QueryInformationJobObject(
            handle, 8, out information,
            (uint)Marshal.SizeOf<BasicAndIoAccounting>(), IntPtr.Zero))
            throw new Win32Exception(Marshal.GetLastWin32Error());

        return new ZettaBenchmarkJobMetrics {
            CpuMilliseconds = TimeSpan.FromTicks(
                information.BasicInfo.TotalUserTime + information.BasicInfo.TotalKernelTime
            ).TotalMilliseconds,
            ReadBytes = information.IoInfo.ReadTransferCount,
            WriteBytes = information.IoInfo.WriteTransferCount,
            TotalProcesses = information.BasicInfo.TotalProcesses
        };
    }

    public void Dispose()
    {
        if (handle != IntPtr.Zero) {
            CloseHandle(handle);
            handle = IntPtr.Zero;
        }
    }
}

public sealed class ZettaBenchmarkJobMetrics
{
    public double CpuMilliseconds { get; set; }
    public ulong ReadBytes { get; set; }
    public ulong WriteBytes { get; set; }
    public uint TotalProcesses { get; set; }
}
'@
}

function Quote-PowerShellLiteral([string]$Value) {
    return "'" + $Value.Replace("'", "''") + "'"
}

function Quote-Posix([string]$Value) {
    return "'" + $Value.Replace("'", "'\''") + "'"
}

function Resolve-OptionalCommand([string]$ExplicitPath, [string[]]$Candidates) {
    if ($ExplicitPath) {
        return (Resolve-Path -LiteralPath $ExplicitPath).Path
    }
    foreach ($candidate in $Candidates) {
        $command = Get-Command $candidate -ErrorAction SilentlyContinue
        if ($command -and $command.Source) {
            return $command.Source
        }
    }
    return $null
}

function Resolve-Msys2Shell([string]$ExplicitPath, [string]$Name) {
    $resolved = Resolve-OptionalCommand $ExplicitPath @(
        "C:\msys64\usr\bin\$Name.exe",
        "C:\tools\msys64\usr\bin\$Name.exe"
    )
    if ($resolved -and $resolved -match '(?i)[\\/]msys(?:2|64)?[\\/]') {
        return $resolved
    }
    return $null
}

function Convert-ToMsysPath([string]$ShellPath, [string]$NativePath) {
    $cygpath = Join-Path (Split-Path -Parent $ShellPath) "cygpath.exe"
    if (-not (Test-Path -LiteralPath $cygpath)) {
        throw "Could not find cygpath.exe beside $ShellPath"
    }
    $converted = (& $cygpath -u -- $NativePath | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $converted) {
        throw "cygpath.exe could not convert $NativePath"
    }
    return $converted
}

function Get-Percentile([double[]]$Values, [double]$Percentile) {
    $sorted = @($Values | Sort-Object)
    $index = [Math]::Max(0, [Math]::Ceiling($Percentile * $sorted.Count) - 1)
    return [Math]::Round($sorted[$index], 3)
}

function Invoke-MeasuredProcess($Case, [string]$Temperature, [int]$Iteration) {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Case.FileName
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.RedirectStandardInput = $null -ne $Case.StandardInput
    foreach ($argument in $Case.Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }
    foreach ($entry in $Case.Environment.GetEnumerator()) {
        $startInfo.Environment[$entry.Key] = $entry.Value
    }

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $job = [ZettaBenchmarkJob]::new()
    $timer = [Diagnostics.Stopwatch]::StartNew()
    try {
        if (-not $process.Start()) {
            throw "Failed to start $($Case.FileName)"
        }
        $job.Assign($process.Handle)
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if ($null -ne $Case.StandardInput) {
            $process.StandardInput.Write($Case.StandardInput)
            $process.StandardInput.Close()
        }
        if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
            $process.Kill($true)
            throw "$($Case.Name) exceeded the ${TimeoutSeconds}s timeout"
        }
        $timer.Stop()
        $stdout = $stdoutTask.GetAwaiter().GetResult()
        $stderr = $stderrTask.GetAwaiter().GetResult()
        $metrics = $job.Query()
        $markerPattern = [regex]::Escape("$([char]27)]2;zetta-cwd:") +
            '(?<cwd>.*?)' + [regex]::Escape("$([char]27)\")
        $marker = [regex]::Match($stdout + $stderr, $markerPattern).Value
        if ($Case.RequiresMarker -and -not $marker) {
            throw "$($Case.Name) exited without a zetta-cwd marker. stderr: $stderr"
        }
        if ($process.ExitCode -ne 0) {
            throw "$($Case.Name) exited with code $($process.ExitCode). stderr: $stderr"
        }

        return [pscustomobject]@{
            case = $Case.Name
            temperature = $Temperature
            iteration = $Iteration
            wall_time_ms = [Math]::Round($timer.Elapsed.TotalMilliseconds, 3)
            cpu_time_ms = [Math]::Round($metrics.CpuMilliseconds, 3)
            read_bytes = [uint64]$metrics.ReadBytes
            write_bytes = [uint64]$metrics.WriteBytes
            child_process_count = [Math]::Max(0, [int]$metrics.TotalProcesses - 1)
            first_cwd_marker = if ($marker) { $marker } else { $null }
        }
    }
    finally {
        $timer.Stop()
        $job.Dispose()
        $process.Dispose()
    }
}

$zetta = (Resolve-Path -LiteralPath $ZettaBinary).Path
$workingDirectory = (Resolve-Path -LiteralPath ".").Path
$emptyEnvironment = @{}
$cases = [Collections.Generic.List[object]]::new()
foreach ($shell in @("bash", "fish", "powershell", "zsh")) {
    $cases.Add([pscustomobject]@{
        Name = "zetta-init-$shell"
        FileName = $zetta
        Arguments = @("init", $shell)
        Environment = $emptyEnvironment
        StandardInput = $null
        RequiresMarker = $false
    })
}

$powerShellCommand = @(
    "`$integration = & $(Quote-PowerShellLiteral $zetta) init powershell | Out-String"
    "Invoke-Expression `$integration"
    "Set-Location -LiteralPath $(Quote-PowerShellLiteral $workingDirectory)"
    "prompt"
) -join "; "
foreach ($shell in @(
    [pscustomobject]@{ Name = "windows-powershell"; Command = "powershell.exe" },
    [pscustomobject]@{ Name = "powershell-7"; Command = "pwsh.exe" }
)) {
    $executable = Resolve-OptionalCommand $null @($shell.Command)
    if ($executable) {
        $cases.Add([pscustomobject]@{
            Name = $shell.Name
            FileName = $executable
            Arguments = @("-NoLogo", "-NoProfile", "-NonInteractive", "-Command", $powerShellCommand)
            Environment = $emptyEnvironment
            StandardInput = $null
            RequiresMarker = $true
        })
    } else {
        Write-Warning "Skipping $($shell.Name): $($shell.Command) was not found."
    }
}

$cmd = Resolve-OptionalCommand $null @("cmd.exe")
if ($cmd) {
    $cases.Add([pscustomobject]@{
        Name = "cmd"
        FileName = $cmd
        Arguments = @("/d", "/q", "/k")
        Environment = @{ PROMPT = '$E]2;zetta-cwd:$P$E\$P$G' }
        StandardInput = "exit`r`n"
        RequiresMarker = $true
    })
}

$msysBashPath = Resolve-Msys2Shell $Msys2Bash "bash"
$msysZshPath = Resolve-Msys2Shell $Msys2Zsh "zsh"
foreach ($shell in @(
    [pscustomobject]@{ Name = "msys2-bash"; Kind = "bash"; Path = $msysBashPath; Arguments = @("--noprofile", "--norc", "-i", "-c") },
    [pscustomobject]@{ Name = "msys2-zsh"; Kind = "zsh"; Path = $msysZshPath; Arguments = @("-dfi", "-c") }
)) {
    if (-not $shell.Path) {
        Write-Warning "Skipping $($shell.Name): pass its executable path explicitly if MSYS2 is installed elsewhere."
        continue
    }
    $zettaPosix = Convert-ToMsysPath $shell.Path $zetta
    $cwdPosix = Convert-ToMsysPath $shell.Path $workingDirectory
    $command = "eval `"`$($(Quote-Posix $zettaPosix) init $($shell.Kind))`"; cd -- $(Quote-Posix $cwdPosix); __zetta_report_cwd"
    $cases.Add([pscustomobject]@{
        Name = $shell.Name
        FileName = $shell.Path
        Arguments = @($shell.Arguments + $command)
        Environment = $emptyEnvironment
        StandardInput = $null
        RequiresMarker = $true
    })
}

if (-not $SkipWsl) {
    $wsl = Resolve-OptionalCommand $null @("wsl.exe")
    if ($wsl) {
        $zettaWsl = (& $wsl --exec wslpath -a $zetta | Out-String).Trim()
        if ($LASTEXITCODE -eq 0 -and $zettaWsl) {
            $wslCommand = "eval `"`$($(Quote-Posix $zettaWsl) init bash)`"; cd /; __zetta_report_cwd"
            $cases.Add([pscustomobject]@{
                Name = "wsl-control"
                FileName = $wsl
                Arguments = @("--exec", "bash", "--noprofile", "--norc", "-i", "-c", $wslCommand)
                Environment = $emptyEnvironment
                StandardInput = $null
                RequiresMarker = $true
            })
        } else {
            Write-Warning "Skipping WSL control: wslpath could not convert the Zetta path."
        }
    }
}

$samples = [Collections.Generic.List[object]]::new()
foreach ($case in $cases) {
    Write-Host "Measuring $($case.Name) ($ColdRuns cold-order, $WarmRuns warm runs)"
    for ($iteration = 1; $iteration -le $ColdRuns; $iteration++) {
        $samples.Add((Invoke-MeasuredProcess $case "cold" $iteration))
    }
    Invoke-MeasuredProcess $case "warmup" 0 | Out-Null
    for ($iteration = 1; $iteration -le $WarmRuns; $iteration++) {
        $samples.Add((Invoke-MeasuredProcess $case "warm" $iteration))
    }
}

$summaries = foreach ($group in $samples | Group-Object case, temperature) {
    $values = @($group.Group)
    [pscustomobject]@{
        case = $values[0].case
        temperature = $values[0].temperature
        sample_count = $values.Count
        wall_time_ms = [pscustomobject]@{
            median = Get-Percentile ([double[]]$values.wall_time_ms) 0.50
            p95 = Get-Percentile ([double[]]$values.wall_time_ms) 0.95
        }
        cpu_time_ms = [pscustomobject]@{
            median = Get-Percentile ([double[]]$values.cpu_time_ms) 0.50
            p95 = Get-Percentile ([double[]]$values.cpu_time_ms) 0.95
        }
        read_bytes = [pscustomobject]@{
            median = Get-Percentile ([double[]]$values.read_bytes) 0.50
            p95 = Get-Percentile ([double[]]$values.read_bytes) 0.95
        }
        write_bytes = [pscustomobject]@{
            median = Get-Percentile ([double[]]$values.write_bytes) 0.50
            p95 = Get-Percentile ([double[]]$values.write_bytes) 0.95
        }
        child_process_count = [pscustomobject]@{
            median = Get-Percentile ([double[]]$values.child_process_count) 0.50
            p95 = Get-Percentile ([double[]]$values.child_process_count) 0.95
        }
        first_cwd_markers = @($values.first_cwd_marker | Where-Object { $_ } | Select-Object -Unique)
    }
}

$report = [ordered]@{
    schema_version = 1
    generated_at_utc = [DateTime]::UtcNow.ToString("o")
    zetta_binary = $zetta
    operating_system = [Environment]::OSVersion.VersionString
    processor_count = [Environment]::ProcessorCount
    cold_definition = "Runs before the per-case warmup; Windows file-cache purging is intentionally not attempted."
    parameters = [ordered]@{
        cold_runs = $ColdRuns
        warm_runs = $WarmRuns
        timeout_seconds = $TimeoutSeconds
    }
    summaries = @($summaries)
    samples = @($samples)
}

$output = [IO.Path]::GetFullPath($OutputPath)
[IO.Directory]::CreateDirectory((Split-Path -Parent $output)) | Out-Null
$report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $output -Encoding utf8NoBOM
Write-Host "Wrote shell-startup benchmark report to $output"
