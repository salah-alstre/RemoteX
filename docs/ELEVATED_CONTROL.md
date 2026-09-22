# Elevated (administrator) control

## Why it exists
Windows blocks an ordinary desktop program from (a) sending input to administrator windows (UIPI) and (b) seeing
or answering the **secure desktop** where UAC prompts appear. Without help, a remote session freezes at a UAC
prompt until someone at the PC clicks "Yes". RemoteX solves this the supported way: an installed Windows
service, used only for sessions the owner of the PC explicitly approved. **UAC is not bypassed, disabled or
auto-approved**: the prompt still has to be answered; an elevated session can *see and answer it*, exactly as a
person sitting at the PC could.

## Architecture
```
RemoteX app (user)  --named pipe \\.\pipe\remotex-service-->  remotex-service.exe (LocalSystem, starts at boot)
                                                                     | only while a grant is active
                                                                     v
                                                     remotex-service.exe agent  (SYSTEM, in the console session)
                                                     - SendInput on the *current input desktop*
                                                     - GDI capture of the current input desktop (secure desktop)
```
* The app (React/Tauri) never runs as administrator. Least privilege: only the tiny service and its short-lived
  agent hold rights, and the agent exists **only during an elevated session**.
* Ordinary capture (DXGI) keeps being used; the service's pictures are used only while a secure desktop is in
  front (the app asks every 250 ms). Elevated windows on the normal desktop are captured normally; only *input*
  needs the service there.
* Input in an elevated session goes to the service instead of the ordinary injector, one path at a time.

## Authorization
1. The viewer chooses **Standard Control** or **Full / Elevated Control** (`access.*` in the UI). The choice is
   only a request (`Permissions.elevated`).
2. The host owner sees the request and picks the access level. Standard is preselected; elevated is never
   pre-ticked, and is disabled if the service is not available on that PC.
3. If approved, the host asks the service for a **grant**. The service returns a random 128-bit token bound to
   the requesting process; input and picture pipes attach with it.
4. A grant belongs to exactly one session. It ends when: the session ends (any reason), the owner switches it
   off, the app's pipe closes (crash/exit), heartbeats stop for 10 s (dead-man switch), or the agent dies.
   A dropped-and-resumed session **never** regains elevation without a new prompt.
5. Unattended access is elevated only if the owner separately enabled *Allow elevated control in unattended
   sessions* (off by default).
6. If the service is missing, the session continues as standard and *both sides are told*; a permission that
   cannot work is never reported as granted.

## What the service will and will not do
Only the operations in `UiToService` exist: grant, attach, revoke, heartbeat, inject one input event, capture one
picture, report whether a secure desktop is showing. There is no "run this program", no path, no command, no
generic RPC. Every request is size/range validated before it is relayed.

## Security review (focused)
| Risk | Mitigation |
| --- | --- |
| Another local process impersonates RemoteX | The pipe ACL admits only SYSTEM, Administrators and the interactive user, remote clients are refused, and the service identifies the caller from the pipe (`GetNamedPipeClientProcessId`) and only serves the executable at `<install dir>\remotex.exe`, in the active console session. |
| Replaced executable (user-writable install) | The service refuses to grant unless its own directory is under Program Files (administrator-writable only). The installers are per-machine. |
| Pipe squatting | The service pipe uses `FILE_FLAG_FIRST_PIPE_INSTANCE`; agent pipes have random names, SYSTEM-only ACLs, and the agent's PID and role are verified against the process the service started. |
| Arbitrary command execution | None exists. The agent command line is fixed by the service (`agent --input <pipe> --video <pipe>`), arguments are generated pipe names. |
| Path traversal / DLL hijacking | No paths accepted. The service and agent load from their own protected directory; the agent is the service's own executable. |
| Session spoofing / replay | Token is 128-bit random, bound to the owner PID, invalid after revoke; nothing is replayable across grants. Pipes are local, not network. |
| Resource exhaustion | Max 8 connections, 16 MB frame cap, 7680 px dimension cap, text cap. Malformed request drops the connection and its grant. |
| Service tampering | Service object ACL: SYSTEM/Administrators manage; users may only query. Restart-on-failure configured. |
| **Trust limit (honest)** | The service trusts the verified RemoteX app to report that the owner approved. Malware that can inject code into the RemoteX process already runs as that user; the service cannot see the click itself. Mitigations are the dead-man switch, single grant, per-session tokens and the always-visible "Elevated Control" banner on both ends. |

## Windows limitations (honest)
* Secure-desktop pictures come from GDI at a modest frame rate; they are for reading and answering prompts, not
  for video. Some protected content (DRM, some lock-screen elements) may be black.
* The lock screen / sign-in screen is a secure desktop too: with an elevated session it is visible, but signing
  in still requires credentials.
* Some vendor "secure input" dialogs may still refuse synthetic input.
* Fast user switching: the service serves the active console session only.

## Testing
* `crates/service/tests/broker.rs`: real pipes and the real agent (as the current user): caller refusal, grant
  lifecycle, capture, input relay, revocation, owner-disconnect, lapse, malformed requests.
* `crates/session/tests/elevation.rs`: policy through the real engine with a mock service (standard never
  elevated, per-session, revocation, unattended, missing service, secure-desktop picture switch).
* The SYSTEM-token launch and the real UAC secure desktop can only be exercised on an installed system with
  administrator rights: install the MSI/NSIS installer, open a session with *Full / Elevated Control*, start
  `regedit` as administrator and trigger a UAC prompt.
