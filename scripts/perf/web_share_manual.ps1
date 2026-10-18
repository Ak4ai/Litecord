# Run this first; if needed, copy the browser offer when prompted, then press Enter.
# Requires a local automation test build with LITECORD_WEB_MANUAL=1 and an active share.
param([ValidateRange(1, 65535)][int]$Port = 39091)
. (Join-Path $PSScriptRoot 'lib.ps1')
while ($true) {
    $offer = $null
    try {
        $candidate = Get-Clipboard -Raw | ConvertFrom-Json -ErrorAction Stop
        if ($candidate.type -eq 'offer' -and $candidate.sdp -is [string] -and -not [string]::IsNullOrWhiteSpace($candidate.sdp)) {
            $offer = $candidate
        }
    } catch {
        # Copying this command replaces the offer. Wait for a fresh copy instead
        # of exposing a JSON parsing error or echoing private SDP to the terminal.
    }
    if ($null -ne $offer) { break }
    Write-Host 'A área de transferência ainda não contém uma oferta WebRTC válida.'
    $choice = Read-Host 'No navegador, selecione TODO o texto do primeiro campo e copie. Volte aqui e pressione Enter (ou Q para sair)'
    if ($choice -match '^\s*q\s*$') { return }
}
$reply = Send-Litecord $Port (@{ cmd = 'web_share_answer'; offer = $offer } | ConvertTo-Json -Depth 8 -Compress)
if (-not $reply.ok) { throw $reply.error }
Set-Clipboard ($reply.answer | ConvertTo-Json -Depth 8 -Compress)
Write-Output 'Resposta copiada. Cole no segundo campo da página e clique em Conectar.'
