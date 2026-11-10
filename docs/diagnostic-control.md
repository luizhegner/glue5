# Diagnostic control ownership (v1)

Tracking: GLU-39. Target: Android Galaxy S21+ SM-G996B / Mali-G78.
This is the exclusive-control/consent part of the diagnostic API, NOT an
autonomous bug-finding agent, screen-capture API or replay recorder.

## Owner experience

With the game running and the existing Mods/Studio connection enabled, a
**named Studio pairing** may request a control session. The phone shows the
client's name, warns that gameplay can change progress and requires local
authorization (Back/decline is selected initially). The pairing alone never
grants gameplay control. Each new session asks again; restart/reconnect does
not restore a grant. Save before granting control; this feature does not
create a save sandbox or disable the game's autosave.

While approved, the top strip says **A IA está controlando o jogo** and
**Parar IA** remains touchable. Virtual controls/settings shortcuts hide,
local gameplay input is blocked, and keyboard/mouse/physical pads cannot
silently take over. The Android system navigation is not blocked. Esc/Android
Back or a controller's Back+Start combination also stops the session.
The controller emergency combination acts immediately, even before the
controller returns to neutral and while other buttons/sticks are held.
Both buttons must be on the same controller.

Stop, lease expiry, pairing revocation, Studio shutdown, focus loss and
backgrounding cancel the session and its action. Held input, edge queues,
mouse deltas/text queues and touch gestures are cleared. Sensors close;
physical controllers must return to neutral before gameplay resumes. The app
tries a pause-menu pulse if no menu/loading/alert/cutscene is already active,
without delaying input ownership return or toggling an existing pause menu.
Some game states/Online cannot pause. No automatic reconnect/resume.

## Transport

Use the **existing Studio address/port**, POST `/diagnostics/control`,
`Authorization: Bearer <paired-client-token>`, JSON request/response.
Only a named pairing with Studio scope is accepted; the raw pairing code and
Control-only pairings are insufficient. Pair first using the existing Studio
flow. No new listener, browser panel, dependencies or OS permissions.

This uses Studio's existing HTTP transport: use a trusted private network or
an explicitly arranged local tunnel. Do not expose Studio to the Internet.
Never place real tokens in docs, reports, URLs or committed scripts.

Maximum request body: 4096 bytes. Operations:

| op | Fields | Meaning |
|---|---|---|
| request | none | Create a pending request; return session_id; phone must approve within 30s |
| status | none | idle / awaiting_local_consent / active; caller's session_id only |
| heartbeat | session_id | Renew an already approved lease, never approve/resume it |
| pad | session_id, duration_ms, pad | Replace the current bounded gamepad action |
| stop | session_id | Stop caller's pending/active session |

The lease is 5 seconds. Send heartbeats at least once per second while using
control; status polling and input submissions do **not** renew it. Stateless
HTTP connections may close between requests. If the network disappears,
the action becomes neutral within at most 1 second and the session ends by
5 seconds, restoring local ownership. The app's stop button acts immediately,
without a network round trip.

`duration_ms` is an integer 1..1000. No action queue exists. `pad` is an object
with optional `buttons` (RAGE u16 bits), `left`/`right` (two finite numbers
-1..1 each), and `l2`/`r2` (finite 0..1). Omitted values are neutral.
Example **non-secret** request after local approval:

```json
{"op":"pad","session_id":123,"duration_ms":100,"pad":{"buttons":64}}
```

Input goes to pad 0; other pads are neutral during exclusive ownership.
An expired action becomes neutral even if heartbeats continue. Late requests,
foreign session owners and stale grants cannot control the game. No raw
memory writes, arbitrary shell commands or save-file operations are exposed.
Existing Studio eval/mod privileges remain trusted capabilities, not a
security sandbox against malicious already-authorized mods or clients.

## Validation and remaining gates

The lightweight CI harness tests the real consent/lease/action policy and
banner geometry, renders a diagnostic-strip preview and checks modal consent.
The full remote suite compiles Android and tests runtime policy, protocol and
existing touch routing. No Rust, game or decoder execution on the Inspiron.

Device acceptance still required: approve/decline, tap the strip/Stop, press
Back, use all physical input classes, expire action/heartbeat, background,
revoke pairing, reconnect, and confirm no held acceleration/aim survives.
Check normal/expanded radar, build label, notches, UI scales and orientation.
No claim of complete automated diagnosis, image observation or safe saves
follows from a passing CPU test.
