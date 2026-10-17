# Idle cost of one instance sitting in a voice call: wakeups per thread and CPU with the
# window open, then hidden in the tray, then after leaving the call.
#
#   powershell -File scripts\perf\call_idle.ps1 -Account "<name>" -Server "<server>" -Channel "<voice channel>"
param(
    [Parameter(Mandatory)] [string]$Account,
    [Parameter(Mandatory)] [string]$Server,
    [Parameter(Mandatory)] [string]$Channel,
    [string]$Exe = "",
    [string]$OutDir = "",
    [string]$Backend = "",
    [int]$Seconds = 60
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "lib.ps1")
if (-not $Exe) { $Exe = Join-Path $RepoRoot "target\release\litecord.exe" }
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP ("litecord-call-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$env_ = @{ LITECORD_TEST_ACCOUNT = $Account }
if ($Backend) { $env_.SLINT_BACKEND = $Backend }
$proc = Start-Litecord $Exe (Join-Path $OutDir "instance") 47131 $env_
$report = Join-Path $OutDir "wakeups.txt"
function Measure-Phase($label) {
    $proc.Refresh(); $cpu0 = $proc.TotalProcessorTime.TotalSeconds
    & (Join-Path $PSScriptRoot "wakeups.ps1") -ProcId $proc.Id -Seconds $Seconds -Label $label -Out $report
    $proc.Refresh()
    "{0}: CPU {1:N1}% of one core" -f $label, (($proc.TotalProcessorTime.TotalSeconds - $cpu0) / $Seconds * 100)
}
try {
    Wait-LoggedIn 47131 | Out-Null
    Join-Voice 47131 $Server $Channel
    Start-Sleep -Seconds 5
    Measure-Phase "in call, window open"
    Send-Litecord 47131 '{"cmd":"minimize_to_tray"}' | Out-Null
    Start-Sleep -Seconds 5
    Measure-Phase "in call, in the tray"
    Send-Litecord 47131 '{"cmd":"leave_voice"}' | Out-Null
    Start-Sleep -Seconds 5
    Measure-Phase "after leaving, in the tray"
    Get-Content $report | Where-Object { $_ -match "^#|main-ui|tokio|signaling|cpal|screen-capture|timer|igxel|nvogl" }
} finally {
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
}
