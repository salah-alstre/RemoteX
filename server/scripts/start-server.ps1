. "$PSScriptRoot\_common.ps1"
Assert-Admin
Start-Service $script:ServiceName
(Get-Service $script:ServiceName).WaitForStatus('Running', [TimeSpan]::FromSeconds(20))
Write-Host 'RemoteX server started.' -ForegroundColor Green
