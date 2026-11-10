# Android diagnostic v2 acceptance

Build `fix/mobile-ui-followup` after CI checks. No local compilation on the
Inspiron. Save before authorizing: input is real, not a save sandbox.

1. Open Diagnóstico por IA, enable requests, return to gameplay. Existing
   named pairing/config remains valid; refresh the client's tool discovery.
2. Agent calls `request`. Confirm the new local notice explicitly mentions
   game image/audio/context, no microphone/other apps. Declining must yield
   no control or capture. Approve only when ready.
3. Heartbeat each second. Call `context`: check actual character, plausible
   coordinates/activity, current prompts. Location and mission names are
   intentionally unknown, not made up.
4. Request `frame` and `audio`, poll `observe` while keeping heartbeat alive.
   Verify game-only image and sound. No second request while pending; no
   continuous recording. Repeated `observe` must not redeliver media.
5. Use two short pad steps separated by a neutral step. Observe the result,
   then cancel with a neutral `pad`. Do not start with long driving/shooting
   actions. For the taxi, verify the actual displayed prompt/mapping first.
6. During pending capture/sequence, use Parar IA / Android Back, background
   the app, revoke pairing and separately test lost heartbeat. Each must
   release inputs and reject capture/context reads. A fresh request/consent
   is necessary afterwards; no old frame should arrive in a new session.
7. Compare frame pacing/audio with capture off and isolated requests on
   S21+ Mali-G78. Report readback failure or degradation rather than raising
   the frequency. Check game resolutions up to/above the 3M-pixel boundary.

Not yet validated on device. CPU/GPU CI proves bounded policy/encoding, not
Android playback quality, visual correctness or performance.
