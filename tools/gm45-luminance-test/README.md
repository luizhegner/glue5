# GM45 luminance fallback — hardware validation harness

This is a standalone, Windows-only D3D11 test for the two captured GTA V compute shader families that calculate average luminance. It is deliberately kept outside the GLUE5 runtime: it validates the pixel-shader algorithm and resource formats before any renderer integration.

## What is tested

- Full-screen triangle vertex shader compiled as `vs_4_0`.
- Tile reduction compiled as `ps_4_0`.
- Final reduction compiled as `ps_4_0`.
- Two-pass flow: source SRV → R32_FLOAT RTV/SRV partials → 1×1 R32_FLOAT RTV.
- The source format used by the capture: `R11G11B10_FLOAT`.
- CPU-vs-GPU readback comparison for several source dimensions, including a 16×16 partial grid.
- NaN and infinity filtering.

The HLSL entry points are replacements only for these two compute shader families. This is not a general compute-to-pixel-shader translator.

## Build

Requirements: Windows 10/11, CMake 3.20+, Visual Studio C++ Build Tools, Windows SDK with `fxc.exe`, and a D3D11 hardware adapter.

From this directory:

```powershell
cmake -S . -B build -G "Visual Studio 17 2022" -A x64
cmake --build build --config Release --parallel 2
```

CMake locates `fxc.exe`, compiles the three entry points, and copies the resulting `.cso` files beside each executable.

## Run

```powershell
.\build\Release\gm45_luminance_test.exe
.\build\Release\r11_format_probe.exe
.\build\Release\r11_luminance_test.exe
```

The default executable path is used to locate the shader bytecode, so no extra argument is needed.

Or run all three through CTest:

```powershell
ctest --test-dir build -C Release --output-on-failure
```

These are hardware tests, not software-renderer-only CI tests. The output reports the adapter and D3D feature level used by the device.

## Captured shader mapping

| Stage | Captured compute family | WGSL/program hash | Original resource bindings |
|---|---|---|---|
| 1 | avgLuminanceCompute::LuminanceDownsampleInitCS | 9b8e0c6c85e065da | SRV t3 → UAV u1 |
| 2 | avgLuminanceCompute::LuminanceDownsampleCS | 7a1e45a48ffaaf08 | SRV t2 → UAV u1 |

Each stage was recorded with 16×16 workgroups. Stage 1 writes each tile contribution divided by the full original image area. Stage 2 only sums the partials; it must not divide a second time. The second source grid is constrained to at most 16×16 for the captured single-dispatch pattern.

## GM45 validation record

See [VALIDATION-RESULTS.md](VALIDATION-RESULTS.md) for actual hardware results, tested format support masks, comparisons and limitations.

## Scope and non-goals

This test does not validate GLUE5's resource state machine, shader binding translation, command-ring handling, or complete renderer. It does not prove that a native D3D11 backend exists. The more complex `tiled_lighting::DepthDownsampleCS_Default` family is not included.
