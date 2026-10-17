# Idle cost of one instance outside a call: wakeups per thread, CPU and memory with the
# window open and then hidden in the tray.
#
#   powershell -File scripts\perf\idle.ps1 -Account "<name>" [-Seconds 180] [-Backend winit-software]
param(
    [Parameter(Mandatory)] [string]$Account,
    [string]$Exe = "",
    [string]$OutDir = "",
    [string]$Backend = "",
    [int]$Seconds = 180
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "lib.ps1")
if (-not $Exe) { $Exe = Join-Path $RepoRoot "target\release\litecord.exe" }
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP ("litecord-idle-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$env_ = @{ LITECORD_TEST_ACCOUNT = $Account }
if ($Backend) { $env_.SLINT_BACKEND = $Backend }
$proc = Start-Litecord $Exe (Join-Path $OutDir "instance") 47131 $env_
$report = Join-Path $OutDir "wakeups.txt"
function Measure-Phase($label) {
    $proc.Refresh(); $cpu0 = $proc.TotalProcessorTime.TotalSeconds
    & (Join-Path $PSScriptRoot "wakeups.ps1") -ProcId $proc.Id -Seconds $Seconds -Label $label -Out $report
    $proc.Refresh()
    "{0}: CPU {1:N2}% of one core, working set {2:N1} MB, private {3:N1} MB" -f $label,
        (($proc.TotalProcessorTime.TotalSeconds - $cpu0) / $Seconds * 100),
        ($proc.WorkingSet64 / 1MB), ($proc.PrivateMemorySize64 / 1MB)
}
try {
    Wait-LoggedIn 47131 | Out-Null
    Start-Sleep -Seconds 20   # past the one-time trim after login
    Measure-Phase "window open"
    Send-Litecord 47131 '{"cmd":"minimize_to_tray"}' | Out-Null
    Start-Sleep -Seconds 5
    Measure-Phase "in the tray"
    Get-Content $report | Where-Object { $_ -match "^#" }
} finally {
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
}
