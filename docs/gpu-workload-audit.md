# GLUE5 GPU workload audit (Android / Vulkan)

This is an opt-in diagnostic build for the GM45 D3D11 feasibility investigation.
It does not change the normal renderer path or the default Android build.

## Build

Run **Actions -> Build** on branch `diag/gpu-workload-s21`:

- Platform: `android`
- GPU workload audit: enabled

The build is signed with the repository's existing Android draft-release key and
uses the `game.wasm` fetched by the normal Build workflow. The APK and build
metadata include `gpu-audit` in their names so they are not confused with a
normal build.

## Capture on the S21+

1. Install the diagnostic APK as an update to the existing GLUE 5 installation.
   Do not uninstall the existing app to work around a signing mismatch; stop and
   investigate the signing key first because uninstalling can delete local game data.
2. Start GLUE 5 and the same game build.
3. Capture these stages separately in one session where practical: startup,
   main menu, loading, at least one playable scene, and a sustained gameplay
   segment. Record what scene was active and approximately when.
4. Use the app's existing Shizuku logcat capture UI to save the GLUE 5 process
   log. The capture is PID-filtered to the GLUE 5 process.
5. Share the resulting text log for analysis.

No launch-time environment variable is required. The audit is compiled into
this diagnostic APK only.

## Log records

All audit records start with `GLUE5_GPU_AUDIT|`:

- `BEGIN`: audit schema marker.
- `SHADER`: stage, shader hash, WGSL-variant hash, label, bindings, and simple
  source indicators for atomics, workgroup memory/barriers, and texture stores.
- `WGSL`: source of compute shaders and graphics shaders that declare storage
  resources, split into numbered chunks. Reassemble chunks by shader hash and
  variant, ordered by part.
- `RESOURCE`: first observed size/stride or dimensions/format/sample count for
  resources used by compute.
- `COMPUTE`: dispatch count, frame range, group dimensions, and read/write
  bindings in the reporting interval. The read/write field is a list of
  `binding:resource-id:count` tuples.
- `FLOW_COMPUTE`: a compute shader read a resource most recently written by a
  compute shader.
- `FLOW_DRAW`: a draw used a resource most recently written by a compute shader.

The report interval is five seconds in this feature build. There is no log call
for every dispatch; per-dispatch information is aggregated in memory.

## Interpretation limits

- Resource IDs are command-ring object IDs, meaningful only within one app run.
- Flow tracking records the most recent compute writer for each resource ID.
  It is a useful dependency signal, not a complete GPU memory-dependency graph:
  copies, CPU updates, aliases/views, and indirect argument consumption need
  separate interpretation.
- A `FLOW_DRAW` means the resource was bound or used as a vertex/index buffer
  by a successful draw. It does not prove that every byte affected visible
  pixels.
- Shader source indicators are textual heuristics. The WGSL dump is the
  authoritative material for classifying atomics, workgroup memory, barriers,
  scatter/gather patterns, and exact storage access.
- The S21+ workload is representative of the guest only if the game does not
  change its command stream based on adapter capabilities. The adapter and
  driver diagnostics in the same process log must be retained with the capture.
- This capture describes the S21+ Vulkan path. It does not measure GM45
  performance and does not prove that a shader is convertible to SM 4.0.

Keep this branch and APK diagnostic-only. Do not merge it into `main` or
publish it as a normal game release without a separate review.
