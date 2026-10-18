# End-to-end screen share test on one machine with two saved accounts: the sender shares
# its screen, the receiver watches it. Reports whether the stream opened and what
# watching cost the receiver (CPU, wakeups, FPS).
#
#   powershell -File scripts\perf\stream_e2e.ps1 -Sender "<account>" -Receiver "<account>" `
#       -Server "<server name>" -Channel "<voice channel>" [-ReceiverBackend winit-software]
param(
    [Parameter(Mandatory)] [string]$Sender,
    [Parameter(Mandatory)] [string]$Receiver,
    [Parameter(Mandatory)] [string]$Server,
    [Parameter(Mandatory)] [string]$Channel,
    [string]$Exe = "",
    [string]$OutDir = "",
    [string]$ReceiverBackend = "",   # SLINT_BACKEND for the receiver, e.g. winit-software
    [string]$Resolution = "1080p",
    [int]$Fps = 60,
    [int]$WatchSeconds = 30,
    [switch]$Fullscreen,            # watch with the stream full screen (the costliest view)
    [int]$DecodeDelayMs = 0,        # slow the receiver's decoder down to test falling behind
    [string]$DecodeStall = "",      # "<ms>,<every ms>": stall the receiver's decoder now and then
    [switch]$TrayCheck,             # then hide the receiver in the tray and bring it back
    [int]$ConversionAB = 0          # then <n> rounds alternating scaled / full-size conversion
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "lib.ps1")
if (-not $Exe) { $Exe = Join-Path $RepoRoot "target\release\litecord.exe" }
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP ("litecord-stream-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null

$receiverEnv = @{ LITECORD_TEST_ACCOUNT = $Receiver; LITECORD_TEST_MUTE_OUTPUT = "1" }
if ($ReceiverBackend) { $receiverEnv.SLINT_BACKEND = $ReceiverBackend }
if ($DecodeDelayMs -gt 0) { $receiverEnv.LITECORD_TEST_DECODE_DELAY_MS = "$DecodeDelayMs" }
if ($DecodeStall) { $receiverEnv.LITECORD_TEST_DECODE_STALL = $DecodeStall }
$senderProc = Start-Litecord $Exe (Join-Path $OutDir "sender") 47131 @{ LITECORD_TEST_ACCOUNT = $Sender; LITECORD_TEST_MUTE_OUTPUT = "1" }
$receiverProc = Start-Litecord $Exe (Join-Path $OutDir "receiver") 47132 $receiverEnv
try {
    $a = Wait-LoggedIn 47131; $b = Wait-LoggedIn 47132
    if ($a.user_id -eq $b.user_id) { throw "both instances logged in as the same account ($($a.user_tag))" }
    "sender:   $($a.user_tag)"; "receiver: $($b.user_tag) (backend: $(if ($ReceiverBackend) { $ReceiverBackend } else { 'default' }))"
    Join-Voice 47132 $Server $Channel
    Join-Voice 47131 $Server $Channel
    Start-Sleep -Seconds 3

    $r = Send-Litecord 47131 (@{ cmd = "start_screen_share"; res = $Resolution; fps = $Fps; screen = 0; audio = $false } | ConvertTo-Json -Compress)
    if (-not $r.ok) { throw "start_screen_share: $($r.error)" }
    $t0 = Get-Date; $opened = $null
    while (((Get-Date) - $t0).TotalSeconds -lt 30) {
        $s = (Send-Litecord 47132 '{"cmd":"state"}').stream
        if ($s.has_active_stream -or ($s.peers_streaming | Where-Object { "$_" -match $a.user_id })) { $opened = ((Get-Date) - $t0).TotalSeconds; break }
        Start-Sleep -Milliseconds 500
    }
    if ($null -eq $opened) { "RESULT: FAIL (the receiver never saw the stream)"; return }
    "stream visible to the receiver after $([math]::Round($opened, 1)) s"
    Send-Litecord 47132 (@{ cmd = "watch_stream"; user_id = "$($a.user_id)" } | ConvertTo-Json -Compress) | Out-Null
    if ($Fullscreen) {
        Send-Litecord 47132 '{"cmd":"resize","width":1920,"height":1080}' | Out-Null
        Start-Sleep -Seconds 2
        Send-Litecord 47132 '{"cmd":"stream_fullscreen"}' | Out-Null
    }
    Start-Sleep -Seconds 5
    "receiver window: $((Send-Litecord 47132 '{"cmd":"size"}').logical -join 'x')"

    $rx0 = [Cycles]::OfProcess($receiverProc.Id); $tx0 = [Cycles]::OfProcess($senderProc.Id)
    & (Join-Path $PSScriptRoot "wakeups.ps1") -ProcId $receiverProc.Id -Seconds $WatchSeconds -Label "receiver watching" -Out (Join-Path $OutDir "receiver-wakeups.txt")
    $rxCpu = Cpu-Percent $rx0 ([Cycles]::OfProcess($receiverProc.Id)) $WatchSeconds
    $txCpu = Cpu-Percent $tx0 ([Cycles]::OfProcess($senderProc.Id)) $WatchSeconds
    $s = (Send-Litecord 47132 '{"cmd":"state"}').stream
    $cores = [Environment]::ProcessorCount
    "receiver CPU while watching: $([math]::Round($rxCpu, 1))% of a 3 GHz core ($([math]::Round($rxCpu / $cores, 2))% of the machine), shown FPS: $($s.remote_fps)"
    "sender CPU while sharing:    $([math]::Round($txCpu, 1))% of a 3 GHz core, local FPS: $((Send-Litecord 47131 '{"cmd":"state"}').stream.local_fps)"
    Get-Content (Join-Path $OutDir "receiver-wakeups.txt") | Select-Object -First 12
    for ($i = 0; $i -lt $ConversionAB; $i++) {
        # Same content, alternating, so drift in what is on screen affects both sides alike.
        foreach ($full in $false, $true) {
            Send-Litecord 47132 (@{ cmd = "set_full_conversion"; on = $full } | ConvertTo-Json -Compress) | Out-Null
            Start-Sleep -Seconds 2
            $rx0 = [Cycles]::OfProcess($receiverProc.Id); $tx0 = [Cycles]::OfProcess($senderProc.Id)
            Start-Sleep -Seconds 10
            "conversion $(if ($full) { 'full size' } else { 'scaled   ' }): receiver $([math]::Round((Cpu-Percent $rx0 ([Cycles]::OfProcess($receiverProc.Id)) 10), 1))%, sender $([math]::Round((Cpu-Percent $tx0 ([Cycles]::OfProcess($senderProc.Id)) 10), 1))% of a 3 GHz core"
        }
    }
    Send-Litecord 47132 '{"cmd":"set_full_conversion","on":false}' | Out-Null
    if ($TrayCheck) {
        # Hidden (tray, minimized) nothing should be decoded and the picture must come back
        # once restored (from a keyframe the receiver asks for). With the popout open the
        # stream is still seen, so it keeps decoding while the main window is in the tray.
        function Stream-State { (Send-Litecord 47132 '{"cmd":"state"}').stream }
        function Check-Hidden([string]$hideCmd, [string]$label) {
            Send-Litecord 47132 "{`"cmd`":`"$hideCmd`"}" | Out-Null
            Start-Sleep -Seconds 3
            $rx0 = [Cycles]::OfProcess($receiverProc.Id); $s0 = Stream-State
            Start-Sleep -Seconds 10
            $s1 = Stream-State
            "${label}: receiver CPU $([math]::Round((Cpu-Percent $rx0 ([Cycles]::OfProcess($receiverProc.Id)) 10), 1))% of a 3 GHz core, pictures shown $($s1.frames_shown - $s0.frames_shown), skipped $($s1.frames_unwatched - $s0.frames_unwatched)"
            Send-Litecord 47132 '{"cmd":"restore"}' | Out-Null
            $t0 = Get-Date
            while (((Get-Date) - $t0).TotalSeconds -lt 10) {
                if ((Stream-State).frames_shown -gt $s1.frames_shown + 5) { return "  picture back $([math]::Round(((Get-Date) - $t0).TotalSeconds, 2)) s after restoring" }
                Start-Sleep -Milliseconds 100
            }
            throw "the picture did not come back after restoring from '$label'"
        }
        Check-Hidden "minimize_to_tray" "in the tray"
        Check-Hidden "minimize" "minimized"
        Send-Litecord 47132 (@{ cmd = "popout_stream"; user_id = "$($a.user_id)" } | ConvertTo-Json -Compress) | Out-Null
        Start-Sleep -Seconds 2
        $p = (Check-Hidden "minimize_to_tray" "popout open, main window in the tray") -join "`n"
        $p
        if ($p -notmatch "picture back") { throw "popout check failed" }
    }
    "RESULT: PASS (stream opened)"
} finally {
    try { Send-Litecord 47131 '{"cmd":"stop_screen_share"}' | Out-Null } catch { }
    foreach ($port in 47131, 47132) { try { Send-Litecord $port '{"cmd":"leave_voice"}' | Out-Null } catch { } }
    Start-Sleep -Seconds 2
    foreach ($p in $senderProc, $receiverProc) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force } }
}
