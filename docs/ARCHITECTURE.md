# Architecture

This document records what was built, why, and where the real limits are.

## Overview

```
 Viewer (Tauri app)                                    Host (Tauri app)
 ┌────────────────────────┐                            ┌────────────────────────────┐
 │ React UI  ── WebCodecs │◀── E2E-encrypted stream ──▶│ DXGI capture → GPU NV12    │
 │ input capture          │    (ChaCha20-Poly1305)     │ Media Foundation encoder   │
 │ Rust engine (session)  │                            │ SendInput · clipboard · FS │
 └───────────┬────────────┘                            └───────────────┬────────────┘
             │   1. direct TCP on the same LAN  ───────────────────────┘
             │   2. otherwise: WSS to the server, which relays ciphertext only
             ▼
      ┌─────────────────────────────────────────────┐
      │ remotex-server (Rust, one TCP port, TLS)    │
      │ registry · signaling · relay · rate limits  │
      │ SQLite metadata · /health · /admin (token)  │
      └─────────────────────────────────────────────┘
```

## Repository layout

| Path | Role |
| --- | --- |
| `crates/common` | Wire protocol (postcard), ids, branding loader. Every frame passes through bounded `wire::decode`. |
| `crates/security` | SPAKE2 PAKE, HKDF, ChaCha20-Poly1305 channel, DPAPI wrapper, ids/passwords, attempt limiter. |
| `crates/capture` | DXGI Desktop Duplication, D3D11 video-processor scaling + NV12 conversion, cursor shape capture. |
| `crates/codec` | Media Foundation H.264/HEVC encoder (NVENC / Quick Sync / AMF via their MFTs, software fallback). |
| `crates/input` | `SendInput` injection with held-key tracking, clipboard sync with loop prevention. |
| `crates/files` | Path sanitising, resumable hash-verified transfer endpoints. |
| `crates/elevate`, `crates/service` | Elevated-control protocol and the Windows service / helper (see [ELEVATED_CONTROL.md](ELEVATED_CONTROL.md)). |
| `crates/session` | The engine: signaling client, handshake, host/viewer loops, adaptive streaming, transfers, persistence. |
| `server` | The backend service (library + binary + Windows Service host). |
| `apps/desktop` | Tauri 2 shell (`src-tauri`) and React/TypeScript UI. |
| `branding.json` | The single place for name, colours, URLs, company, support email. |

## Decisions

### Transport: an authenticated encrypted stream, not WebRTC
The brief prefers WebRTC "unless you identify a better architecture". WebRTC in Rust (`webrtc-rs`) has no
hardware-encoder path, no congestion controller tuned for desktop content, and would still need a TURN
server that runs well on Windows. The chosen design keeps the parts that matter:

* **End-to-end encryption** with a password-authenticated key exchange (see [SECURITY.md](SECURITY.md)).
* **Direct connection** when both peers share a network; **relay** through the server otherwise.
* **Prioritised send lanes** (control/input > video > bulk) with fragmentation, so input never waits behind a file or a
  video frame ([LATENCY.md](LATENCY.md)).
* **Adaptive video** driven by real send-queue delay, drops and RTT (`crates/session/src/adapt.rs`).

The cost is real: the stream is TCP (direct) or a TLS WebSocket (relay), so a lossy WAN link shows up as
head-of-line delay rather than loss concealment, and there is no UDP hole punching. Section
"Limitations" lists what that means in practice.

### Capture and encoding stay on the GPU as long as possible
DXGI Desktop Duplication delivers the desktop as a GPU texture. The D3D11 video processor scales it and
converts BGRA→NV12 on the GPU; only the small NV12 image is read back for the encoder. Unchanged desktops
produce no frames, so an idle screen costs almost nothing. Measured on the development machine (RTX 4070,
2560×1440): ~3.6 ms capture+convert and ~4.6 ms encode per frame with NVENC.

The encoder is fed system-memory NV12 (one readback, no per-pixel CPU work). A fully zero-copy path (passing the
D3D11 texture straight to the MFT) is possible future work.

### Decoding uses WebCodecs
The webview (WebView2/Chromium) decodes H.264 and, where the OS has the extension, HEVC with GPU
acceleration through `VideoDecoder`, then draws to a canvas. The Rust side hands over raw frames through a
Tauri binary `Channel` and sheds load if the webview falls behind (unacknowledged-frame limit → drop until
the next keyframe). The decoder's codec string is derived from the SPS in the stream itself.

### One server port
API, signaling, relay, health and the admin page share one TLS port (default `8443/tcp`). Fewer exposed ports, one
certificate, one firewall rule. Devices register over the same WebSocket (no separate REST API).

### Server: Rust + SQLite
`axum` + `rusqlite` (bundled SQLite, WAL). SQLite is appropriate for one server node holding small metadata;
the code isolates persistence in `server/src/db.rs` should a network database ever be needed. It runs as a
Windows Service (`windows-service` crate) with automatic restart configured by the installer.

### Temporary passwords and unattended access
The temporary password is 6 characters from a 31-symbol unambiguous alphabet (~29.7 bits). That is safe only
because SPAKE2 turns every guess into an online interaction with the host, which locks out a viewer after 5
failures and burns the password after 10 failures across viewers. The unattended password must be ≥ 8
characters, is stored DPAPI-encrypted, and only *authorised* (trusted) devices may use it unless the owner
explicitly allows any device.

## Protocol summary

1. `Register`/`Auth` over `wss://…/ws`. The server issues a random 9-digit id and a 256-bit secret; it stores
   only `SHA-256(pepper ‖ secret)`.
2. Viewer sends `Connect{target}`. The server checks rate limits and bans, creates a random 128-bit session id,
   sends `Incoming` to the host and `SessionReady` to the viewer (LAN candidates only when both share a public IP).
3. Peers connect (direct TCP with a 16-byte session preamble, else relayed frames) and run SPAKE2:
   `Hello → HelloReply → Confirm ↔ Confirm`. The host verifies that the viewer id in `Hello` equals the id the
   server attested.
4. The host shows the consent dialog; the viewer receives `Accepted{permissions,…}` or `Rejected`.
5. Encrypted `Msg` frames flow: video, cursor, input, clipboard, files, chat, stats, pings.

## Adaptive streaming

Every second the host feeds `(rtt, send-queue delay, dropped frames)` to the controller:

* congestion → cut bitrate 25%, then frame rate (60→30→15), then resolution (100→75→50→33%);
* recovery is slow (six clean seconds, 5 s hold-off after every change) and reverses the order;
* manual FPS/bitrate/resolution pin that dimension; "adaptive quality: off" pins all three.

Video frames are never queued when the network is behind: the pipeline skips encoding, requests a refresh of the
final desktop state, and counts a dropped frame.

## Telemetry

Everything in the connection-info panel is measured: RTT/jitter from ping-pong, receive rate from socket bytes,
capture/encode time from the host pipeline, decode/render time from `VideoDecoder`, FPS from rendered frames.
The transport is a reliable stream, so "packet loss" is reported as **dropped-frame share** instead.

## Limitations (platform and scope)

* **No UDP / NAT hole punching.** Direct connections work on a shared LAN (or when the host is reachable);
  other traffic uses the relay. Latency over the internet therefore includes the relay hop.
* **Secure desktop.** UAC prompts, the lock screen and Ctrl+Alt+Del cannot be seen or driven by a normal user-mode
  process. Elevated Control uses an installed Windows service for approved sessions ([ELEVATED_CONTROL.md](ELEVATED_CONTROL.md));
  it has automated tests but has not yet been validated end to end on an installed system with a live UAC prompt.
* **AV1** is not implemented (Media Foundation exposes AV1 encoders only on newer hardware/OS builds).
  H.265 works where the GPU and the WebView decoder support it, with automatic fallback to H.264.
* **Audio streaming** and **session recording** are not implemented in this version; they are absent from the UI
  rather than faked.
* **Privacy (screen-blanking) mode** is not implemented.
* **Windows only** (capture, injection and installer).
