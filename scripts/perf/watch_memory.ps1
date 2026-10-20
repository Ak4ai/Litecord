# Receiver memory while a stream is hidden and watched again (or popped out and back)
# many times. Each cycle used to leave 30-50 MB behind in the video driver (the GPU
# device was released before what was made from it). The memory may swing while the driver frees
# what was released, but must not keep growing: fails when even the lowest of the last
# five samples is more than 100 MB above the first.
#
#   powershell -File scripts\perf\watch_memory.ps1 -Sender "<account>" -Receiver "<account>" `
#       -Server "<server name>" -Channel "<voice channel>" [-Mode watch|popout|tray] [-Cycles 20]
param(
    [Parameter(Mandatory)] [string]$Sender,
    [Parameter(Mandatory)] [string]$Receiver,
    [Parameter(Mandatory)] [string]$Server,
    [Parameter(Mandatory)] [string]$Channel,
    [ValidateSet("watch", "popout", "tray")] [string]$Mode = "watch",
    [int]$Cycles = 20,
    [string]$Exe = "",
    [string]$OutDir = ""
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "lib.ps1")
if (-not $Exe) { $Exe = Join-Path $RepoRoot "target\release\litecord.exe" }
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP ("litecord-watchmem-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$senderProc = Start-Litecord $Exe (Join-Path $OutDir "sender") 47131 @{ LITECORD_TEST_ACCOUNT = $Sender; LITECORD_TEST_MUTE_OUTPUT = "1" }
$receiverProc = Start-Litecord $Exe (Join-Path $OutDir "receiver") 47132 @{ LITECORD_TEST_ACCOUNT = $Receiver; LITECORD_TEST_MUTE_OUTPUT = "1" }
function Mem { $receiverProc.Refresh(); $receiverProc.PrivateMemorySize64 / 1MB }
try {
    $a = Wait-LoggedIn 47131; Wait-LoggedIn 47132 | Out-Null
    Join-Voice 47132 $Server $Channel
    Join-Voice 47131 $Server $Channel
    Start-Sleep -Seconds 3
    Send-Litecord 47131 (@{ cmd = "start_screen_share"; res = "1080p"; fps = 60; screen = 0; audio = $false } | ConvertTo-Json -Compress) | Out-Null
    Start-Sleep -Seconds 5
    $watch = @{ cmd = "watch_stream"; user_id = "$($a.user_id)" } | ConvertTo-Json -Compress
    Send-Litecord 47132 $watch | Out-Null
    Start-Sleep -Seconds 15
    $start = Mem
    "start: {0:F1} MB private" -f $start
    $samples = @()
    for ($i = 1; $i -le $Cycles; $i++) {
        switch ($Mode) {
            "watch" { Send-Litecord 47132 (@{ cmd = "unwatch_stream"; user_id = "$($a.user_id)" } | ConvertTo-Json -Compress) | Out-Null }
            "popout" { Send-Litecord 47132 (@{ cmd = "popout_stream"; user_id = "$($a.user_id)" } | ConvertTo-Json -Compress) | Out-Null }
            "tray" { Send-Litecord 47132 '{"cmd":"minimize_to_tray"}' | Out-Null }
        }
        Start-Sleep -Seconds 5
        if ($Mode -eq "tray") { Send-Litecord 47132 '{"cmd":"restore"}' | Out-Null } else { Send-Litecord 47132 $watch | Out-Null }
        Start-Sleep -Seconds 5
        $m = Mem; $samples += $m
        "after {0,2}: {1:F1} MB" -f $i, $m
    }
    $floor = ($samples | Select-Object -Last 5 | Measure-Object -Minimum).Minimum
    $shown = (Send-Litecord 47132 '{"cmd":"state"}').stream.frames_shown
    if ($floor - $start -gt 100) { "RESULT: FAIL - the lowest of the last five samples is {0:F0} MB above the start" -f ($floor - $start) }
    elseif ($shown -le 0) { "RESULT: FAIL - no picture shown" }
    else { "RESULT: PASS ({0:+0;-0} MB at the floor of the last five samples); logs: $OutDir" -f ($floor - $start) }
} finally {
    try { Send-Litecord 47131 '{"cmd":"stop_screen_share"}' | Out-Null } catch { }
    foreach ($port in 47131, 47132) { try { Send-Litecord $port '{"cmd":"leave_voice"}' | Out-Null } catch { } }
    Start-Sleep -Seconds 2
    foreach ($p in $senderProc, $receiverProc) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force } }
}
