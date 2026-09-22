. "$PSScriptRoot\_common.ps1"
Assert-Admin
Stop-Service $script:ServiceName -Force
(Get-Service $script:ServiceName).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
Write-Host 'RemoteX server stopped.' -ForegroundColor Yellow
