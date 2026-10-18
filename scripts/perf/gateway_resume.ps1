# A dropped gateway connection must not end the call: the sender shares its screen, the
# receiver watches, then both drop their gateway connection (like a network loss or
# Discord's "reconnect" request). Discord removes a user from voice about 1m45s after the
# session that joined is gone, so this waits longer than that and checks that both are
# still in the call and the stream still arrives.
#
#   powershell -File scripts\perf\gateway_resume.ps1 -Sender "<account>" -Receiver "<account>" `
#       -Server "<server name>" -Channel "<voice channel>"
param(
    [Parameter(Mandatory)] [string]$Sender,
    [Parameter(Mandatory)] [string]$Receiver,
    [Parameter(Mandatory)] [string]$Server,
    [Parameter(Mandatory)] [string]$Channel,
    [string]$Exe = "",
    [string]$OutDir = "",
    [int]$WaitSeconds = 150
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "lib.ps1")
if (-not $Exe) { $Exe = Join-Path $RepoRoot "target\release\litecord.exe" }
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP ("litecord-resume-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null

$senderProc = Start-Litecord $Exe (Join-Path $OutDir "sender") 47131 @{ LITECORD_TEST_ACCOUNT = $Sender; LITECORD_TEST_MUTE_OUTPUT = "1" }
$receiverProc = Start-Litecord $Exe (Join-Path $OutDir "receiver") 47132 @{ LITECORD_TEST_ACCOUNT = $Receiver; LITECORD_TEST_MUTE_OUTPUT = "1" }
function Stream-State { (Send-Litecord 47132 '{"cmd":"state"}').stream }
function In-Call([int]$Port) {
    $v = (Send-Litecord $Port '{"cmd":"state"}').voice
    return ($v.is_in_voice -and $v.channel -eq $Channel)
}
$failures = @()
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
    $s0 = Stream-State; Start-Sleep -Seconds 5; $s1 = Stream-State
    "before: $($s1.frames_shown - $s0.frames_shown) pictures in 5 s"

    Send-Litecord 47131 '{"cmd":"drop_gateway"}' | Out-Null
    Send-Litecord 47132 '{"cmd":"drop_gateway"}' | Out-Null
    "gateway dropped on both; waiting $WaitSeconds s"
    Start-Sleep -Seconds $WaitSeconds

    foreach ($p in @(@{ port = 47131; name = "sender" }, @{ port = 47132; name = "receiver" })) {
        if (-not (In-Call $p.port)) { $failures += "$($p.name) left the call" }
    }
    $s0 = Stream-State; Start-Sleep -Seconds 5; $s1 = Stream-State
    $shown = $s1.frames_shown - $s0.frames_shown
    "after: $shown pictures in 5 s"
    if ($shown -lt 150) { $failures += "stream stalled after the drop ($shown pictures in 5 s)" }
    foreach ($side in "sender", "receiver") {
        $log = Get-Content (Join-Path $OutDir "$side\litecord_app.log") -Raw
        if ($log -notmatch "retomada; chamada de voz") { $failures += "$side did not resume its gateway session" }
    }
    if ($failures.Count) { "RESULT: FAIL - " + ($failures -join "; ") } else { "RESULT: PASS; logs: $OutDir" }
} finally {
    try { Send-Litecord 47131 '{"cmd":"stop_screen_share"}' | Out-Null } catch { }
    foreach ($port in 47131, 47132) { try { Send-Litecord $port '{"cmd":"leave_voice"}' | Out-Null } catch { } }
    Start-Sleep -Seconds 2
    foreach ($p in $senderProc, $receiverProc) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force } }
}
