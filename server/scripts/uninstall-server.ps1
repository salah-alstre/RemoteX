param([string]$InstallDir, [switch]$PurgeData)
. "$PSScriptRoot\_common.ps1"
Assert-Admin
$dir = Get-InstallDir $InstallDir
if (Get-Service $script:ServiceName -ErrorAction SilentlyContinue) {
    Stop-Service $script:ServiceName -Force -ErrorAction SilentlyContinue
    sc.exe delete $script:ServiceName | Out-Null
}
Get-NetFirewallRule -DisplayName $script:FirewallRule -ErrorAction SilentlyContinue | Remove-NetFirewallRule
if ($PurgeData) {
    Remove-Item $dir -Recurse -Force
} else {
    Remove-Item (Join-Path $dir 'remotex-server.exe'), (Join-Path $dir 'scripts') -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "Service removed. Data, logs, certificates and .env were kept in $dir (use -PurgeData to delete them)."
}
