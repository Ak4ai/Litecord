# Long session: the sender shares its screen and the receiver watches it for a long time.
# Every few minutes it samples CPU (cycles), memory and pictures shown on both sides,
# and between samples it can send the receiver to the tray and back and open and close
# the popout, so the GPU decoder and video layers are created and destroyed many times.
# Keeps the machine and the display awake while it runs. Writes a CSV.
#
#   powershell -File scripts\perf\soak.ps1 -Sender "<account>" -Receiver "<account>" `
#       -Server "<server name>" -Channel "<voice channel>" [-Minutes 120] [-Cycle]
param(
    [Parameter(Mandatory)] [string]$Sender,
    [Parameter(Mandatory)] [string]$Receiver,
    [Parameter(Mandatory)] [string]$Server,
    [Parameter(Mandatory)] [string]$Channel,
    [string]$Exe = "",
    [string]$OutDir = "",
    [int]$Minutes = 120,
    [int]$SampleMinutes = 5,
    [switch]$Cycle
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "lib.ps1")
if (-not $Exe) { $Exe = Join-Path $RepoRoot "target\release\litecord.exe" }
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP ("litecord-soak-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$csv = Join-Path $OutDir "soak.csv"

Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Awake { [DllImport("kernel32.dll")] public static extern uint SetThreadExecutionState(uint f); }
"@
# ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED, undone at the end.
[Awake]::SetThreadExecutionState([uint32]"0x80000003") | Out-Null

$senderProc = Start-Litecord $Exe (Join-Path $OutDir "sender") 47131 @{ LITECORD_TEST_ACCOUNT = $Sender; LITECORD_TEST_MUTE_OUTPUT = "1" }
$receiverProc = Start-Litecord $Exe (Join-Path $OutDir "receiver") 47132 @{ LITECORD_TEST_ACCOUNT = $Receiver; LITECORD_TEST_MUTE_OUTPUT = "1" }
function Stream-State { (Send-Litecord 47132 '{"cmd":"state"}').stream }
try {
    $a = Wait-LoggedIn 47131; $b = Wait-LoggedIn 47132
    Join-Voice 47132 $Server $Channel
    Join-Voice 47131 $Server $Channel
    Start-Sleep -Seconds 3
    $r = Send-Litecord 47131 (@{ cmd = "start_screen_share"; res = "1080p"; fps = 60; screen = 0; audio = $false } | ConvertTo-Json -Compress)
    if (-not $r.ok) { throw "start_screen_share: $($r.error)" }
    Start-Sleep -Seconds 5
    Send-Litecord 47132 (@{ cmd = "watch_stream"; user_id = "$($a.user_id)" } | ConvertTo-Json -Compress) | Out-Null
    Start-Sleep -Seconds 10

    "minute,receiver_cpu,sender_cpu,receiver_ws_mb,receiver_private_mb,sender_ws_mb,sender_private_mb,pictures,gpu_pictures,skipped,remote_fps,cycles_done" | Out-File -Encoding ascii $csv
    $start = Get-Date
    $cycles = 0
    while (((Get-Date) - $start).TotalMinutes -lt $Minutes) {
        if ($Cycle) {
            # Tray and back, then popout open and closed: the GPU objects come and go.
            Send-Litecord 47132 '{"cmd":"minimize_to_tray"}' | Out-Null
            Start-Sleep -Seconds 5
            Send-Litecord 47132 '{"cmd":"restore"}' | Out-Null
            Start-Sleep -Seconds 5
            Send-Litecord 47132 (@{ cmd = "popout_stream"; user_id = "$($a.user_id)" } | ConvertTo-Json -Compress) | Out-Null
            Start-Sleep -Seconds 5
            Send-Litecord 47132 (@{ cmd = "watch_stream"; user_id = "$($a.user_id)" } | ConvertTo-Json -Compress) | Out-Null
            $cycles++
        }
        Start-Sleep -Seconds 10
        $rx0 = [Cycles]::OfProcess($receiverProc.Id); $tx0 = [Cycles]::OfProcess($senderProc.Id); $s0 = Stream-State
        Start-Sleep -Seconds 30
        $rxCpu = Cpu-Percent $rx0 ([Cycles]::OfProcess($receiverProc.Id)) 30
        $txCpu = Cpu-Percent $tx0 ([Cycles]::OfProcess($senderProc.Id)) 30
        $s1 = Stream-State
        $receiverProc.Refresh(); $senderProc.Refresh()
        # Invariant formatting: the locale's decimal comma would break the CSV.
        $row = [string]::Format([cultureinfo]::InvariantCulture, "{0:F1},{1:F1},{2:F1},{3:F1},{4:F1},{5:F1},{6:F1},{7},{8},{9},{10},{11}",
            ((Get-Date) - $start).TotalMinutes, $rxCpu, $txCpu,
            ($receiverProc.WorkingSet64 / 1MB), ($receiverProc.PrivateMemorySize64 / 1MB), ($senderProc.WorkingSet64 / 1MB), ($senderProc.PrivateMemorySize64 / 1MB),
            ($s1.frames_shown - $s0.frames_shown), ($s1.frames_gpu - $s0.frames_gpu), ($s1.frames_unwatched - $s0.frames_unwatched), ($s1.remote_fps -replace ' FPS', ''), $cycles)
        $row | Out-File -Append -Encoding ascii $csv
        $row
        # Nothing shown in a sample means the stream or the call is gone: what follows would measure nothing.
        if ($s1.frames_shown -eq $s0.frames_shown) { throw "the stream stopped arriving (no pictures in 30 s)" }
        $wait = $SampleMinutes * 60 - 40 - ($(if ($Cycle) { 15 } else { 0 }))
        if ($wait -gt 0) { Start-Sleep -Seconds $wait }
        if ($receiverProc.HasExited -or $senderProc.HasExited) { throw "an instance exited" }
    }
    "RESULT: PASS ($Minutes minutes, $cycles tray/popout cycles); CSV: $csv"
} finally {
    try { Send-Litecord 47131 '{"cmd":"stop_screen_share"}' | Out-Null } catch { }
    foreach ($port in 47131, 47132) { try { Send-Litecord $port '{"cmd":"leave_voice"}' | Out-Null } catch { } }
    Start-Sleep -Seconds 2
    foreach ($p in $senderProc, $receiverProc) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force } }
    [Awake]::SetThreadExecutionState([uint32]"0x80000000") | Out-Null
}
