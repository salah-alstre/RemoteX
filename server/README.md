# RemoteX server — deployment guide (Windows Server, `45.88.9.191`)

One Rust binary provides the device registry, signaling, encrypted-traffic relay, health endpoint and a small
admin dashboard. It runs as the Windows Service **RemoteXServer**, starts at boot and restarts itself on failure.

## What you need

| Requirement | Notes |
| --- | --- |
| Windows Server 2016+ (x64) | Administrator access (RDP) |
| The package `remotex-server-<version>.zip` | Built on your PC with `npm run build:server` |
| One inbound **TCP** port | Default `8443`. No UDP. No database port is ever exposed. |
| Optional: a domain + certificate | Not required for the first test (see "TLS") |

No Rust, Node, Visual Studio, or database needs to be installed on the server: the zip is self-contained
(SQLite is compiled into the binary).

## Ports

| Port | Protocol | Purpose | Public? |
| --- | --- | --- | --- |
| `8443` | **TCP** | HTTPS + WebSocket: signaling, relay, `/health`, `/admin`, `/updates` | Yes |
| — | — | Database (SQLite file, no network port) | Never |

If your hosting provider has a separate network firewall / security group, open `8443/tcp` there too.

## 1. Build the package (on your PC)

```powershell
npm install
npm run build:server
# -> dist-server\remotex-server-0.1.0.zip  (+ .sha256)
```

## 2. Copy it to the server
Copy the zip to the server (RDP drag-and-drop, `scp`, or `Copy-Item -ToSession`). Verify the hash if you like:
```powershell
Get-FileHash .\remotex-server-0.1.0.zip -Algorithm SHA256   # compare with the .sha256 file
```

## 3. Install (elevated PowerShell on the server)

```powershell
Expand-Archive .\remotex-server-0.1.0.zip -DestinationPath C:\Temp\remotex-server -Force
cd C:\Temp\remotex-server
Set-ExecutionPolicy -Scope Process Bypass
.\scripts\install-server.ps1 -PublicHost 45.88.9.191
```

The installer:

1. copies the binary and scripts to `C:\RemoteX\server` (`data`, `logs`, `certs`, `updates`);
2. generates a **self-signed certificate** for `45.88.9.191` (unless you pass `-CertPath/-KeyPath`) and prints its
   **SHA-256 fingerprint**;
3. writes `C:\RemoteX\server\.env` with random `SERVER_SECRET` and `ADMIN_TOKEN` (kept on re-install);
4. locks the folder so only SYSTEM/Administrators (full) and the service account (read/modify where needed) can access it;
5. creates the Windows Firewall rule `RemoteX Server (TCP)` for the port;
6. installs the service (`LocalService`, automatic start, restart after 5 s / 5 s / 30 s on failure) and starts it;
7. checks `https://127.0.0.1:8443/health` and prints the values you must keep.

Equivalent firewall commands, if you prefer to run them yourself:

```powershell
New-NetFirewallRule -DisplayName "RemoteX Server (TCP)" -Direction Inbound -Action Allow -Protocol TCP -LocalPort 8443 -Profile Any
Get-NetFirewallRule -DisplayName "RemoteX Server (TCP)" | Get-NetFirewallPortFilter
```

## 4. Clients need nothing

Installed RemoteX clients connect to the official service automatically and trust only its certificate; users are
never asked for an address, port or fingerprint. The address and the trusted certificate fingerprint(s) are compiled
into the client from `apps/desktop/src-tauri/config/production.json`. You only touch that file when the service
address or certificate changes; see [../docs/INFRASTRUCTURE.md](../docs/INFRASTRUCTURE.md) for certificate
rotation (current + backup pin) and the later move to a domain.

## Environment variables (`C:\RemoteX\server\.env`)

| Variable | Default | Meaning |
| --- | --- | --- |
| `SERVER_HOST` | `0.0.0.0` | Listen interface |
| `SERVER_PORT` | `8443` | The single TCP port |
| `PUBLIC_SERVER_URL` | derived | Informational; what clients use |
| `DATABASE_PATH` | `remotex.db` | SQLite file |
| `SERVER_SECRET` | **required**, ≥ 32 chars | Pepper for device-secret hashes. Changing it invalidates all devices |
| `ADMIN_TOKEN` | **required**, ≥ 20 chars | Bearer token for `/admin/api/stats` |
| `TLS_CERT_PATH`, `TLS_KEY_PATH` | unset | PEM certificate chain and private key. Both or neither. Without them the server refuses to start on any non-loopback address unless `ALLOW_PLAIN_HTTP=1` (only for a TLS-terminating proxy) |
| `LOG_DIR` | `logs` | Log folder (daily rotation; `server`, `connection`, `security`) |
| `MAX_SESSIONS` | `500` | Concurrent session cap |
| `MAX_RELAY_MBPS_PER_SESSION` | `80` | Relay throttle per session |
| `UPDATES_DIR` | `updates` | Served at `/updates/` |

## Operating it

```powershell
cd C:\RemoteX\server\scripts
.\status-server.ps1     # service state, listening, health, firewall, last log lines
.\stop-server.ps1
.\start-server.ps1
.\restart-server.ps1    # restarts and verifies /health
```

**Logs:** `C:\RemoteX\server\logs\` — `server.log.*` (application), `connection.log.*` (sessions/devices),
`security.log.*` (auth failures, bans, admin access). Live view:
`Get-Content C:\RemoteX\server\logs\security.log.* -Tail 20 -Wait`.

**Admin dashboard:** `https://45.88.9.191:8443/admin` — enter `ADMIN_TOKEN`. Shows online devices, active
sessions (direct / relay), relay bandwidth, server CPU/memory, client versions, error and auth-failure counts.
It never shows anyone's screen or data. Because the test certificate is self-signed your browser will warn; that
is expected for IP-only testing.

**Health:** `GET /health` → `{"ok":true,"version":"…","uptime_secs":…}` (no authentication, no sensitive data).

## Updating the server
Build a new package, copy it over, then:
```powershell
Expand-Archive .\remotex-server-<new>.zip -DestinationPath C:\Temp\remotex-server-new -Force
C:\Temp\remotex-server-new\scripts\update-server.ps1
```
It stops the service, backs up the binary and database, swaps the binary, starts it, and **rolls back
automatically** if `/health` does not come up.

## Uninstalling
```powershell
.\scripts\uninstall-server.ps1               # removes service + firewall rule, keeps data/logs/certs/.env
.\scripts\uninstall-server.ps1 -PurgeData    # removes everything
```

## TLS

* **IP-only operation (current).** The installer generates a self-signed certificate for the IP. Traffic is
  TLS-encrypted and clients verify the server against the pinned SHA-256 fingerprint(s) built into the app, so a
  man-in-the-middle is rejected. Certificate verification is never disabled anywhere. The fingerprint printed by the
  installer is what goes into `production.json` (`pins.current`) when you build a client.
* **Rotation.** Generate the next certificate with `remotex-server.exe gen-cert`, ship its pin as `pins.backup` in
  a client release, and only then swap the certificate on the server. Full procedure:
  [../docs/INFRASTRUCTURE.md](../docs/INFRASTRUCTURE.md).
* **Domain.** Point DNS at the server, obtain a CA-issued certificate (for example with
  [win-acme](https://www.win-acme.com/)), install it with
  `.\scripts\install-server.ps1 -PublicHost api.example.com -CertPath ... -KeyPath ...`, then change `host` (and
  optionally clear the pins) in `production.json` and release a new client.

## Capacity notes
The relay forwards ciphertext with bounded per-session queues, a per-session bandwidth cap, idle/abandoned
session cleanup (60 s pending, 120 s idle relay) and a global session limit. Direct (LAN) sessions do not use
relay bandwidth at all. Watch *Relay bandwidth* in the dashboard; one 1080p session is typically 3–10 Mbps.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| `/health` fails locally | `status-server.ps1`; read the last lines of `server.log.*` (missing `SERVER_SECRET`, unreadable certificate) |
| Clients show "Service unavailable" | Windows Firewall rule and any provider firewall for `8443/tcp`; `Test-NetConnection 45.88.9.191 -Port 8443` from your PC |
| Clients show "Service unavailable" but the service is healthy | The certificate on the server no longer matches a pin built into the client (`production.json`). Restore the previous certificate or publish a client with the new pin |
| Service will not start | Event Viewer → Windows Logs → Application; run `.\remotex-server.exe run` in a console from `C:\RemoteX\server` to see errors |
