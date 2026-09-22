. "$PSScriptRoot\_common.ps1"
Assert-Admin
Restart-Service $script:ServiceName -Force
(Get-Service $script:ServiceName).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
$cfg = Read-EnvFile (Join-Path (Get-InstallDir) '.env')
$port = [int](Get-EnvValue $cfg 'SERVER_PORT' 8443)
if (Test-Health -Port $port -Tls ([bool](Get-EnvValue $cfg 'TLS_CERT_PATH' ''))) {
    Write-Host 'RemoteX server restarted and healthy.' -ForegroundColor Green
} else {
    Write-Host 'Restarted, but /health is not answering. Check the logs.' -ForegroundColor Red
    exit 1
}
