# Watching on demand, end to end (docs/WATCH_ON_DEMAND.md): the sender shares its screen
# and the receiver does not watch, then watches, hides the stream, watches again and goes
# to the tray and back. Checks that nothing is received or encoded while nobody watches,
# that the picture arrives when watching, and how long it takes to come back.
#
#   powershell -File scripts\perf\watch_on_demand.ps1 -Sender "<account>" -Receiver "<account>" `
#       -Server "<server name>" -Channel "<voice channel>"
param(
    [Parameter(Mandatory)] [string]$Sender,
    [Parameter(Mandatory)] [string]$Receiver,
    [Parameter(Mandatory)] [string]$Server,
    [Parameter(Mandatory)] [string]$Channel,
    [string]$Exe = "",
    [string]$OutDir = ""
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "lib.ps1")
if (-not $Exe) { $Exe = Join-Path $RepoRoot "target\release\litecord.exe" }
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP ("litecord-watch-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null

$senderProc = Start-Litecord $Exe (Join-Path $OutDir "sender") 47131 @{ LITECORD_TEST_ACCOUNT = $Sender; LITECORD_TEST_MUTE_OUTPUT = "1" }
$receiverProc = Start-Litecord $Exe (Join-Path $OutDir "receiver") 47132 @{ LITECORD_TEST_ACCOUNT = $Receiver; LITECORD_TEST_MUTE_OUTPUT = "1" }
function Shown { (Send-Litecord 47132 '{"cmd":"state"}').stream.frames_shown }
# Pictures shown over a few seconds, and the sender's CPU meanwhile.
function Window([string]$Label, [int]$Seconds = 6) {
    $c0 = [Cycles]::OfProcess($senderProc.Id); $f0 = Shown
    Start-Sleep -Seconds $Seconds
    $cpu = Cpu-Percent $c0 ([Cycles]::OfProcess($senderProc.Id)) $Seconds
    $n = (Shown) - $f0
    Write-Host ("{0,-34} pictures {1,4} in {2} s, sender CPU {3:F1}% of a 3 GHz core" -f $Label, $n, $Seconds, $cpu)
    return $n
}
# Seconds until the receiver shows new pictures again.
function Picture-Back {
    $f0 = Shown; $t0 = Get-Date
    while (((Get-Date) - $t0).TotalSeconds -lt 10) {
        if ((Shown) -gt $f0 + 1) { return ((Get-Date) - $t0).TotalSeconds }
        Start-Sleep -Milliseconds 50
    }
    return -1
}
$failures = @()
try {
    $a = Wait-LoggedIn 47131; $b = Wait-LoggedIn 47132
    Join-Voice 47132 $Server $Channel
    Join-Voice 47131 $Server $Channel
    Start-Sleep -Seconds 3
    $r = Send-Litecord 47131 (@{ cmd = "start_screen_share"; res = "1080p"; fps = 60; screen = 0; audio = $false } | ConvertTo-Json -Compress)
    if (-not $r.ok) { throw "start_screen_share: $($r.error)" }
    Start-Sleep -Seconds 6
    $watch = @{ cmd = "watch_stream"; user_id = "$($a.user_id)" } | ConvertTo-Json -Compress
    $unwatch = @{ cmd = "unwatch_stream"; user_id = "$($a.user_id)" } | ConvertTo-Json -Compress

    $n = Window "not watching"
    Send-Litecord 47132 (@{ cmd = "screenshot"; path = (Join-Path $OutDir "receiver_not_watching.png") } | ConvertTo-Json -Compress) | Out-Null
    if ($n -ne 0) { $failures += "pictures arrived without watching ($n)" }

    Send-Litecord 47132 $watch | Out-Null
    $t = Picture-Back; Write-Host ("  first picture {0:F2} s after Watch" -f $t)
    if ($t -lt 0) { $failures += "no picture after Watch" }
    $n = Window "watching"
    if ($n -lt 250) { $failures += "watching: only $n pictures" }

    Send-Litecord 47132 $unwatch | Out-Null
    Start-Sleep -Seconds 1
    $n = Window "hidden (stopped watching)"
    if ($n -ne 0) { $failures += "pictures kept arriving after hiding ($n)" }

    Send-Litecord 47132 $watch | Out-Null
    $t = Picture-Back; Write-Host ("  picture back {0:F2} s after Watch again" -f $t)
    if ($t -lt 0) { $failures += "no picture after watching again" }

    Send-Litecord 47132 '{"cmd":"minimize_to_tray"}' | Out-Null
    # The request lapses after 4 s: then the sender pauses.
    Start-Sleep -Seconds 5
    $c0 = [Cycles]::OfProcess($senderProc.Id); Start-Sleep -Seconds 6
    "{0,-34} sender CPU {1:F1}% of a 3 GHz core" -f "receiver in the tray", (Cpu-Percent $c0 ([Cycles]::OfProcess($senderProc.Id)) 6)
    Send-Litecord 47132 '{"cmd":"restore"}' | Out-Null
    $t = Picture-Back; Write-Host ("  picture back {0:F2} s after leaving the tray" -f $t)
    if ($t -lt 0) { $failures += "no picture after leaving the tray" }

    foreach ($side in "sender", "receiver") {
        Get-Content (Join-Path $OutDir "$side\litecord_app.log") -Encoding UTF8 |
            Select-String -Pattern "ASSISTIR SOB DEMANDA|ABR\]" | Select-Object -Last 12 | ForEach-Object { "  [$side] " + $_.Line.Substring([Math]::Min(14, $_.Line.Length)) }
    }
    if ($failures.Count) { "RESULT: FAIL - " + ($failures -join "; ") } else { "RESULT: PASS; logs: $OutDir" }
} finally {
    try { Send-Litecord 47131 '{"cmd":"stop_screen_share"}' | Out-Null } catch { }
    foreach ($port in 47131, 47132) { try { Send-Litecord $port '{"cmd":"leave_voice"}' | Out-Null } catch { } }
    Start-Sleep -Seconds 2
    foreach ($p in $senderProc, $receiverProc) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force } }
}
