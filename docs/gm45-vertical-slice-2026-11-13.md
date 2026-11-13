# GM45 D3D11 — branch audit and hardware milestones (2026-11-13)

## Sources and isolation

- Base: origin/main at 78b4e5d; unchanged.
- Main implementation branch requested: experimental/gm45 was **not found**
  among local or fetched origin refs on 2026-11-13. Its available implementation
  survived as **untracked source files** inside the
  research/gm45-compute-fallbacks worktree. Those sources were copied into an
  independent worktree instead of deleting, resetting or committing unrelated
  local changes.
- Research evidence: commits afa2a91 and a786c70, carried via cherry-pick.
- Independent implementation: feat/gm45-d3d11-vertical-slice in
  C:\k8r\glue5-gm45-implementation (based on origin/main, no Android merge).
- The Android integration/s21-r5-mobile branch was not modified.

The research branch's already modified root Cargo files and runtime/wgpu files
were deliberately **not** copied into the independent implementation. They need
a separate, reviewed port after standalone validation.

## Feature audit

| Component | State | Evidence and boundary |
|---|---|---|
| Hardware D3D11 device | Hardware validated | GM45 returns FL10_0, no WARP |
| Swapchain + window + Clear/Present | Hardware validated | Legacy DXGI swapchain; C++ research preview and new Rust 60-frame executable |
| VS/PS SM4.0 | Hardware validated for two synthetic pipelines | FXC compiles, driver creates and runs shaders |
| Draw and DrawIndexed | Hardware validated in offscreen probe | 64x64 readback checks centre and corner |
| Vertex/index buffers + input layout | Hardware validated for probe | Indexed triangle passes |
| Constant buffer | Hardware validated for probe | 16-byte tint changes indexed output; engine Pack binding incomplete |
| 2D textures / SRV / sampler / RTV | Hardware validated, restricted | Restricted formats/mips/bindings only |
| R11G11B10 texture input + R32 luminance pixel fallback | Research probe validated | C++ separate render-pass harness, not yet routed from engine |
| Event query/FENCE | Physical probe, partial ring | Driver query + isolated worker tests, full runtime unvalidated |
| Ring opcode decoder | Implemented, unit tested | Restricted allowlist; no full-game execution |
| Draw opcodes through production ring | Probe validated in research; not on active runtime | Native pipeline tests exist but root runtime integration not validated |
| DXBC signature / shader resolver | Partially implemented | Small fixture tests; broad real shader corpus compatibility not proven |
| WGSL → HLSL compiler | Research implementation | Selected shader pairs only; not a general production pipeline |
| SRV/RTV hazards and resource replacement | Partially implemented | Some state handling; comprehensive combinations unverified |
| Blend / depth / raster pipeline | Partial | Raster no-cull probe; full fixed-function command coverage missing |
| BC textures, arrays, 3D, extra mips, depth formats | Absent | No general GLUE5 support demonstrated |
| UAV / compute | Unsupported in this FL10_0 path | Requires selective render-pass/CPU fallbacks, not native compute |
| Runtime-selected GM45 renderer | Absent | Do not enable backend in game |
| Game scene / GTA V | Not validated | Not claimed |

## Independent hardware reproduction

On the physical Dell Inspiron 1545 with Intel Mobile 4 Series:
- Rust library test suite: 67 passed / 1 ignored / 0 failed.
- gm45_probe: true driver FL10_0, buffer/texture uploads, SRVs and samplers,
  format checks and event query passed.
- gm45_present_probe: DEVICE Level10_0; SWAPCHAIN OK 640x360;
  PRESENT OK frames=60; exit 0.
- gm45_sm4_draw_probe: Draw centre [64,128,191,255], corner [0,0,0,255];
  DrawIndexed centre [128,64,32,255], corner [0,0,0,255]; exit 0.

These results prove executed API calls and completed presentations, **not**
visible frame-content correctness of the window (no screenshot assertion),
nor engine runtime or game compatibility.

## Next stages

1. Make presentation resize-aware and use it as the display target of the
   native GraphicsExecutor, keeping the same device.
2. Re-run the real GpuRing reader/pipeline probes against the new tracked
   implementation. Preserve and differentiate test doubles.
3. Complete engine constant-buffer packing, fixed-function states and
   SRV/RTV hazard handling, with focused tests.
4. Add minimal ring-generated triangle to swapchain, then first real GLUE5
   resource frame. Keep runtime opt-in; do not assume performance.
5. After a representative shader workload is verified, route the bounded
   luminance pixel fallback by shader hash/resource contract.

