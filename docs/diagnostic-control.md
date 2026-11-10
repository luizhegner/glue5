# Diagnostic control and observation (v3)

Tracking: GLU-39. Target: Android Galaxy S21+ SM-G996B / Mali-G78.
This is a locally authorized diagnostic API, not an autonomous bug-finding
agent or replay recorder. V3 adds native spatial context, game-clock holds,
one-shot local reactions, latest-frame watch and composed step responses.

## Owner experience

Open **Settings > Diagnostics > Diagnóstico por IA** in the native app.
Enable **Permitir solicitações de IA** for the current app run (off again on
process restart). This only allows requests; it never grants control. The
page shows the missing prerequisites, lets the owner enable the existing
mods/Studio/LAN service, and copies the MCP address or pairing code. If mods
were off when this game session started, restart the game with mods enabled.

Choose **Formato da conexão** before **Criar acesso para um agente > Criar e copiar**.
The default Codex format exports a private `config.toml` section with
`http_headers`, and restricts enabled tools to `diagnostic_control` and `logs`
with approval prompts. Generic JSON (`mcpServers`) remains an explicit
alternative for compatible clients. These are different configuration
formats, not interchangeable. See the [official OpenAI MCP documentation](https://learn.chatgpt.com/docs/extend/mcp?surface=cli).

The action creates a named Studio pairing and puts the selected configuration
with its bearer token on the clipboard.
This is an explicit trust decision: Studio access also permits scripts/mods.
Paste only into a trusted client's MCP configuration, never into logs or
public reports. It does not configure or connect the agent automatically.
Client configuration formats vary. Remove this pairing in Mods > Studio >
Paired to revoke it. Do not create repeated credentials for the same client.
The client-side tool allowlist does not reduce the token's Studio privileges.
Append or update only the `mcp_servers.glue5` section in the private Codex
configuration; never replace the entire file, create duplicate sections or
commit a real credential. Copying does not install/activate the connection.

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

The existing MCP endpoint now advertises **diagnostic_control**. Its arguments
are the same JSON object as the HTTP operations below; its authenticated
named pairing is the owner. A raw pairing-code MCP connection cannot call
this tool. Other MCP tools are unchanged. An attached agent can request,
poll status, renew, submit bounded pad actions and stop through MCP, without
hand-writing HTTP requests. Existing `logs` reads Studio/mod logs; Android
process capture still requires the separate owner-authorized flow.

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
| hold | session_id, duration_ms, pad | Hold for 1..3000 game milliseconds, hard wall cap 4s |
| input | session_id | Revision, actual host pad reads, last buttons and reaction state |
| react | session_id, when, duration_ms, pad, timeout_ms | Arm one local condition/action, <=15s wait; still needs heartbeat |
| step | session_id, optional action, settle_ms, with_image | Submit pad/hold, wait, return context + input receipt + preview in one RPC |
| watch | session_id, duration_ms | Latest-only game preview at <=1Hz for <=15s; still needs heartbeat |
| sequence | session_id, steps | Replace input with 1..16 pad/duration_ms steps, <=4000ms total |
| prompt | session_id, input, duration_ms | Press a currently visible logical prompt for 1..1000ms |
| context | session_id | Fresh read-only character/coordinates/activity/menus/prompts |
| frame | session_id | Request next game backbuffer preview; returns ticket |
| audio | session_id | Request next 1s of game mixer output; returns ticket |
| observe | session_id | Nonblocking poll; consumes ready image/audio exactly once |
| stop | session_id | Stop caller's pending/active session and discard captures |

The lease is 5 seconds. Send heartbeats at least once per second while using
control; status polling and input submissions do **not** renew it. Stateless
HTTP connections may close between requests. If the network disappears,
one pad/prompt action becomes neutral within at most 1 second; a previously
submitted sequence ends within at most 4 seconds. The session ends by
5 seconds, restoring local ownership. The app's stop button acts immediately,
without a network round trip.

`duration_ms` is an integer 1..1000. `sequence` replaces rather than appends;
each step follows the same bound, with <=4 seconds in total. Late frames skip
elapsed steps, never replay a backlog. A neutral `pad` replaces/cancels a
sequence without ending the session. Neither actions nor observations renew
the lease. `pad` is an object
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

### V3 latency and control improvements

Use `step` instead of hand-written PowerShell loops and separate pad/sleep/
frame/sleep/observe/context calls. Optional action is
`{"kind":"hold","duration_ms":1500,"pad":{"buttons":16}}`, or kind `pad`.
Action + settle must be <=2500ms, settle defaults to100ms (max1000); preview
wait adds <=800ms. The response contains context, an actual host input-read
receipt, media pending/errors/timestamps and MCP native image/audio blocks.
It does not extend the lease. Heartbeats belong to a live controller loop,
not an arbitrary short-lived helper; helper shutdown is not proof of a
network failure. This avoids the repeated expiry observed in early tests.

`hold` uses the engine's GET_GAME_TIMER, not a guessed wall duration or a
series of remotely repeated taps. A script-clock sample must be <500ms old;
holds stop by4s wall time even if game time freezes. `input` reports actual
pad0 reads, never counts status/active polls as consumed input. It does not
prove that the script interpreted a hold correctly or that a taxi ride
started. The earlier carjacking is still a regression to validate, not a
claimed solved bug. Existing `pad` and `sequence` retain wall-clock limits.

`react` watches one condition from fresh native samples: `taxi_near` means
a taxi-model vehicle <=8m, speed <=0.5m/s, a known driver and not the player's
current vehicle; `passenger`/`vehicle` use verified seat/vehicle queries.
When satisfied it executes the explicitly supplied pad as a game-clock hold,
once, and reports `fired_once`. New pad/hold/sequence/prompt input, local Stop,
lease expiry and background cancel it. It never chooses an action, finds a
path, renews consent/heartbeat, teleports or assumes the nearest taxi is the
one dispatched. A wrong configured button can still carjack; save first.

`watch` is not video streaming: one volatile latest frame replaces the old
ready frame at <=1Hz, never queues frames or overlaps GPU readbacks. A
15-second request does not keep a disconnected controller authorized. Use
observe to consume, stop to cancel. No capture when watch/requests are off.

Map context under `context.world` comes from a fixed read-only native sampler
inside the script thread, at most4Hz with a2ms native-call budget (individual
calls/pool scans can exceed this budget). It includes native coordinates,
heading/camera rotation, region/street, actual driver/passenger distinction,
up to4 taxi-model vehicles within120m, planar distance/relative bearing/speed,
their existing entity blips and the waypoint. This is NOT all map icons or
a navigation mesh; absent/budget-exhausted fields are null, not fabricated.
Dispatch ownership, pathfinding and mission recognition remain unavailable.
Every sample is generation-scoped; samples >=1s old are omitted. The fallback
ped coordinate field is corrected from orientation +112 to translation +144,
the same build layout already used by world.rs. Confirmation on device is
still required. No proprietary game code/assets are added.

### Observation and context limits

The local approval explicitly covers control, context and on-demand game
image/audio. No additional OS capture permission: image comes from the own
game GPU backbuffer **before scaling and the GLUE overlay**, not Android's
screen. It cannot include notifications/other applications. Audio is tapped
only from the SDL callback reading the game's mixer, not the microphone or
system playback; GLUE start-page music/SFX are not included.

One request per second **per kind**, one slot each, <=3 seconds retention
in process memory, no recording/files. PNG is aspect-preserving <=640x360,
RGB, <=1MiB. Only single-sampled RGBA/BGRA8 backbuffers <=3M pixels are
supported. GPU readback is asynchronous, at most one physically in flight;
encoding uses the existing worker. Audio is one second 16kHz mono PCM16 WAV
(32044 bytes). A busy audio mutex skips samples instead of blocking playback;
an underrun may contain silence. This is a diagnostic preview, not lossless
capture or synchronized audiovisual streaming. Frame/audio have independent
capture times; do not assume lip-sync. A stopped/expired session drops slots,
rejects reads and discards late GPU completions; already delivered media
cannot be revoked from the trusted client. Pending/expired polls carry no
media. A new request is needed after expiry. Encoding/readback already in
flight can finish and release its resources after stop, without delivery.

MCP `observe` returns native image/audio content blocks; HTTP returns base64
image/audio objects. Client media support varies. Keep heartbeats running
on a separate request while waiting; never hold the network call waiting
for capture. No allocation/readback/encoding when not requested; idle paths
only check an atomic capture flag.

`context` also snapshots the existing game-control hook at most 4Hz during an
approved session, rejects snapshots from other grants or >=1s old. Character
comes from the actual ped model; coordinates use the build's existing ped
position layout. Loading/unknown peds return null character/coordinates.
Activity without a recent task signal is null. Region/street names are
available only when the native world sampler supplies them; mission remains
null. This does not infer names from coordinates. `buttons` contains actual RAGE button constants, `prompts`
contains observed labels/input IDs/pad icons; input IDs are NOT pad bitmasks.
`prompt` accepts only one of those currently observed IDs (never arbitrary
inputs), with a fresh context; it uses the same engine input path as touch.
It replaces other input, expires within 1s, cancels as soon as the prompt
disappears and never resumes on its return. A neutral `pad` cancels it too.
This allows actual taxi prompts without guessing a controller mapping.
Controls and observations still need validation on the owner's S21+.

The lightweight CI harness tests the real consent/lease/action policy and
banner geometry, renders native setup and diagnostic-strip previews and
checks modal consent. It also checks honest setup states (paired is not
connected/controlling). The session policy tests local opt-in and revocation.
The full remote suite compiles Android and tests runtime policy, protocol and
existing touch routing. No Rust, game or decoder execution on the Inspiron.

Device acceptance still required: approve/decline, tap the strip/Stop, press
Back, use all physical input classes, expire action/heartbeat, background,
revoke pairing, reconnect, and confirm no held acceleration/aim survives.
Check normal/expanded radar, build label, notches, UI scales and orientation.
Also check setup enable/disable, restart resetting opt-in, creating/copying
and revoking a named connection, and MCP discovery/errors/consent/stop.
No claim of complete automated diagnosis, image observation or safe saves
follows from a passing CPU test.

The staged device checklist is in [testing-diagnostic-control.md](testing-diagnostic-control.md).
