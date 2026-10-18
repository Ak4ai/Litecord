# Counts how often each thread of a process wakes up (context switches per second) over
# a time window, with thread names (or the module of the start address for unnamed ones),
# plus each thread's CPU (percent of a 3 GHz core, counted in cycles: see lib.ps1) and the
# process memory. Usage: wakeups.ps1 -ProcId <pid> -Seconds 60 -Label x -Out file.txt
param([int]$ProcId, [int]$Seconds = 60, [string]$Label = "", [string]$Out)

Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;
public static class Wake {
    [DllImport("kernel32.dll")] static extern IntPtr OpenThread(int access, bool inherit, int id);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern int GetThreadDescription(IntPtr h, out IntPtr desc);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
    [DllImport("ntdll.dll")] static extern int NtQueryInformationThread(IntPtr h, int cls, out IntPtr info, int len, IntPtr ret);
    [DllImport("kernel32.dll")] static extern bool QueryThreadCycleTime(IntPtr h, out ulong c);
    public static double Cycles(int tid) {
        IntPtr h = OpenThread(0x0800, false, tid);
        if (h == IntPtr.Zero) return 0;
        ulong c; QueryThreadCycleTime(h, out c); CloseHandle(h); return c;
    }
    public static long Start(int tid) {
        IntPtr h = OpenThread(0x0040, false, tid);
        if (h == IntPtr.Zero) return 0;
        IntPtr a; long r = 0;
        if (NtQueryInformationThread(h, 9, out a, IntPtr.Size, IntPtr.Zero) == 0) r = a.ToInt64();
        CloseHandle(h);
        return r;
    }
    public static string Name(int tid) {
        IntPtr h = OpenThread(0x0800, false, tid);
        if (h == IntPtr.Zero) return "?";
        IntPtr p; string n = "";
        if (GetThreadDescription(h, out p) >= 0 && p != IntPtr.Zero) { n = Marshal.PtrToStringUni(p); LocalFree(p); }
        CloseHandle(h);
        return n;
    }
    public static Dictionary<int, long> Sample(int pid) {
        var res = new Dictionary<int, long>();
        var data = new PerformanceCounterCategory("Thread").ReadCategory();
        var pids = data["ID Process"]; var tids = data["ID Thread"]; var cs = data["Context Switches/sec"];
        foreach (InstanceData d in pids.Values) {
            if (d.RawValue != pid) continue;
            int tid = (int)tids[d.InstanceName].RawValue;
            res[tid] = cs[d.InstanceName].RawValue;
        }
        return res;
    }
}
"@

function Thread-Cpu {
    $map = @{}
    foreach ($t in (Get-Process -Id $ProcId).Threads) { $map[$t.Id] = [double][Wake]::Cycles($t.Id) }
    return $map
}
$cpuA = Thread-Cpu
$a = [Wake]::Sample($ProcId)
Start-Sleep -Seconds $Seconds
$b = [Wake]::Sample($ProcId)
$cpuB = Thread-Cpu
$proc = Get-Process -Id $ProcId
$mods = $proc.Modules
function Start-Module($tid) {
    $addr = [Wake]::Start($tid)
    if ($addr -eq 0) { return "?" }
    foreach ($m in $mods) {
        $base = $m.BaseAddress.ToInt64()
        if ($addr -ge $base -and $addr -lt $base + $m.ModuleMemorySize) { return "{0}+0x{1:x}" -f $m.ModuleName, ($addr - $base) }
    }
    return "0x{0:x}" -f $addr
}
$proc.Refresh()
$rows = foreach ($tid in $b.Keys) {
    $prev = if ($a.ContainsKey($tid)) { $a[$tid] } else { 0 }
    $n = [Wake]::Name($tid)
    if (-not $n) { $n = "(" + (Start-Module $tid) + ")" }
    $cpuPrev = if ($cpuA.ContainsKey($tid)) { $cpuA[$tid] } else { 0 }
    $cpuNow = if ($cpuB.ContainsKey($tid)) { $cpuB[$tid] } else { $cpuPrev }
    # cpu_pct: percent of a 3 GHz core used by this thread over the window.
    [pscustomobject]@{ label = $Label; tid = $tid; name = $n; per_s = [math]::Round(($b[$tid] - $prev) / $Seconds, 2);
        cpu_pct = [math]::Round(($cpuNow - $cpuPrev) / $Seconds / 3.0e7, 2) }
}
$rows = $rows | Sort-Object per_s -Descending
$p = Get-Process -Id $ProcId
$total = ($rows | Measure-Object per_s -Sum).Sum
"# $Label  threads=$($rows.Count) total_wakeups_per_s=$total cpu_pct=$([math]::Round(($rows | Measure-Object cpu_pct -Sum).Sum, 2)) ws_mb=$([math]::Round($p.WorkingSet64/1MB,1)) private_mb=$([math]::Round($p.PrivateMemorySize64/1MB,1))" | Out-File -Append -Encoding utf8 $Out
$rows | Where-Object { $_.per_s -gt 0 } | Format-Table -AutoSize | Out-String -Width 200 | Out-File -Append -Encoding utf8 $Out
