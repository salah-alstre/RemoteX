# Security model

## Goals and non-goals

* **Goal:** an attacker who controls the network or the relay server cannot read or alter a session, cannot
  impersonate a host or viewer, and cannot cheaply guess passwords.
* **Goal:** the person at the host machine always knows when a session is active and decides what it may do.
* **Non-goal:** protecting a machine that is already compromised, or defending against the user of the host
  itself. A viewer granted keyboard control can do anything the logged-in user can.

## Cryptography (no custom primitives)

| Purpose | Construction |
| --- | --- |
| Password-authenticated key exchange | SPAKE2 (Ed25519 group), identities = `session ‖ role ‖ device id` |
| Key schedule | HKDF-SHA256 salted with the handshake transcript hash; four independent 32-byte keys |
| Key confirmation | HMAC-SHA256 over the transcript, one label per direction (not reflectable) |
| Session encryption | ChaCha20-Poly1305, one key per direction, implicit 64-bit counter nonce |
| Server transport | TLS 1.2/1.3 (rustls + ring); production certificate pinned in the native client (current + backup pin for rotation) |
| At-rest secrets | Windows DPAPI (per-user) with application entropy |
| Randomness | operating-system RNG only (`OsRng`), rejection sampling for uniform ids/passwords |

Because the nonce is an implicit ordered counter, a **replayed, dropped, reordered or duplicated frame fails
authentication and ends the session**. There are no negotiable parameters (no downgrade surface).

## Threats and mitigations

| Threat | Mitigation |
| --- | --- |
| Relay/server eavesdropping | All session content is end-to-end encrypted; the relay forwards opaque bytes |
| Offline password guessing from captured traffic | SPAKE2: a transcript reveals nothing to test guesses against |
| Online password guessing | Host locks a viewer out after 5 failures/10 min (30 min lock); global 10-failure trip rotates the password; server bans the viewer id and source IP after reported failures |
| Device id enumeration | Per-device connect budget, per-IP "offline target" budget → temporary ban, presence queries throttled |
| Device secret brute force | 256-bit secrets, constant-time compare, per-IP auth-failure ban (10/5 min → 15 min), uniform work for unknown ids |
| Registration flood | 30 registrations/hour/IP (then a 15-minute pause; homes and offices share addresses, so this is deliberately not tighter) |
| Session hijack | Session id is a 128-bit random capability *and* only members' authenticated connections may relay; frames are AEAD-authenticated end to end |
| Viewer-id spoofing | Host requires `Hello.viewer_id` to equal the id attested by the server |
| Unauthorised control messages | Every input/clipboard/file message is checked against the *current* permission set on the host |
| Malformed packets | Bounded postcard decoding (8 MiB frames), trailing-byte rejection, per-message validation, fuzz-style tests |
| Path traversal / hostile file names | `sanitize_relative`: no `..`, absolute paths, drive letters, ADS `:`, reserved device names, control characters, trailing dots/spaces, length caps; never overwrites existing files |
| Oversized/zip-bomb style transfers | Per-file size cap, in-order chunk enforcement, SHA-256 verification, `.part` files |
| Stuck keys | Host tracks held keys/buttons, releases on viewer blur, permission loss, disconnect and drop |
| Clipboard loops / exfiltration | Hash-based echo suppression, size caps, per-session and per-permission switches |
| Silent sessions | Session banner, tray status and window title change are always shown; approval is required for attended access |
| Downgrade to plaintext | `ws://` is refused unless a developer explicitly opts in; certificate verification is never globally disabled |

## What the server can and cannot see

See [PRIVACY.md](PRIVACY.md). In short: device ids, connection times, source IPs, client versions and byte
counts — never screen content, keystrokes, clipboard or files.

## Unattended access

Off by default and gated in four steps: the owner enables it, sets a ≥ 8-character permanent password (DPAPI
encrypted, never plaintext), the connecting device must be on the *trusted* list (or the owner allows any
device), and the session is still visible on the host (banner, tray, title). Revoking a trusted device removes
its unattended right immediately; "revoke all" is one click.

## Logging

Structured JSON logs are split by category on the client (`security`, `session`, `network` incl. server connection, `media` for capture/codec, and `app`) and into application, connection and security streams on the server. Passwords, keys, tokens,
clipboard content and screen data are never passed to the logger.

## Reporting vulnerabilities

Email the address in `branding.json` (`supportEmail`). Please do not open public issues for security bugs.

## Known gaps (be aware before public release)

* The client has not had an independent security audit.
* Windows code signing and update signing keys must be provisioned by you (see [RELEASING.md](RELEASING.md)).
* The relay sees timing and volume metadata (unavoidable for any relay).
* A viewer that is granted file access can browse the host's file system, subject to Windows permissions of
  the account running the app.
