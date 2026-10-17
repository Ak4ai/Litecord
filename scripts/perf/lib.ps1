# Shared helpers for the performance and end-to-end test scripts in this folder.
# They drive test builds (`cargo build --release --features automation`) through the
# localhost automation port; see src/automation.rs and src/utils/test_hooks.rs.

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path

function Send-Litecord([int]$Port, [string]$Json) {
    $client = New-Object System.Net.Sockets.TcpClient("127.0.0.1", $Port)
    try {
        $stream = $client.GetStream()
        $writer = New-Object System.IO.StreamWriter($stream)
        $reader = New-Object System.IO.StreamReader($stream)
        $writer.WriteLine($Json); $writer.Flush()
        return ($reader.ReadLine() | ConvertFrom-Json)
    } finally {
        $client.Close()
    }
}

# Starts one Litecord instance in its own working directory (so its settings and log
# stay apart) and returns the process. $Env is a hashtable of extra environment variables.
function Start-Litecord([string]$Exe, [string]$WorkDir, [int]$Port, [hashtable]$Env = @{}) {
    New-Item -ItemType Directory -Force $WorkDir | Out-Null
    $saved = @{}
    $vars = @{ LITECORD_AUTOMATION_PORT = "$Port"; PATH = "$RepoRoot;$env:PATH" } + $Env
    foreach ($k in $vars.Keys) {
        $saved[$k] = [Environment]::GetEnvironmentVariable($k, "Process")
        [Environment]::SetEnvironmentVariable($k, $vars[$k], "Process")
    }
    try {
        return Start-Process -FilePath $Exe -WorkingDirectory $WorkDir -PassThru
    } finally {
        foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k], "Process") }
    }
}

function Wait-LoggedIn([int]$Port, [int]$TimeoutSeconds = 90) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        try {
            $s = Send-Litecord $Port '{"cmd":"state"}'
            # user_id comes from the gateway READY; the tag alone may still be the vault's.
            if ($s.is_logged_in -and $s.user_id -and $s.user_id -ne "0") { return $s }
        } catch { }
        Start-Sleep -Seconds 2
    }
    throw "instance on port $Port did not log in within $TimeoutSeconds s"
}

# Opens the server and joins the voice channel by name; refuses to stay anywhere else.
function Join-Voice([int]$Port, [string]$Server, [string]$Channel) {
    $r = Send-Litecord $Port (@{ cmd = "open_guild"; name = $Server } | ConvertTo-Json -Compress)
    if (-not $r.ok) { throw "port ${Port}: $($r.error)" }
    Start-Sleep -Seconds 4
    $r = Send-Litecord $Port (@{ cmd = "open_channel"; name = $Channel } | ConvertTo-Json -Compress)
    if (-not $r.ok) { throw "port ${Port}: $($r.error)" }
    if (-not $r.is_voice) { throw "port ${Port}: '$Channel' is not a voice channel" }
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline) {
        $v = (Send-Litecord $Port '{"cmd":"state"}').voice
        if ($v.is_in_voice -and -not $v.is_connecting -and $v.channel -eq $Channel) { return }
        Start-Sleep -Seconds 1
    }
    Send-Litecord $Port '{"cmd":"leave_voice"}' | Out-Null
    throw "port ${Port}: did not connect to '$Channel'"
}
