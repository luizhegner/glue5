# UI checks (GitHub Actions)

The `UI layout tests` workflow compiles and tests the actual production
`theme`, `widgets`, `shell`, `popup`, `menu` and `spinner` modules. It does
not build the game, SDL, wgpu or an APK. Platform services and menu data
are explicitly isolated fixtures in `src/main.rs` and `src/ui.rs`.

The owner's PC is not a build machine. Run this workflow on GitHub Actions,
not Cargo/Rust locally. The workflow checks both touch and keyboard variants
and uploads `ui-previews` with CPU-rendered images of real egui output.
Settings previews use representative controls, not the full Settings page.
These previews do not simulate Android fonts supplied by the player, the
3D background, driver rendering, keyboard events from SDL or device DPI.

Regression coverage includes safe-area panel geometry, keyboard slider input,
row/control bounds and non-overlap, the Resume navigation route, launch
availability and confirmation actions remaining above a landscape keyboard.

The normal full application tests and the owner's manual Android build/device
check are still required before integration. Passing this small harness does
not claim that every application module or the APK has been validated.
