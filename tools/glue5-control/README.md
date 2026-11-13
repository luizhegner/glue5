# GLUE5 Diagnostic Control — MVP v1

This is a **local, opt-in QA harness**, separate from GLUE5 Studio's mod-authoring MCP
endpoint. The adapter is deliberately independent of the GPU backend and game variant.

## What is implemented

- Versioned JSON-lines control protocol over TCP **127.0.0.1 only**.
- Required `GLUE5_DIAGNOSTIC_TOKEN`, 16–256 printable ASCII bytes, on **every request**.
- `status`, `get_settings`, `get_metrics`, `request_capture`, `capture_status`,
  `input_press` and `input_axis`.
- A bounded command mailbox between the socket and headless game loop.
- Structured button names (including `INPUT_FRONTEND_ACCEPT`,
  `INPUT_FRONTEND_CANCEL`, `INPUT_FRONTEND_PAUSE`, `INPUT_JUMP`),
  with a strict pad-button allowlist. Analogue sticks have magnitude and duration limits.
- On-demand PNG screenshots using the **existing GPU screenshot pipeline**.
  Captures go to a fresh per-session directory beneath the headless user directory.
- GPU frame time, render time, GPU time when available, draws, passes, submits,
  allocated GPU memory, and audio underruns/silence/update waits.
- Standard-library Python CLI and stdio MCP adapter; neither can run shell commands.

## Running

Requires an existing compatible GLUE5 game package and a built `headless` example.
No game files or game download are included.

In PowerShell, generate a new token **locally** and retain it in the same
environment used by the harness and client:

```powershell
$env:GLUE5_DIAGNOSTIC_TOKEN = python -c "import secrets; print(secrets.token_urlsafe(32))"
$env:GLUE5_DIAGNOSTIC_PORT = "49355"

# Example only: requires a built executable and a compatible game directory
.\target\debug\examples\headless.exe --game "C:\path\to\glue5-game" --diagnostic-port 49355 --touch
```

In another shell, provide the same token and run the CLI:

```powershell
python tools\glue5-control\glue5_control.py status
python tools\glue5-control\glue5_control.py settings
python tools\glue5-control\glue5_control.py metrics
python tools\glue5-control\glue5_control.py press INPUT_FRONTEND_ACCEPT --ms 120
python tools\glue5-control\glue5_control.py axis left 0.25 -0.75 --ms 500
python tools\glue5-control\glue5_control.py capture --wait 10
```

MCP clients that can launch a **local stdio MCP server** can run
`python tools/glue5-control/glue5_mcp.py`. The launcher must supply
`GLUE5_DIAGNOSTIC_TOKEN` and optionally `GLUE5_DIAGNOSTIC_PORT`.
Remote ChatGPT connections would require a separately secured bridge; the
MCP adapter does **not** open a network listener.

## Wire protocol

Each connection carries **one** UTF-8 JSON request, followed by a newline.
The request is limited to 8 KiB; the connection then closes after one
response. There are no arbitrary file paths, memory addresses, or shell operations.

Request:

```json
{"v":1,"id":"run-001","token":"<session-token>","op":"input_press","args":{"control":"INPUT_JUMP","duration_ms":100}}
```

Successful response:

```json
{"v":1,"id":"run-001","ok":true,"result":{"accepted":true,"control":"INPUT_JUMP","duration_ms":100}}
```

Failure:

```json
{"v":1,"id":"run-001","ok":false,"error":"operation not allowed: exec"}
```

`request_capture` returns a `capture_id` immediately after queuing.
`capture_status` reports `ready: true` only after the PNG exists.
No capture is promised until a frame is actually presented. The returned
path is on the **GLUE5 host**, not necessarily on the MCP client's machine.

## Important semantics and limits

- `runtime_input_ready` means the runtime has exposed its input interface,
  **not** that the world has finished loading. `game_loaded` is currently null.
- `get_settings` reads headless startup settings and **does not mutate them**.
- Audio counters are reset by `get_metrics`; results describe the interval
  since the last call. GPU timing can be absent depending on the GPU/backend.
- A diagnostic frame is the rendered game image, not necessarily the complete
  application-level touch HUD; real Android UI snapshots are not yet integrated.
- Input is executed on the headless harness loop, subject to its 10 ms polling
  cadence. This is **not yet deterministic frame-indexed replay**.
- The protocol's current endpoint runs in the **headless desktop harness**.
  Full windowed GLUE5, Android lifecycle/touch, Shizuku, and GM45 integrations
  are future adapters, not validated by these changes.
- No arbitrary `load_save`, `exec`, file read/write, pointer access or
  remote LAN listening is provided.

## Validation

```powershell
python -m unittest discover -s tools\glue5-control -p "test_*.py" -v
rustfmt --edition 2024 --check --config skip_children=true crates\glue5\examples\headless.rs
```

The Python tests cover protocol framing and MCP tool restrictions using a
**mock socket**, not the real engine. For an end-to-end verification, prepare
the renderer's patched dependencies and run the headless harness with a compatible
game package. Avoid a full build on low-memory machines unless necessary.

## Next increments

1. Share the protocol endpoint with the full windowed/Android app lifecycle,
   with loopback/ADB forwarding and explicit app opt-in.
2. Expose read-only semantic UI tree and touch layout, then add touch injection
   with safe-area validation and physically normalized coordinates.
3. Add timestamped/versioned scenario recording, replay, step, repeat, wait
   and assertions. Include reproducible evidence manifest and frame comparison.
4. Expand GPU, audio and streaming telemetry while keeping explicit allowlists.
