# End-to-end voice test on one machine with two saved accounts.
#
# The sender's "microphone" is a loopback of the default output device; a synthesized
# reference speech is played there, travels through Discord and is decoded by the
# receiver, which writes what it got to disk. voice_compare.py then scores it.
#
#   powershell -File scripts\perf\voice_e2e.ps1 -Sender "<account>" -Receiver "<account>" `
#       -Server "<server name>" -Channel "<voice channel>"
#
# Both instances play nothing (LITECORD_TEST_MUTE_OUTPUT) so the loopback only hears
# the reference. Results land in -OutDir.
param(
    [Parameter(Mandatory)] [string]$Sender,
    [Parameter(Mandatory)] [string]$Receiver,
    [Parameter(Mandatory)] [string]$Server,
    [Parameter(Mandatory)] [string]$Channel,
    [string]$Exe = "",
    [string]$OutDir = "",
    [int]$SpeechRepeats = 3,
    # Send on the old 20 ms timer (with 1 ms resolution) instead of the microphone clock.
    [switch]$TimerPacing
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "lib.ps1")
if (-not $Exe) { $Exe = Join-Path $RepoRoot "target\release\litecord.exe" }
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP ("litecord-voice-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null

# Reference speech, rendered to a file so the comparison has the exact source.
Add-Type -AssemblyName System.Speech
$ref = Join-Path $OutDir "reference.wav"
$tts = New-Object System.Speech.Synthesis.SpeechSynthesizer
$tts.SetOutputToWaveFile($ref)
$text = "Teste de voz do Litecord. Um, dois, tres, quatro, cinco, seis, sete, oito, nove, dez. " +
        "O rato roeu a roupa do rei de Roma. Agora uma frase mais longa, sem pausas, para ver se a voz chega inteira do outro lado."
for ($i = 0; $i -lt $SpeechRepeats; $i++) { $tts.Speak($text) }
$tts.Dispose()

$senderEnv = @{
    LITECORD_TEST_ACCOUNT = $Sender; LITECORD_TEST_MIC = "loopback"; LITECORD_TEST_MUTE_OUTPUT = "1"
    LITECORD_TEST_VOICE_LOG = (Join-Path $OutDir "sender.csv")
    LITECORD_TEST_TX_PCM = (Join-Path $OutDir "captured.f32")
}
if ($TimerPacing) { $senderEnv.LITECORD_TEST_TIMER_PACING = "1" }
$senderProc = Start-Litecord $Exe (Join-Path $OutDir "sender") 47131 $senderEnv
$receiverProc = Start-Litecord $Exe (Join-Path $OutDir "receiver") 47132 @{
    LITECORD_TEST_ACCOUNT = $Receiver; LITECORD_TEST_MUTE_OUTPUT = "1"
    LITECORD_TEST_VOICE_LOG = (Join-Path $OutDir "receiver.csv")
    LITECORD_TEST_RX_PCM = (Join-Path $OutDir "received.f32")
}
try {
    $a = Wait-LoggedIn 47131; $b = Wait-LoggedIn 47132
    if ($a.user_id -eq $b.user_id) { throw "both instances logged in as the same account ($($a.user_tag))" }
    "sender:   $($a.user_tag)"; "receiver: $($b.user_tag)"
    Join-Voice 47132 $Server $Channel
    Join-Voice 47131 $Server $Channel
    Start-Sleep -Seconds 4
    "playing reference..."
    (New-Object System.Media.SoundPlayer $ref).PlaySync()
    Start-Sleep -Seconds 3
    Send-Litecord 47131 '{"cmd":"leave_voice"}' | Out-Null
    Send-Litecord 47132 '{"cmd":"leave_voice"}' | Out-Null
    Start-Sleep -Seconds 2
} finally {
    foreach ($p in $senderProc, $receiverProc) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force } }
}
python (Join-Path $PSScriptRoot "voice_compare.py") $OutDir
