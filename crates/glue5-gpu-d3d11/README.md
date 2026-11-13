# Native D3D11 backend primitives

This crate is the Windows-only native D3D11 side of the GM45 investigation. It is intentionally separate from the current wgpu renderer. D3D11 COM interfaces are owned by this crate; they are not wrapped in, cast to, or shared as wgpu handles.

## Current scope

- Open a hardware D3D11 device (no WARP fallback), report its actual feature level and query format capabilities.
- Create restricted 2D textures with explicit SRV and/or RTV views, and clear render targets.
- Create explicit SRVs for base-mip, single-slice 2D textures and typed buffer ranges.
- Bind SRV ranges to vertex and pixel stage slots on this GM45 FL10_0 device; a zero object ID explicitly unbinds a slot. Compute-stage SRV binding is rejected below FL11_0.
- Create sampler state objects from the engine's 13-word `CREATE_STATE` sampler payload and bind them to VS/PS slots with `SET_SAMPLERS`. Compute-stage sampler binding is rejected below FL11_0.
- Map min/mag/mip point/linear filter bits, anisotropic and comparison filter flags, address modes, anisotropy, comparison function, border color and LOD clamps into `D3D11_SAMPLER_DESC`.
- Create D3D11 buffers with the engine's vertex, index, constant-buffer and shader-resource binding bits. Buffer UAV binding and unknown flags are rejected.
- Upload byte ranges into buffers. D3D11's 4-byte offset/range alignment is enforced; a final logical tail shorter than four bytes is padded inside the aligned physical allocation. Constant buffers require a full-buffer update.
- Upload regions into a restricted set of uncompressed 2D texture formats with `UpdateSubresource`. The current direct bytes-per-texel table is deliberately explicit; compressed and typeless formats are rejected.
- Consume a small `GpuRing` allowlist: `NOP`, `INIT`, restricted `CREATE_TEXTURE`, `CREATE_BUFFER`, `UPLOAD_BUFFER`, `UPLOAD_BUFFER_NO_OVERWRITE`, restricted 2D `UPLOAD_TEXTURE`, `CREATE_SRV`, `SET_SRVS`, sampler `CREATE_STATE`, `SET_SAMPLERS`, `DESTROY_OBJECT`, `CLEAR_RT` and `FENCE`.
- Own textures, buffers, SRVs and samplers by numeric object ID, reject ID collisions, and destroy them deterministically.
- Use a lazily-created reusable D3D11 event query for `FENCE`; the worker signals the engine only after GPU completion.

`CREATE_SRV` currently supports only:
- Texture SRV: D3D11 `TEXTURE2D`, mip 0, array slice 0, same DXGI format as the resource (or format 0 meaning the resource format).
- Typed buffer SRV: D3D11 `BUFFER` with a supported typed DXGI format, such as `R32_FLOAT`, and a valid element range.

The GM45 exposes D3D Feature Level 10_0. A physical test of `D3D11_RESOURCE_MISC_BUFFER_STRUCTURED` failed at `CreateBuffer` with `E_INVALIDARG`, so structured buffer resources are rejected below FL11_0. Raw buffer SRVs, structured buffer SRVs, and buffer UAVs are not supported. `CREATE_UAV` and `SET_UAVS` are explicitly rejected on this backend; no compute/UAV capability is being claimed.

`UPLOAD_BUFFER_NO_OVERWRITE` is currently preserved as an ordered update but uses the same DEFAULT-resource `UpdateSubresource` path as `UPLOAD_BUFFER`. A dynamic-buffer Map/NO_OVERWRITE strategy is not implemented yet.

The texture upload path currently accepts only mip 0, array slice 0, z 0, depth 1 (or the engine's zero value, which has depth-one semantics), and supported uncompressed formats. Row pitch may include padding. Compressed BC formats, A8 CPU expansion, UNORM16 conversion, texture arrays, 3D textures and nonzero mip uploads are not implemented.

## Hardware validation

The example `gm45_probe` was compiled and executed on the Dell Inspiron 1545's physical adapter:

- Adapter: Mobile Intel 4 Series Express Chipset Family
- PCI ID: 8086:2A42
- Driver: 8.15.10.2702, WDDM 1.1
- Feature level: 10_0
- `R11G11B10_FLOAT` support mask: `0x0006f3f3`; Texture2D + shader-load supported.
- `R32_FLOAT` support mask: `0x0016f5fb`; Texture2D + shader-load + render-target supported.

The example creates the three diagnostic textures (240×108, 15×7 and 1×1), clears two render targets, and validates:
- A 64-byte vertex buffer with stride 16, two partial updates (12 bytes at offset 8 and 8 bytes at offset 28), and all 64 bytes compared through a CPU-readable staging buffer.
- A 15-byte logical buffer backed by a 16-byte aligned allocation. An unaligned offset is rejected; a 3-byte final update at offset 12 is safely padded and read back.
- An 8×4 RGBA8 texture upload with a 36-byte source pitch (32 useful bytes + 4 padding bytes per row). Staging readback matches all 128 texel bytes, excluding source padding.
- A texture2D SRV and a typed R32_FLOAT buffer SRV, bound to vertex and pixel stages and explicitly unbound.
- A linear sampler using wrap/mirror/clamp addressing, bound to VS/PS and unbound. Compute-stage SRV and sampler binds are both rejected at FL10_0.
- Two event-query waits and explicit destruction of every resource/view/state.

Observed output:

```text
D3D11 hardware device opened
Feature level: Level10_0
R11G11B10_FLOAT: support=0x0006f3f3, Texture2D+shader-load=true
R32_FLOAT: support=0x0016f5fb, Texture2D+shader-load=true, Texture2D+RTV=true
PASS: D3D11 event query completed on GM45
Resources created: source=240x108, partials=15x7, final=1x1
PASS: texture and typed-buffer SRVs created and bound to VS/PS on GM45
PASS: compute-stage SRV binding rejected on GM45 Feature Level 10_0
PASS: sampler state created, bound to VS/PS and unbound on GM45
PASS: compute-stage sampler binding rejected on GM45 Feature Level 10_0
PASS: partial D3D11 buffer uploads round-tripped through staging on GM45
PASS: final partial buffer word is padded safely
PASS: pitched RGBA8 texture upload round-tripped through staging on GM45
Commands: 27, texture creates: 4, texture uploads: 1, texture source bytes: 144, buffer creates: 3, buffer uploads: 3, uploaded bytes: 23, SRVs: 2, SRV binds: 4, SRV slots: 8, samplers: 1, sampler binds: 4, sampler slots: 4, clears: 2, flushes: 2
PASS: typed D3D11 executor on GM45
PASS: explicit resource destruction
```

## Controlled GpuRing worker validation

The isolated example `gm45_ring_fence_probe` sends 27 commands through the production worker function while using a small runtime test double for the ring memory/reader API:

1. Create and clear a render-target texture.
2. Create a vertex/SRV-capable typed buffer and perform `UPLOAD_BUFFER` plus `UPLOAD_BUFFER_NO_OVERWRITE`.
3. Create and upload a second RGBA8 texture.
4. Create a texture SRV and a typed R32_FLOAT buffer SRV; bind them to VS/PS slots.
5. Create a sampler through `CREATE_STATE`, bind it to VS/PS with `SET_SAMPLERS`, then unbind sampler and SRV slots with zero IDs.
6. Complete two FENCE operations and destroy the views, sampler and resources.

The physical D3D11 device and driver execute resource creation, updates, clears, SRV/sampler creation and binding, and event queries. The test double supplies only the queue, command memory and notification capture. Result: 27 commands consumed, two notifications in order, worker remains Running, zero ring errors. This is not a substitute for a test using the full runtime ring.

The crate's decoder suite currently passes 22/22 tests in the isolated validation workspace.

## Validation workspace and build

The examples are compiled through `C:\k8r\gm45-d3d11-rust-validation`, whose Cargo manifest points directly to this crate's source files. Its local runtime test double avoids the root workspace's full game build.

After `mise run setup` has prepared the pinned third-party dependencies required by the root workspace:

```powershell
cargo run -p glue5-gpu-d3d11 --example gm45_probe
cargo fmt --package glue5-gpu-d3d11 -- --check
```

The root workspace's full `cargo check` remains unverified: translating the approximately 63 MB game WASM with `wasm2c` exceeded the Inspiron 1545's available RAM, so that process was stopped to protect the machine. The game files were not changed.

## Important limits

This is **not yet a GLUE5 renderer backend**.

- Texture creation remains restricted to 2D, depth 1, one mip, one array slice and one sample. `CLEAR_RT` only accepts mip 0 / slice 0.
- Texture SRVs are restricted to base-mip/single-slice 2D views without format reinterpretation.
- Buffer SRVs support only typed formats from the explicit bytes-per-texel table. Structured/raw buffer views are unavailable on this GM45 FL10_0 device.
- UAV creation/binding and compute execution are unavailable in this backend at Feature Level 10_0; `CREATE_UAV` and `SET_UAVS` are rejected explicitly.
- Sampler creation and binding are implemented for VS/PS; CS-stage sampler binding is rejected below FL11_0.
- SRV binding is implemented for VS/PS. CS-stage SRV binding is rejected below FL11_0; the GM45 physical probe exercises VS and PS. Automatic resource hazard tracking/unbinding across render-target and SRV bindings is not implemented yet; the test explicitly unbinds SRVs before resource destruction.
- Shader creation, constant-buffer binding, input layouts, fixed-function pipeline states, vertex/index binding, draw, dispatch, engine query/readback opcodes and present are not implemented.
- Constant buffers are not directly bound yet. The current wgpu executor repacks source constant buffers per shader using `shader::Pack`; `SET_CBUFFERS` also carries a first-constant offset in 16-byte units. Binding the original whole D3D11 buffer would ignore that offset and can supply incorrect constants. The native path must reproduce the per-shader packing (or validate a compatible range-binding alternative) before implementing `SET_CBUFFERS`.
- The staging readback methods exist only for hardware diagnostics; they are not wired to the engine's `READBACK` opcode.
- `UPLOAD_BUFFER_NO_OVERWRITE` has ordered-update semantics but is not yet implemented with D3D11 dynamic-buffer mapping.
- The worker's ring/reader test uses a runtime test double. The full runtime ring and root workspace still need validation without triggering the memory-heavy game-WASM translation.
- The app's current `Screen` / window / presentation path still belongs to wgpu.
- The two luminance pixel-shader replacements remain in the separate `tools/gm45-luminance-test` harness and are not yet routed by the native executor.

The existing wgpu backend, main app, S21+ GPU diagnostic worktree and UI-radio integration worktree remain unchanged. This research backend remains opt-in and must not be selected for GTA V.

## Next integration step

Add constant-buffer binding, vertex/pixel shader creation, input layouts and fixed-function rasterizer/blend/depth-stencil states. Then build a deliberately minimal VS/PS draw path into a native render target with staging readback. Keep automatic SRV/RTV hazard tracking explicit in that work. Do not attempt the compute/UAV path on this FL10_0 adapter; the two validated luminance compute fallbacks need a pixel-shader route.
