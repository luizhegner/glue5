# Diag v3: faster control and map regression gates

Target: Galaxy S21+ G996B / Mali-G78; branch fix/mobile-ui-followup.
No builds/tests/game extraction on the Inspiron. Owner runs Android Build.

## Remote CI

Pure policy: game-clock holds,4s wall cap, expiry, real-read receipts, local
reaction one-shot/timeout/new-input cancellation, watch replacement/stop,
coordinate bearings, strict step plan limits. Full suite compiles Android
and checks the sampler's required read-only native names. None of these
proves that the taxi script interprets hold as passenger entry.

## Real-device checks (pending)

1. Refresh MCP discovery after installing. Check status.version=3; reconnect
   a stale client schema rather than assuming the old op enum describes the
   running APK. The existing pairing/diagnostic_control allowlist still fits.
2. Approve locally; continuously heartbeat from the live controller. Check
   context.world position against the actual map, street/region, heading and
   actual seat. Rotate in place: heading changes, position should not jump.
   Walking must change translation, not just a normalized orientation vector.
3. Request step with a short movement and preview; compare its elapsed_ms
   and response round trips to the old multi-command helper. Inspect
   input.non_neutral_frames, not merely accepted=true. Watch for lost taps.
4. Hold Y in a controlled situation for2 game seconds, inspect enter_control
   pressed/just_pressed and input receipts. Check neutral replacement and
   foreground/lease stop. Do not assert a carjacking fix until a real taxi
   stops, entry completes and context.world.vehicle_seat says passenger.
5. Request a taxi normally. Check nearby_taxis distance/bearing/speed, the
   observed entity blip and waypoint. A taxi model is not proof of dispatch
   ownership. Verify unavailable driver/seat/native data stays unknown.
6. Arm a SAFE short neutral/non-combat reaction first. taxi_near must fire
   once without an agent round trip, only near a stopped taxi with driver.
   Verify timeout, replacement, Stop IA, background, revocation and heartbeat
   loss; no autonomous resumption or action after a new grant.
7. Watch for<=15s while heartbeating: no backlog, <=1Hz, latest-only slot,
   no capture after stop/expiry. Compare FPS/audio with watch off/on. Image
   is still a low-res game preview, not the Android screen or all overlays.

## Not implemented / not promised

All map icons, map graph/pathfinding, mission recognition, taxi-dispatch
ownership, a video-fed real-time model or arbitrary autonomous play. The
model can still take seconds between tool results. Local bounded conditions
and one-call steps reduce orchestration latency; they do not speed inference.
