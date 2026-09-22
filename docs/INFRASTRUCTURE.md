# Infrastructure configuration (maintainers)

End users never see or configure any of this. A normal RemoteX build connects to the official service
automatically and trusts only the official server certificate. This page is for the people who operate the
service and publish client builds.

## Where the official endpoint lives

**One file:** [`apps/desktop/src-tauri/config/production.json`](../apps/desktop/src-tauri/config/production.json)

```json
{
  "host": "45.88.9.191",
  "port": 8443,
  "path": "/ws",
  "pins": { "current": "<sha256 of the leaf certificate>", "backup": "<sha256 of the NEXT certificate or empty>" },
  "updateManifestUrl": "https://45.88.9.191:8443/updates/latest.json"
}
```

* It is compiled into the native Rust client (`apps/desktop/src-tauri/src/infra.rs`, `include_str!`). It is never
  read from disk at runtime and never sent to the web UI, so neither users nor the frontend can change it.
* `infra.rs` is the only code that knows the host or the pins. Everything else asks `infra::resolve()`.
* `scripts/apply-branding.mjs` copies only `updateManifestUrl` into the Tauri updater configuration.
* `branding.json` holds non-sensitive branding only (name, colours, website, support email).
* Nothing on the client side contains `ADMIN_TOKEN`, `SERVER_SECRET`, private keys or database credentials.

Development builds (`--features dev-tools`, never used for installers) can be pointed elsewhere:

```powershell
$env:REMOTEX_DEV_SERVER_OVERRIDE='wss://127.0.0.1:18443/ws'   # or ws://… for a plain local server
$env:REMOTEX_DEV_CERT_PIN='<sha256 of the local server certificate>'
```

## How certificate pinning works

`crates/session/src/tls.rs` installs a custom rustls verifier. During the TLS handshake it computes the SHA-256
of the server's leaf certificate and accepts the connection **only if it equals one of the configured pins**
(current or backup). The handshake signature is still verified with the normal rustls/ring algorithms, so a
certificate copied without its private key does not work. If a pin does not match, the connection is dropped
before any application data is sent; the user sees only "Service unavailable". Certificate verification is never
disabled anywhere. With **no pins configured** the client falls back to normal CA validation (used after a move to a
domain with a public certificate).

## Rotating the server certificate without an emergency

Because two pins are supported, rotation is a planned, zero-downtime operation:

1. **Create the next certificate on the server** (does not touch the running one):
   ```powershell
   cd C:\RemoteX\server
   .\remotex-server.exe gen-cert --ip 45.88.9.191 --out certs-next
   ```
   It prints the new SHA-256 fingerprint.
2. **Ship the new pin in a client release first.** Put it in `production.json` as `"backup"`, build
   (`npm run build:windows`) and publish the installer. Clients now trust *current and next*.
3. **Wait until enough clients have updated** (the dashboard at `/admin` shows client versions).
4. **Swap the server certificate:** copy `certs-next\server.crt` / `server.key` over `certs\`, then
   `.\scripts\restart-server.ps1`. Old clients trusting `current`, and updated clients trusting `backup`, both connect
   until you retire the old pin.
5. **Promote in the next client release:** move the new value to `"current"` and put the following certificate (or an
   empty string) in `"backup"`.

Clients older than step 2 will not trust the new certificate, which is why step 3 matters. Never swap the
server certificate before a release containing its pin is widely installed.

## Moving from the IP address to a domain later (e.g. `api.remotex.com`)

1. Point DNS at the server and obtain a CA-issued certificate (win-acme / Let's Encrypt), then install it:
   ```powershell
   .\scripts\install-server.ps1 -PublicHost api.remotex.com -CertPath C:\certs\fullchain.pem -KeyPath C:\certs\privkey.pem
   ```
2. Edit **only** `production.json`: set `"host": "api.remotex.com"` (and `"port"`, e.g. `443` if you moved the port).
   - Keep `pins` (current + backup) while old certificates are still in use, **or** set both to `""` to rely on normal
     CA validation. Pinning the CA-issued leaf as well is optional.
   - Update `updateManifestUrl` (signed updates work with a real certificate).
3. Build and publish a new client. No networking code changes are required.
4. Keep the old IP endpoint alive until most clients have updated; older clients only know the old address.

## Server version compatibility

Client and server share a wire protocol (`crates/common`). The current client works with the deployed server
without changes. The included server package adds one optional improvement: when the service stops it now closes
established connections, so clients notice a restart immediately instead of after a network timeout. Upgrade with
`.\scripts\update-server.ps1`, which keeps `.env`, the database, certificates, and logs.

## Diagnosing "stuck without an ID" reports

Client logs (`%LOCALAPPDATA%\app.remotex.desktop\logs\network.log*`, `security.log*`) record each step of the
connection with an `event` field and never contain secrets, passwords or the service address:

| Event | Meaning |
| --- | --- |
| `tcp.connected` / `tcp.failed` | TCP to the service opened / failed (with reason) |
| `tls.handshake.start` → `tls.pin.valid` | Certificate matched a pinned fingerprint |
| `tls.pin.mismatch` | Something presented a different certificate (TLS-inspecting antivirus or proxy); only 4 bytes of the public fingerprint are logged |
| `tls.handshake.failed` | Handshake failed or timed out (with reason) |
| `websocket.connected` | Upgrade to the service protocol succeeded |
| `identity.loaded` / `identity.created` / `identity.unreadable` | Stored device identity found / new one needed / could not be decrypted (DPAPI) |
| `registration.request.sent` → `registration.response.received` | Registration round trip (outcome logged; `rate limited` means this address registered too many devices recently) |
| `auth.request.sent` → `registration.success` / `registration.failed` | Sign-in with the stored identity |
| `device_id.persisted` / `device_id.persist_failed` | Identity saved to disk |

Waits after a failed attempt stretch automatically (up to 6× the base 8/10/10 s) so slow or lossy links converge
instead of timing out identically forever. The UI only shows Ready once the service accepted this device *and* the
ID reached the window; a bare TCP connection is never shown as ready.
