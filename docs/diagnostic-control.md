# Diagnostic control ownership (v1)

Tracking: GLU-39. Target: Android Galaxy S21+ SM-G996B / Mali-G78.
This is the exclusive-control/consent part of the diagnostic API, NOT an
autonomous bug-finding agent, screen-capture API or replay recorder.

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
