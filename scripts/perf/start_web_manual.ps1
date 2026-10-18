# Local interactive test build. Web SDP is manual; native signaling is unchanged.
param([int]$Port = 39091)
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$exe = Join-Path $repo 'target\web-share-e2e\litecord-web-test.exe'
if (-not (Test-Path -LiteralPath $exe)) { throw 'Build de teste ausente: target/web-share-e2e/litecord-web-test.exe' }
$oldManual = $env:LITECORD_WEB_MANUAL
$oldPort = $env:LITECORD_AUTOMATION_PORT
try {
    $env:LITECORD_WEB_MANUAL = '1'
    $env:LITECORD_AUTOMATION_PORT = "$Port"
    Push-Location $repo
    Write-Output 'Inicie a transmissão e use Compartilhar na web > Copiar link.'
    & $exe
} finally {
    Pop-Location
    $env:LITECORD_WEB_MANUAL = $oldManual
    $env:LITECORD_AUTOMATION_PORT = $oldPort
}
