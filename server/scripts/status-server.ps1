. "$PSScriptRoot\_common.ps1"
$dir = Get-InstallDir
$cfg = Read-EnvFile (Join-Path $dir '.env')
$port = [int](Get-EnvValue $cfg 'SERVER_PORT' 8443)
$tls = [bool](Get-EnvValue $cfg 'TLS_CERT_PATH' '')
$svc = Get-Service $script:ServiceName -ErrorAction SilentlyContinue
if (-not $svc) { Write-Host 'Service is not installed.' -ForegroundColor Red; exit 1 }
$mode = (Get-CimInstance Win32_Service -Filter "Name='$script:ServiceName'").StartMode
$listening = @(Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue).Count -gt 0
$healthy = Test-Health -Port $port -Tls $tls -Retries 2
$rule = $null -ne (Get-NetFirewallRule -DisplayName $script:FirewallRule -ErrorAction SilentlyContinue)
Write-Host "Service  : $($svc.Status) (start type: $mode)"
Write-Host "Install  : $dir"
Write-Host "Listening: $listening (TCP $port)"
Write-Host ("Health   : " + $(if ($healthy) { 'OK' } else { 'NOT RESPONDING' }))
Write-Host "Firewall : $rule"
foreach ($log in 'server', 'connection', 'security') {
    $f = Get-ChildItem (Join-Path $dir 'logs') -Filter "$log.log*" -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
    if ($f) { Write-Host "`n--- last lines of $($f.Name) ---"; Get-Content $f.FullName -Tail 5 }
}
