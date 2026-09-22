<#
.SYNOPSIS
  Replaces the server binary with the one from this package, keeping data and configuration.
  Rolls back automatically if the new version does not become healthy.
#>
param([string]$InstallDir)
. "$PSScriptRoot\_common.ps1"
Assert-Admin
$dir = Get-InstallDir $InstallDir
$package = Split-Path $PSScriptRoot -Parent
$new = Join-Path $package 'remotex-server.exe'
if (-not (Test-Path $new)) { throw "remotex-server.exe not found in $package" }
$exe = Join-Path $dir 'remotex-server.exe'
$backup = Join-Path $dir 'remotex-server.previous.exe'
$cfg = Read-EnvFile (Join-Path $dir '.env')
$port = [int](Get-EnvValue $cfg 'SERVER_PORT' 8443)
$tls = [bool](Get-EnvValue $cfg 'TLS_CERT_PATH' '')

Stop-Service $script:ServiceName -Force
(Get-Service $script:ServiceName).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
Copy-Item $exe $backup -Force
Copy-Item (Join-Path $dir 'data\remotex.db') (Join-Path $dir 'data\remotex.db.bak') -Force -ErrorAction SilentlyContinue
Copy-Item $new $exe -Force
Copy-Item (Join-Path $PSScriptRoot '*.ps1') (Join-Path $dir 'scripts') -Force
Start-Service $script:ServiceName
if (Test-Health -Port $port -Tls $tls) {
    Write-Host 'Update complete.' -ForegroundColor Green
} else {
    Write-Host 'New version failed its health check: rolling back.' -ForegroundColor Red
    Stop-Service $script:ServiceName -Force
    Copy-Item $backup $exe -Force
    Start-Service $script:ServiceName
    exit 1
}
