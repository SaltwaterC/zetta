param([string]$Payload)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ByteProducer {
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr GetStdHandle(int kind);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool WriteFile(IntPtr output, byte[] bytes, uint size, out uint written, IntPtr overlapped);
}
'@
$stream = [System.IO.File]::OpenRead($Payload)
try {
    $buffer = New-Object byte[] 65536
    $output = [ByteProducer]::GetStdHandle(-11)
    while (($count = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
        [uint32]$written = 0
        if (-not [ByteProducer]::WriteFile($output, $buffer, $count, [ref]$written, [IntPtr]::Zero) -or $written -ne $count) {
            throw 'console output was incomplete'
        }
    }
} finally {
    $stream.Dispose()
}
