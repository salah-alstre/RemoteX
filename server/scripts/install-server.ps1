<#
.SYNOPSIS
  Installs (or repairs) the RemoteX server as a Windows Service.
.DESCRIPTION
  Run from the extracted server package in an elevated PowerShell:
    .\scripts\install-server.ps1 -PublicHost 45.88.9.191
  With a domain and a real certificate:
    .\scripts\install-server.ps1 -PublicHost api.example.com -CertPath C:\certs\fullchain.pem -KeyPath C:\certs\privkey.pem
#>
param(
    [Parameter(Mandatory)][string]$PublicHost,
    [string]$InstallDir = 'C:\RemoteX\server',
    [int]$Port = 8443,
    [string]$CertPath,
    [string]$KeyPath
)
. "$PSScriptRoot\_common.ps1"
Assert-Admin

$package = Split-Path $PSScriptRoot -Parent
$exeSource = Join-Path $package 'remotex-server.exe'
if (-not (Test-Path $exeSource)) { throw "remotex-server.exe not found in $package" }

Write-Host "Installing to $InstallDir" -ForegroundColor Cyan
foreach ($d in 'data', 'logs', 'certs', 'updates', 'scripts') { New-Item -ItemType Directory -Force -Path (Join-Path $InstallDir $d) | Out-Null }

# Stop a running instance before replacing the binary.
if (Get-Service $script:ServiceName -ErrorAction SilentlyContinue) {
    Stop-Service $script:ServiceName -Force -ErrorAction SilentlyContinue
    (Get-Service $script:ServiceName).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(20))
}
Copy-Item $exeSource (Join-Path $InstallDir 'remotex-server.exe') -Force
Copy-Item (Join-Path $PSScriptRoot '*.ps1') (Join-Path $InstallDir 'scripts') -Force
$exe = Join-Path $InstallDir 'remotex-server.exe'

# TLS material.
$certDir = Join-Path $InstallDir 'certs'
$fingerprint = $null
if ($CertPath -and $KeyPath) {
    Copy-Item $CertPath (Join-Path $certDir 'server.crt') -Force
    Copy-Item $KeyPath (Join-Path $certDir 'server.key') -Force
} elseif (-not (Test-Path (Join-Path $certDir 'server.crt'))) {
    Write-Host 'No certificate supplied: generating a self-signed certificate for pinning.' -ForegroundColor Yellow
    $parsed = $null
    $kind = if ([Net.IPAddress]::TryParse($PublicHost, [ref]$parsed)) { '--ip' } else { '--dns' }
    $out = & $exe gen-cert $kind $PublicHost --out $certDir
    $fingerprint = ($out | Select-Object -Last 1).Trim()
}

# Configuration. Existing secrets are preserved on re-install.
$envPath = Join-Path $InstallDir '.env'
$existing = Read-EnvFile $envPath
$secret = Get-EnvValue $existing 'SERVER_SECRET' ''
if ($secret.Length -lt 32) { $secret = New-RandomSecret 48 }
$admin = Get-EnvValue $existing 'ADMIN_TOKEN' ''
if ($admin.Length -lt 20) { $admin = New-RandomSecret 32 }
$lines = @(
    'SERVER_HOST=0.0.0.0',
    "SERVER_PORT=$Port",
    "PUBLIC_SERVER_URL=wss://${PublicHost}:$Port/ws",
    "DATABASE_PATH=$InstallDir\data\remotex.db",
    "SERVER_SECRET=$secret",
    "ADMIN_TOKEN=$admin",
    "TLS_CERT_PATH=$certDir\server.crt",
    "TLS_KEY_PATH=$certDir\server.key",
    "LOG_DIR=$InstallDir\logs",
    'MAX_SESSIONS=500',
    'MAX_RELAY_MBPS_PER_SESSION=80',
    "UPDATES_DIR=$InstallDir\updates"
)
Set-Content -Path $envPath -Value $lines -Encoding ascii

# Least privilege: the service runs as LocalService; secrets are readable only by it and administrators.
icacls $InstallDir /inheritance:r /grant:r 'SYSTEM:(OI)(CI)F' 'Administrators:(OI)(CI)F' 'NT AUTHORITY\LocalService:(OI)(CI)RX' | Out-Null
foreach ($d in 'data', 'logs') { icacls (Join-Path $InstallDir $d) /grant 'NT AUTHORITY\LocalService:(OI)(CI)M' | Out-Null }

# Firewall: one TCP port, nothing else.
Get-NetFirewallRule -DisplayName $script:FirewallRule -ErrorAction SilentlyContinue | Remove-NetFirewallRule
New-NetFirewallRule -DisplayName $script:FirewallRule -Direction Inbound -Action Allow -Protocol TCP -LocalPort $Port -Profile Any | Out-Null

# Service with automatic start and automatic restart on failure.
if (Get-Service $script:ServiceName -ErrorAction SilentlyContinue) { sc.exe delete $script:ServiceName | Out-Null; Start-Sleep -Seconds 2 }
sc.exe create $script:ServiceName binPath= "`"$exe`" service" start= auto DisplayName= 'RemoteX Server' obj= 'NT AUTHORITY\LocalService' | Out-Null
sc.exe description $script:ServiceName 'RemoteX signaling, relay and device registry' | Out-Null
sc.exe failure $script:ServiceName reset= 86400 actions= restart/5000/restart/5000/restart/30000 | Out-Null
sc.exe failureflag $script:ServiceName 1 | Out-Null

Start-Service $script:ServiceName
if (Test-Health -Port $Port -Tls $true) {
    Write-Host "`nRemoteX server is running." -ForegroundColor Green
} else {
    Write-Host "`nThe service started but /health did not answer. Check $InstallDir\logs\server.log*" -ForegroundColor Red
}

Write-Host "`n=== Save these values ===" -ForegroundColor Cyan
Write-Host "Install dir     : $InstallDir"
Write-Host "Client URL      : wss://${PublicHost}:$Port/ws"
Write-Host "Admin dashboard : https://${PublicHost}:$Port/admin   (token is ADMIN_TOKEN in $envPath)"
if ($fingerprint) {
    Write-Host 'Certificate SHA-256 (maintainers: goes into apps/desktop/src-tauri/config/production.json as pins.current; end users configure nothing):'
    Write-Host "  $fingerprint" -ForegroundColor Yellow
}
