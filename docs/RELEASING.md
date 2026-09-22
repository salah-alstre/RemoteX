# Releasing

## 1. Branding
Edit `branding.json` (name, identifier, company, website, support email, colours). The service address and the trusted
certificate pins are *not* branding: they live in `apps/desktop/src-tauri/config/production.json`
(see [INFRASTRUCTURE.md](INFRASTRUCTURE.md)).
After changing branding run `node scripts/apply-branding.mjs` and `python scripts/make-icon.py` +
`npx tauri icon assets/logo.png` if you changed the accent colour or want a new icon (replace
`assets/logo.png` with your artwork to use your own).

## 2. Version
Bump `version` in `Cargo.toml` (`[workspace.package]`), `apps/desktop/package.json` and
`apps/desktop/src-tauri/tauri.conf.json`.

## 3. Build
```powershell
npm run build:windows   # installers in .\release (NSIS setup + MSI) with SHA256SUMS.txt
npm run build:server    # dist-server\remotex-server-<version>.zip
```

## 4. Code signing (required for public distribution)
Unsigned installers trigger Windows SmartScreen. Sign the `.exe`/`.msi` with your certificate:
```powershell
signtool sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 /a release\*.exe release\*.msi
```
To have Tauri sign during the build, set `bundle.windows.certificateThumbprint` in `tauri.conf.json`.

## 5. Signed updates
The app uses the Tauri updater. Updates are verified against an Ed25519/minisign public key embedded in the app;
unsigned or mis-signed payloads are rejected.

```powershell
cd apps\desktop
npx tauri signer generate -w $HOME\.remotex\updater.key      # once; keep the private key OFFLINE and backed up
```
1. Put the printed **public key** into `apps/desktop/src-tauri/tauri.conf.json` → `plugins.updater.pubkey`.
2. Set `bundle.createUpdaterArtifacts` to `true` and build with `TAURI_SIGNING_PRIVATE_KEY_PATH` pointing to the key.
3. Publish `latest.json` and the signed installer where `updateManifestUrl` (branding.json) points. The server
   serves `UPDATES_DIR` at `/updates/`.

The updater fetches over HTTPS using the system trust store, so **update checks need a real domain and CA-issued
certificate**. With the self-signed IP test server the check reports "could not check for updates"; this is
intentional (certificate verification is not disabled).

## 6. Publish
Upload the installers, `SHA256SUMS.txt`, and (optionally) the server package. Tag the release in git.
