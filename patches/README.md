# Patches to third-party code

`mise run setup` applies the patches in file order to pinned upstream
sources.

| Directory | Upstream | Result |
|---|---|---|
| `wabt/` | wabt at the commit of the `third_party/wabt` submodule | `wasm2c`, built into `target/glue5-tools/bin` for the game's build script |
| `wgpu/` | wgpu from crates.io, the version and checksum in `.mise/tasks/setup` (`crates`) | `third_party/wgpu` (ignored by git), used through `[patch.crates-io]` in the workspace manifest |
| `wgpu-core/` | wgpu-core from crates.io, likewise | `third_party/wgpu-core`, likewise |
| `wgpu-hal/` | wgpu-hal from crates.io, likewise | `third_party/wgpu-hal`, likewise |
| `sp-fidelity/` | sp-fidelity (FSR 2) from crates.io, likewise | `third_party/sp-fidelity`, likewise |

Each patch message explains the change and its reason. To update an
upstream dependency, change the submodule commit or `crates` in
`.mise/tasks/setup`, then run `mise run setup`. Refresh any patches that no longer apply by editing
the prepared sources and running `git diff` in that directory.

We would like to submit these changes upstream so we can remove the local
patches as they are accepted.

Each patch is offered under the license of the project it changes, not
GLUE 5's, so it can be submitted upstream as it is.
