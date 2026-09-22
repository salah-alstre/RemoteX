# Privacy: what the infrastructure sees

Sessions are peer-to-peer wherever possible and **end-to-end encrypted always**. The server coordinates and,
only when a direct connection is impossible, relays ciphertext it cannot decrypt.

## Stored on the server (SQLite, `data\remotex.db`)

| Table | Contents | Retention |
| --- | --- | --- |
| `devices` | random device id, SHA-256 hash of the device secret, created/last-seen time, client version | until the device is removed |
| `sessions` | session id, viewer id, host id, start/end time, mode (direct/relay), relay byte count | 90 days after end |
| `authentication_attempts` | timestamp, source IP, kind (register/auth/password), success flag, device id | 30 days |
| `server_nodes`, `software_versions` | node heartbeat, versions seen | rolling |

## Held in memory only

Which device ids are online, current sessions, rate-limit counters. Cleared on restart.

## Never seen or stored

Screen content, keystrokes, mouse movement, clipboard content, file names or file contents, chat messages,
passwords (temporary or permanent), session keys.

## Logs

`server.log`, `connection.log`, `security.log` (JSON, daily rotation) record events such as "device online",
"session ended (relay bytes = n)", "authentication failed from <ip>". They never contain secrets or session
content. Delete old files from the `logs` folder according to your own retention policy.

## What another person's client learns

* A host learns the viewer's device id, device name and whether the connection is on the same network.
* A viewer learns the host's device name and its screen.
* LAN addresses of a host are disclosed **only** to a viewer that connects from the same public IP address.

## Admin dashboard

`/admin` shows counters (online devices, sessions, relay bandwidth, versions, error counts) behind a bearer
token. It has no access to session content because none exists on the server.
