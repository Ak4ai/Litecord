# Shared helpers for the performance and end-to-end test scripts in this folder.
# They drive test builds (`cargo build --release --features automation`) through the
# localhost automation port; see src/automation.rs and src/utils/test_hooks.rs.

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path

# CPU use counted in cycles. Windows' CPU times (TotalProcessorTime) are sampled at the
# clock tick and miss threads that wake briefly, such as a 60 fps video thread (0.6%
# reported against 6.9% real). Cycles are exact; we report them as a percentage of a
# 3 GHz core so runs on the same machine compare directly.
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Cycles {
    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(int access, bool inherit, int id);
    [DllImport("kernel32.dll")] static extern IntPtr OpenThread(int access, bool inherit, int id);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern bool QueryProcessCycleTime(IntPtr h, out ulong c);
    [DllImport("kernel32.dll")] static extern bool QueryThreadCycleTime(IntPtr h, out ulong c);
    public static double OfProcess(int pid) {
        IntPtr h = OpenProcess(0x1000, false, pid);
        if (h == IntPtr.Zero) throw new InvalidOperationException("cannot open process " + pid);
        ulong c; QueryProcessCycleTime(h, out c); CloseHandle(h); return c;
    }
    public static double OfThread(int tid) {
        IntPtr h = OpenThread(0x0800, false, tid);
        if (h == IntPtr.Zero) return 0;
        ulong c; QueryThreadCycleTime(h, out c); CloseHandle(h); return c;
    }
}
"@ -ErrorAction SilentlyContinue

$CyclesPerCorePercent = 3.0e9 / 100

# Percent of one (3 GHz) core used by a process between two Cycles readings.
function Cpu-Percent([double]$Cycles0, [double]$Cycles1, [double]$Seconds) {
    return ($Cycles1 - $Cycles0) / $Seconds / $CyclesPerCorePercent
}

function Send-Litecord([int]$Port, [string]$Json) {
    $client = New-Object System.Net.Sockets.TcpClient("127.0.0.1", $Port)
    try {
        $stream = $client.GetStream()
        $writer = New-Object System.IO.StreamWriter($stream)
        $reader = New-Object System.IO.StreamReader($stream)
        $writer.WriteLine($Json); $writer.Flush()
        return ($reader.ReadLine() | ConvertFrom-Json)
    } finally {
        $client.Close()
    }
}

# Starts one Litecord instance in its own working directory (so its settings and log
# stay apart) and returns the process. $Env is a hashtable of extra environment variables.
function Start-Litecord([string]$Exe, [string]$WorkDir, [int]$Port, [hashtable]$Env = @{}) {
    New-Item -ItemType Directory -Force $WorkDir | Out-Null
    $saved = @{}
    $vars = @{ LITECORD_AUTOMATION_PORT = "$Port"; PATH = "$RepoRoot;$env:PATH" } + $Env
    foreach ($k in $vars.Keys) {
        $saved[$k] = [Environment]::GetEnvironmentVariable($k, "Process")
        [Environment]::SetEnvironmentVariable($k, $vars[$k], "Process")
    }
    try {
        return Start-Process -FilePath $Exe -WorkingDirectory $WorkDir -PassThru
    } finally {
        foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k], "Process") }
    }
}

function Wait-LoggedIn([int]$Port, [int]$TimeoutSeconds = 90) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        try {
            $s = Send-Litecord $Port '{"cmd":"state"}'
            # user_id comes from the gateway READY; the tag alone may still be the vault's.
            if ($s.is_logged_in -and $s.user_id -and $s.user_id -ne "0") { return $s }
        } catch { }
        Start-Sleep -Seconds 2
    }
    throw "instance on port $Port did not log in within $TimeoutSeconds s"
}

# Opens the server and joins the voice channel by name; refuses to stay anywhere else.
function Join-Voice([int]$Port, [string]$Server, [string]$Channel) {
    $r = Send-Litecord $Port (@{ cmd = "open_guild"; name = $Server } | ConvertTo-Json -Compress)
    if (-not $r.ok) { throw "port ${Port}: $($r.error)" }
    Start-Sleep -Seconds 4
    $r = Send-Litecord $Port (@{ cmd = "open_channel"; name = $Channel } | ConvertTo-Json -Compress)
    if (-not $r.ok) { throw "port ${Port}: $($r.error)" }
    if (-not $r.is_voice) { throw "port ${Port}: '$Channel' is not a voice channel" }
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline) {
        $v = (Send-Litecord $Port '{"cmd":"state"}').voice
        if ($v.is_in_voice -and -not $v.is_connecting -and $v.channel -eq $Channel) { return }
        Start-Sleep -Seconds 1
    }
    Send-Litecord $Port '{"cmd":"leave_voice"}' | Out-Null
    throw "port ${Port}: did not connect to '$Channel'"
}
