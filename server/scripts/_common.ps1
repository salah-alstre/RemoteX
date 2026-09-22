# Shared helpers for the RemoteX server scripts.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Windows PowerShell 5.1 does not load System.Net.Http by default.
Add-Type -AssemblyName System.Net.Http

# A compiled callback is required: PowerShell script blocks cannot run on the TLS worker thread.
if (-not ('RemoteXLoopbackProbe' -as [type])) {
    Add-Type -ReferencedAssemblies System.Net.Http -TypeDefinition @'
using System.Net.Http;
public static class RemoteXLoopbackProbe {
    public static HttpClientHandler Handler() {
        var h = new HttpClientHandler();
        h.ServerCertificateCustomValidationCallback = (message, cert, chain, errors) => true;
        return h;
    }
}
'@
}

$script:ServiceName = 'RemoteXServer'
$script:FirewallRule = 'RemoteX Server (TCP)'

function Assert-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Run this script from an elevated PowerShell (Run as Administrator).'
    }
}

function Get-InstallDir {
    param([string]$Override)
    if ($Override) { return $Override }
    $svc = Get-CimInstance Win32_Service -Filter "Name='$script:ServiceName'" -ErrorAction SilentlyContinue
    if ($svc) { return Split-Path (($svc.PathName -replace '^"([^"]+)".*$', '$1')) -Parent }
    return 'C:\RemoteX\server'
}

function New-RandomSecret {
    param([int]$Length = 48)
    $chars = [char[]]'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789'
    $bytes = New-Object byte[] $Length
    [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    return -join ($bytes | ForEach-Object { $chars[$_ % $chars.Length] })
}

function Read-EnvFile {
    param([string]$Path)
    $map = @{}
    if (Test-Path $Path) {
        Get-Content $Path | ForEach-Object {
            if ($_ -match '^\s*([A-Z_]+)\s*=\s*(.*?)\s*$') { $map[$Matches[1]] = $Matches[2] }
        }
    }
    return $map
}

function Get-EnvValue {
    param($Map, [string]$Key, $Default)
    if ($Map.ContainsKey($Key) -and $Map[$Key]) { return $Map[$Key] }
    return $Default
}

# Loopback-only probe. The server's certificate may be self-signed, and this request never leaves
# the machine, so validation is skipped for this single call. Clients always verify.
function Test-Health {
    param([int]$Port, [bool]$Tls, [int]$Retries = 15)
    $scheme = if ($Tls) { 'https' } else { 'http' }
    for ($i = 0; $i -lt $Retries; $i++) {
        try {
            $client = New-Object Net.Http.HttpClient([RemoteXLoopbackProbe]::Handler())
            $client.Timeout = [TimeSpan]::FromSeconds(3)
            $body = $client.GetStringAsync("${scheme}://127.0.0.1:$Port/health").GetAwaiter().GetResult()
            if ($body -match '"ok":true') { return $true }
        } catch { Start-Sleep -Seconds 1 }
    }
    return $false
}
