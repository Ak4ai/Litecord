# Only the sender ends its stream: while the receiver watches, a forged OP_STOP (no proof
# made with the stream key) must not take the picture away; when the sender really stops,
# the receiver must close the stream within a second.
#
#   powershell -File scripts\perf\stream_stop_auth.ps1 -Sender "<account>" -Receiver "<account>" `
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
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP ("litecord-stopauth-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$senderProc = Start-Litecord $Exe (Join-Path $OutDir "sender") 47131 @{ LITECORD_TEST_ACCOUNT = $Sender; LITECORD_TEST_MUTE_OUTPUT = "1" }
$receiverProc = Start-Litecord $Exe (Join-Path $OutDir "receiver") 47132 @{ LITECORD_TEST_ACCOUNT = $Receiver; LITECORD_TEST_MUTE_OUTPUT = "1" }
function State { Send-Litecord 47132 '{"cmd":"state"}' }
$failures = @()
try {
    $a = Wait-LoggedIn 47131; Wait-LoggedIn 47132 | Out-Null
    Join-Voice 47132 $Server $Channel
    Join-Voice 47131 $Server $Channel
    Start-Sleep -Seconds 3
    Send-Litecord 47131 (@{ cmd = "start_screen_share"; res = "1080p"; fps = 60; screen = 0; audio = $false } | ConvertTo-Json -Compress) | Out-Null
    Start-Sleep -Seconds 4
    Send-Litecord 47132 (@{ cmd = "watch_stream"; user_id = "$($a.user_id)" } | ConvertTo-Json -Compress) | Out-Null
    Start-Sleep -Seconds 4

    # What an attacker would know: the channel, the sender's id and the P2P port range.
    $cid = [uint64](State).voice.channel_id
    if (-not $cid) { throw "receiver channel not found" }
    $pkt = [byte[]](@(0x4C, 0x54, 0x50, 0x56) + [BitConverter]::GetBytes([uint32]0x12345678)[3..0] + @(3) +
        [BitConverter]::GetBytes($cid)[7..0] + [BitConverter]::GetBytes([uint64]$a.user_id)[7..0])
    $udp = New-Object System.Net.Sockets.UdpClient
    for ($i = 0; $i -lt 20; $i++) {
        foreach ($port in 50005..50015) { [void]$udp.Send($pkt, $pkt.Length, "127.0.0.1", $port) }
        Start-Sleep -Milliseconds 20
    }
    $udp.Close()
    "forged OP_STOP x20 sent to ports 50005-50015"
    $f0 = (State).stream.frames_shown; Start-Sleep -Seconds 3; $n = (State).stream.frames_shown - $f0
    $active = (State).stream.has_active_stream
    "after the forged stop: $n pictures in 3 s, stream active: $active"
    if ($n -lt 120 -or -not $active) { $failures += "a forged OP_STOP ended the stream ($n pictures, active $active)" }

    Send-Litecord 47131 '{"cmd":"stop_screen_share"}' | Out-Null
    $t0 = Get-Date; $closed = -1
    while (((Get-Date) - $t0).TotalSeconds -lt 5) {
        if (-not (State).stream.has_active_stream) { $closed = ((Get-Date) - $t0).TotalSeconds; break }
        Start-Sleep -Milliseconds 50
    }
    "real stop: stream closed on the receiver after {0:F2} s" -f $closed
    if ($closed -lt 0 -or $closed -gt 1.0) { $failures += "the real stop took $closed s" }
    if ($failures.Count) { "RESULT: FAIL - " + ($failures -join "; ") } else { "RESULT: PASS; logs: $OutDir" }
} finally {
    try { Send-Litecord 47131 '{"cmd":"stop_screen_share"}' | Out-Null } catch { }
    foreach ($port in 47131, 47132) { try { Send-Litecord $port '{"cmd":"leave_voice"}' | Out-Null } catch { } }
    Start-Sleep -Seconds 2
    foreach ($p in $senderProc, $receiverProc) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force } }
}
