# GM45 D3D11 — isolated Windows validation

This nested Cargo workspace compiles the **production native D3D11 source files**
without compiling the GTA V game/WASM or the main wgpu workspace. It is not a
replacement for production runtime integration.

Requirements: Windows 10, Rust >= 1.90, MSVC linker/toolchain, and a graphics
driver supporting the D3D11 API. Results must be checked on the physical GM45;
a successful run on another GPU does not establish GM45 support.

From the repository root:

    cargo test --manifest-path tools/gm45-standalone-validation/Cargo.toml --lib
    cargo run --manifest-path tools/gm45-standalone-validation/Cargo.toml --example gm45_probe
    cargo run --manifest-path tools/gm45-standalone-validation/Cargo.toml --example gm45_sm4_draw_probe
    cargo run --manifest-path tools/gm45-standalone-validation/Cargo.toml --example gm45_present_probe

Use --offline only if Cargo dependencies are already cached. All commands are
small, opt-in tests; no entire-workspace build is necessary. The presentation
probe opens a Win32 window, clears a backbuffer and presents 60 frames, then
exits. The draw probe uses 64x64 offscreen readback to verify pixel values.

The glue5-runtime directory is explicitly a **test double**, including its
queue, memory and notifications. It must not be confused with the production
GpuRing. GPU device creation, API calls, shaders, resources and readbacks are
real. The crate's features for integration with the full runtime are not
exercised by this standalone harness.

2026-11-13 on Intel Mobile 4 Series / GM45 (WDDM 1.1):
- 67 Rust library tests passed, 1 ignored.
- Device returned feature level 10_0.
- DXGI swapchain created at 640x360; 60 Clear/Present calls completed.
- SM4.0 shaders, Draw, DrawIndexed, VB/IB/CB and readback succeeded.
- Draw centre [64, 128, 191, 255], indexed centre [128, 64, 32, 255],
  both corners [0, 0, 0, 255].
- No real game scene or production runtime was executed.

