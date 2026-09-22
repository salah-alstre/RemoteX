# Remote-control latency

## What made the mouse feel heavy (measured, old pipeline)
Measured in an emulated network (`crates/session/tests/latency.rs`) before the changes:
* Loopback: **16.9 ms** median input delay on a network with zero delay.
* +12 to +17 ms of application overhead on every emulated path.
* While a file was uploading: **input stopped arriving entirely** (nothing applied in 9 s). Up to 64 file
  chunks of 256 KB were queued *ahead* of the click.

Causes, all fixed:
1. **Deep queues + whole-message sends.** A 256 KB file chunk or a video keyframe was one indivisible unit and
   up to 64 units could wait in the link queue, so a click waited for all of them.
2. **No coalescing.** Every mouse position was replayed in order.
3. **Nagle** on the relay server's sockets (server-side `TCP_NODELAY` missing) and a separate 4-byte length write.
4. **Input handled in the session's main loop**, behind clipboard/file/statistics work; applied inside the async task.
5. **UI**: input batched on animation frames, sent as JSON through the main-thread command path.
6. **Cursor feedback tied to the video loop** and 8 frames of video allowed in flight to the webview.

## Current design
* Three lanes (control/input, video, bulk) on one encrypted ordered stream, **8 KB fragments**, control
  interleaved between fragments; the scheduler reserves a link slot first and chooses last (`secure.rs`).
* Link queue depth 4 (was 64); relay server queue 48 (was 512); `TCP_NODELAY` on server and clients; one write
  per frame.
* Mouse movement: only the newest position is kept (one slot on the sender, run-collapsing on host); clicks,
  keys, wheel never dropped or reordered; sequence numbers reject stale moves.
* Host input runs on its own high-priority thread fed directly by the network reader; all held keys/buttons are
  released on disconnect.
* Viewer page: pointer events sent at up to 250 Hz, no React state, compact binary IPC packet on an async
  command; optional **local cursor prediction** (Auto/On/Off) that hands back to the host position when still.
* Video: at most 3 frames in flight to the webview (was 8), decoder queue 2 (was 6), stale frames dropped and a
  fresh keyframe requested; capture runs at 60 fps while input is active; encoder has no B-frames, CBR, quarter
  second rate-control buffer; `Auto` preset now favours low latency.
* File transfers are paced by a queue-delay controller (`bulk.rs`) that backs off as soon as the priority ping
  RTT inflates.
* Telemetry (Connection info -> Advanced diagnostics): input RTT, local/remote queue, OS apply time,
  input-to-picture arrival, stale moves/frames dropped, file rate.

## Measurements (one-way input delay, host applying an event after the viewer sent it)
The measured results are tabulated in the [README performance section](../README.md#performance). Reproduce them with
`cargo test --release -p remotex-session --test latency -- --ignored --nocapture`,
`--test input_stress`, and `--test latency_prod` (production relay).
