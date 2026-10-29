# Android performance investigation: Galaxy S21 Plus (Exynos 2100)

## Purpose and scope

This document tracks a device-specific, static review of GLUE 5 for the Samsung Galaxy S21 Plus 5G with Exynos 2100 and Mali-G78. It is an investigation record, not a claim that the device has a measured defect or that a proposed optimization is ready to implement.

The initial findings were supplied by the user from a Kimi 3.1 static review. That review says it did not run or profile the app on the device. Claims below are therefore labeled as reported and must be independently checked against the pinned source/dependency and, where they concern performance, measured on the phone before changes are made. The supplied report did not include source URLs or profiling data.

Este é o backlog complementar à [revisão Android](android-review.md): lá ficam achados de comportamento/API e o status das correções; aqui, as hipóteses de desempenho e os critérios para medi-las. Os dois documentos devem ser mantidos em conjunto.

## Findings to verify

### 1. Vulkan surface pre-rotation — high-risk investigation

**Reported:** Android is forced to landscape by `platform/android/AndroidManifest.xml`; the presentation path does not pre-rotate; `wgpu-hal` 30.0.1 uses `preTransform = IDENTITY`. The report hypothesizes that a portrait-native phone may rely on compositor rotation, with possible extra composition, bandwidth, power, and heat.

**Proposed by the report:** use `currentTransform`, rotate the final pass, update viewport/scissor and coordinate handling, and recreate/reconfigure the swapchain when the transform changes. The report notes that the public `wgpu` API may not expose the needed setting, so a backend patch or direct Vulkan swapchain work could be required.

**Gate:** first inspect the exact pinned `wgpu-hal` source and Android/Vulkan contract; then measure the current presentation path on the S21 Plus. Do not patch the backend based only on this static claim.

### 2. ADPF work-duration reporting

**Reported:** `crates/glue5-gpu/src/submit.rs` computes `now - last_shown` and passes it through `glue5_runtime::qos::frame`, which reports it using `APerformanceHint_reportActualWorkDuration`. This interval may include presentation pacing, VSync/backpressure, or idle time rather than CPU work alone.

**Proposed by the report:** measure the actual work interval for the participating threads, exclude deliberate sleeps and presentation waits, and keep frame cadence separate from work duration. CPU/GPU durations may also need distinct metrics.

**Gate:** confirm the Android API's required semantics and trace the participating thread lifecycle before changing the metric. Collect before/after timing evidence.

### 3. 60 Hz fallback versus 120 Hz displays

**Reported:** `crates/glue5-gpu/src/present.rs::frame_budget()` uses 60 fps when the configured rate is zero. The report also says the display refresh rate is read only at startup, so a later 60/120 Hz change may not update pacing, QoS, the performance overlay, or available settings.

**Proposed by the report:** follow display refresh/VSync changes dynamically and update those consumers, using approximately 8.33 ms at 120 Hz and 16.67 ms at 60 Hz.

**Gate:** verify the intended meaning of the “Display” setting, SDL/wgpu refresh-rate facilities, and whether the current behavior is a product decision or a defect. The 60 fps fallback is not inherently wrong when 60 fps is explicitly desired.

### 4. BC texture fallback, memory, and mip quality

**Reported:** when BC compression is unavailable, textures are decoded; some color formats are then re-encoded as ETC2. Approximate per-texel storage ratios in the report:

| Source | Fallback | Approximate relative storage |
|---|---|---:|
| BC1 | ETC2 RGBA8 | 2× |
| BC2 / BC3 / BC7 | ETC2 RGBA8 | 1× |
| BC4 | R8 | 2× |
| BC5 | RG8 | 2× |
| BC6H | RGBA16F | 8× |

The report also identifies an intentional drop of the largest mip for BC 2D textures with at least two mips and a largest dimension of 512 or more. This reduces resolution as a memory tradeoff.

**Proposed by the report:** measure loading time and memory per format; evaluate ETC2/EAC/ASTC only where the actual adapter exposes the needed features; cache transcodes; and consider making the mip-quality/memory tradeoff explicit.

**Gate:** preserve alpha and color-space behavior, validate format support and rendering correctness, and measure on-device before changing the fallback.

### 5. Nested texture-decoding parallelism

**Reported:** a decode pool can run two jobs while each BC decode/ETC2 encode may create up to eight scoped threads, potentially reaching 16 inner workers, in addition to other GLUE 5 and GPU compilation threads. The report hypothesizes oversubscription, context switching, cache pressure, and extra thermal load; it does not prove these effects occur on the S21 Plus.

**Proposed by the report:** use a shared worker budget or persistent pool and compare controlled limits such as 2, 4, and 6 workers, recording texture load duration, frame times, memory, and temperature.

**Gate:** benchmark representative texture streaming before redesigning the pools. Do not assume that either maximum parallelism or a smaller worker count is always faster.

### 6. Android thermal adaptation

**Reported:** the app has a thermal watcher behind an iOS-only target condition, but no equivalent Android thermal adaptation was found in the reviewed paths.

**Proposed by the report:** investigate Android Thermal APIs and, if measurements justify it, gradually adjust resolution/upscaler, frame-rate limit, and worker counts with hysteresis; restore quality gradually as thermal headroom returns.

**Gate:** first add/collect thermal and frame-time instrumentation on the S21 Plus. Do not claim throttling or add adaptive quality solely from static inspection.

## Behaviors the report says to preserve

- Request only GPU features exposed by the adapter.
- Keep the current Vulkan 1.1 requirement unless separate compatibility evidence supports changing it.
- Keep FSR conditional on the required adapter features.
- Reapply the requested frame rate after surface recreation.
- Do not add Turnip/libadrenotools as a Mali path; those are Adreno-specific mechanisms.

These are also report claims to verify where relevant, not a substitute for the code's existing regression tests.

## Suggested investigation order

1. Confirm the current Android Build on the current branch tip and resolve any compile blocker before interpreting runtime results.
2. Verify ADPF work-duration semantics and instrument cadence versus actual work.
3. Measure texture fallback memory/load time and nested worker behavior.
4. Verify 60/120 Hz pacing and dynamic refresh changes.
5. Independently confirm the Vulkan pre-rotation claim; treat any backend/swapchain change as high risk.
6. Evaluate Android thermal adaptation and ASTC/EAC/mip options only after obtaining on-device measurements.

## Status

- This is a static-review backlog; no S21-specific GPU or thermal change has been implemented from the Kimi report.
- No device profiling or validation result is recorded yet.
- Record each investigation and its measurements here; put stable architectural decisions in `design.md`, and user-facing settings in `playing.md`.
