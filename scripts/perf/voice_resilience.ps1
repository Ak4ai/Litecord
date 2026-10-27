# Received voice that stops after a while (#64): two instances in a call, each sending
# a generated tone (LITECORD_TEST_MIC=synth) and logging what it decodes and plays,
# through the events that can break reception:
#
#   steady         both in the call
#   peer-rejoin    the sender leaves and comes back (new MLS epochs for the receiver)
#   self-rejoin    the receiver leaves and comes back
#   gateway-drop   the receiver's main gateway connection drops and resumes
#   output-break   the receiver's output stream stops, as a failing device would
#   long           a long stretch with nothing happening
#
#   powershell -File scripts\perf\voice_resilience.ps1 -Sender "<account>" -Receiver "<account>" `
#       -Server "<server name>" -Channel "<voice channel>" [-LongMinutes 30]
#
# Nothing is played aloud (LITECORD_TEST_SILENT_PLAYBACK). voice_resilience.py scores the run.
param(
    [Parameter(Mandatory)] [string]$Sender,
    [Parameter(Mandatory)] [string]$Receiver,
    [Parameter(Mandatory)] [string]$Server,
    [Parameter(Mandatory)] [string]$Channel,
    [string]$Exe = "",
    [string]$OutDir = "",
    [int]$SteadyMinutes = 3,
    [int]$LongMinutes = 30,
    [int]$Rejoins = 5,
    # Only the steady, output-break and long phases.
    [switch]$SkipRejoins
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "lib.ps1")
if (-not $Exe) { $Exe = Join-Path $RepoRoot "target\release\litecord.exe" }
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP ("litecord-voice-resilience-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$phases = Join-Path $OutDir "phases.csv"
"unix_ms,phase,event" | Set-Content -Encoding ascii $phases

function Mark([string]$Phase, [string]$Event) {
    $ms = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    "$ms,$Phase,$Event" | Add-Content -Encoding ascii $phases
    "{0:HH:mm:ss} {1} {2}" -f (Get-Date), $Phase, $Event
}

$script:instances = @{}
function Start-Instance([string]$Role, [string]$Account, [int]$Port, [hashtable]$Extra = @{}) {
    $n = 1 + ($script:instances.Keys | Where-Object { $_ -like "$Role*" }).Count
    $name = "$Role-$n"
    $env_ = @{
        LITECORD_TEST_ACCOUNT = $Account
        LITECORD_TEST_MIC = "synth"
        LITECORD_TEST_SILENT_PLAYBACK = "1"
        LITECORD_TEST_VOICE_LOG = (Join-Path $OutDir "$name.csv")
    } + $Extra
    $p = Start-Litecord $Exe (Join-Path $OutDir $name) $Port $env_
    $script:instances[$name] = $p
    Wait-LoggedIn $Port | Out-Null
    return $p
}
function Stop-Instance($p, [int]$Port) {
    try { Send-Litecord $Port '{"cmd":"leave_voice"}' | Out-Null } catch { }
    Start-Sleep 2
    if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    Start-Sleep 2
}

$S = 47131; $R = 47132
try {
    $rx = Start-Instance "receiver" $Receiver $R
    $tx = Start-Instance "sender" $Sender $S
    Join-Voice $R $Server $Channel
    Join-Voice $S $Server $Channel

    Mark "steady" "start"; Start-Sleep ($SteadyMinutes * 60); Mark "steady" "end"

    if (-not $SkipRejoins) {
        for ($i = 1; $i -le $Rejoins; $i++) {
            Mark "peer-rejoin" "leave $i"
            Send-Litecord $S '{"cmd":"leave_voice"}' | Out-Null
            Start-Sleep 8
            Join-Voice $S $Server $Channel
            Mark "peer-rejoin" "joined $i"
            Start-Sleep 25
        }
        Mark "peer-rejoin" "end"

        for ($i = 1; $i -le 3; $i++) {
            Mark "self-rejoin" "leave $i"
            Send-Litecord $R '{"cmd":"leave_voice"}' | Out-Null
            Start-Sleep 8
            Join-Voice $R $Server $Channel
            Mark "self-rejoin" "joined $i"
            Start-Sleep 25
        }
        Mark "self-rejoin" "end"

        for ($i = 1; $i -le 2; $i++) {
            Mark "gateway-drop" "drop $i"
            Send-Litecord $R '{"cmd":"drop_gateway"}' | Out-Null
            Start-Sleep 30
        }
        Mark "gateway-drop" "end"
    }

    # Calls without DAVE cannot be tested: Discord closes the voice connection of a
    # client without it (4017, "E2EE/DAVE protocol required").
    Mark "output-break" "break"
    Send-Litecord $R '{"cmd":"break_voice_output"}' | Out-Null
    Start-Sleep 30
    Mark "output-break" "end"
    # Rejoining starts a new output stream, so the long stretch begins from a working one.
    Send-Litecord $R '{"cmd":"leave_voice"}' | Out-Null
    Start-Sleep 5
    Join-Voice $R $Server $Channel
    Start-Sleep 10

    Mark "long" "start"; Start-Sleep ($LongMinutes * 60); Mark "long" "end"
} finally {
    foreach ($port in $S, $R) { try { Send-Litecord $port '{"cmd":"leave_voice"}' | Out-Null } catch { } }
    Start-Sleep 2
    foreach ($p in $script:instances.Values) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force } }
}
python (Join-Path $PSScriptRoot "voice_resilience.py") $OutDir
