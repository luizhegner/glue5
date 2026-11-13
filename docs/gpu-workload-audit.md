File: C:\k8r\glue5-r5-evolution\docs\gpu-workload-audit.md
# GLUE5 GPU workload audit (Android / Vulkan)

This is an opt-in diagnostic build for the GM45 D3D11 feasibility investigation.
It does not change the normal renderer path or the default Android build.

## Separate Android installation

The normal Android build is **GLUE 5** with application ID `local.glue5`.
Only a build explicitly created with **GPU workload audit: enabled** becomes
**GLUE5 GPU Audit** with application ID `local.glue5.gpuaudit`. The Java/AIDL
namespace remains `local.glue5` in both variants. This keeps the diagnostic APK
side-by-side with the normal app, with separate settings, cache and private files.

## Build

Run **Actions -> Build** on branch `diag/gpu-workload-s21`:

- Platform: `android`
- GPU workload audit: enabled

The normal Build workflow obtains the guest `game.wasm` from the repository's
private draft release and compiles it into the app. This is the same build-time
guest-code path as a normal GLUE5 Android build; it is not a game server.

At runtime, choose **Folder** as the source and use **Find files** to select
the existing local game-data folder on the phone. The game data is read from
that folder; the diagnostic app does not stream it from a server and does not
copy the whole game into its own app directory.

Important Android storage limitation: the diagnostic app can reuse the same
folder directly when it is in shared storage and visible to both apps. Android
does not let a different package directly read the normal app's private
`Android/data/local.glue5` directory. If the only copy is inside that private
directory, do not uninstall either app or start a 20+ GB copy blindly; first
identify the exact folder location and decide on a safe shared-storage move.

## Capture on the S21+

1. Install the diagnostic APK alongside the normal GLUE5 app. Its distinct
   application ID prevents it from overwriting the normal installation.
2. Start **GLUE5 GPU Audit**.
3. Select the existing local game-data folder with **Find files**. Do not select
   a server source. The folder must be readable by both packages.
4. Reproduce the same stages where practical: startup, main menu, loading, at
   least one playable scene, and a sustained gameplay segment. Record the
   active scene and approximate time.
5. Use the app's existing Shizuku logcat capture UI to save the GLUE5 GPU Audit
   process log. The capture is PID-filtered to this app process.
6. Share the resulting text log for analysis.

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
- `FLOW_COPY`: a compute-produced resource was propagated through
  `CopyRegion`, `CopyResource`, resolve, or `CopyStructureCount`.
- `FLOW_READBACK`: a compute-produced resource was copied to CPU-visible
  staging storage.
- `FLOW_DRAW`: a successful draw used a resource most recently written by a
  compute shader, including vertex/index buffers and indirect draw arguments.

The report interval is five seconds in this feature build. There is no log call
for every dispatch; per-dispatch information is aggregated in memory.

The ordinary five-second `gpu:` performance line also reports BC fallback load:
number of BC uploads and 4x4 blocks, compressed source MiB, blocks re-encoded
to ETC2, total/longest flush wait, and how many decodes had to run on the
command thread. Use those counters before changing decode parallelism: high BC work
without waits is throughput, while high wait time is directly visible frame pressure.

The companion `gpu srv:` line counts texture-view requests, cache hits and DXGI
format resolutions during the same reporting interval. A high cache-hit rate with
nearly as many format resolutions as requests means the view cache is working but
format/compatibility work still runs before it; use that evidence before changing
the SRV cache key or bypassing `misfit` validation.

## Interpretation limits

- Resource IDs are command-ring object IDs, meaningful only within one app run.
- Flow tracking records the most recent compute writer for each resource ID
  and propagates that association through the copy/resolve paths listed above.
  It is a useful dependency signal, not a complete GPU memory-dependency graph:
  CPU updates, aliases/views, clears, and other engine-side transformations
  still need separate interpretation.
- A `FLOW_DRAW` means the resource was bound or used as a vertex/index/indirect
  argument buffer by a successful draw. It does not prove that every byte
  affected visible pixels.
- Shader source indicators are textual heuristics. The WGSL dump is the
  authoritative material for classifying atomics, workgroup memory, barriers,
  scatter/gather patterns, and exact storage access.
- The S21+ workload is representative of the guest only if the game does not
  change its command stream based on adapter capabilities. The adapter and
  driver diagnostics in the same process log must be retained with the capture.
- This capture describes the S21+ Vulkan path. It does not measure GM45
  performance and does not prove that a shader is convertible to SM 4.0.

Keep the `gpu-workload-audit` feature diagnostic-only and disabled in normal release
builds; enable it explicitly for capture APKs and review its output before changing
renderer policy.

