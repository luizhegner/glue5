File: C:\k8r\glue5-r5-evolution\docs\grandv-r5.md
# GrandV Player Fixes r5 integration

Status: experimental runtime variant for build `8b0b5899ed`. The normal GLUE 5 baseline remains the default.

## Known binaries

| Variant | Bytes | SHA-256 |
|---|---:|---|
| `playgta5-baseline` | 63,201,802 | `11ca8d2c04c5e843d18ff4aea4899d72c86973c6b031df334e67c446b2ae83e0` |
| `grandv-r5` | 63,203,467 | `fd78a6434c3d2ad2965d40b10a4ced74ba286249929b8f50ecfe5277371ad2c8` |

The r5 patch manifest names a different source `game.wasm` hash (`dcd3dc…`) than GLUE 5's current baseline. Therefore r5 is not treated as a textual/binary patch of the private baseline above. It is accepted only as its exact audited final binary and remains opt-in.

## Binary compatibility audit

The baseline and r5 WASMs were parsed directly before enabling the variant. The following module sections are byte-for-byte identical: type, import, table, global, export, start, element, data, data-count and the function-name custom section. Both expose the same 85 function imports, the same exports, 57 globals, and the same 89,368-entry function table. All 91,111 named functions are present with the same names and indices.

r5 adds one unnamed internal function and changes 34 existing function bodies. No named GLUE 5 hook changed index or signature. The structural fingerprint enforced by `crates/glue5-game/build/main.rs` is:

`5051fc3797190d24da58ab7d47301586ef62b98fbba48bd84fc2bbdb7ae974b7`

The fingerprint excludes function declarations/code and custom sections so the audited extra private function is allowed, but includes the module layout and initialized data used by the host/runtime contract. Exact whole-file SHA-256 validation still applies first.

## Interaction with GLUE 5 hooks

Seven r5-modified functions overlap names known by the host. Readback and script-asset hooks call their `orig::...` implementations, so the r5 bodies still execute. `audDecoderOpus::SubmitPacket` is exposed for direct host calls but is not replaced by the runtime hook table.

`CHighQualityScreenshot::StoreScreenShotInBuffer` is the one real override conflict: the baseline GLUE 5 workaround deliberately returns failure to avoid the old libjpeg crash. For `grandv-r5`, the hook now calls `orig::photo_to_jpeg`, allowing r5's repaired photo-buffer/JPEG path to run. Baseline behavior is unchanged.

## Changed-code census

The final r5 WASM changes 34 existing named function bodies and adds one unnamed private function. The changes cluster around the fixes advertised by the r5 package rather than broad engine churn: resource/cache lifetime, audio bank/Opus and vehicle-engine voice handling, screenshot/JPEG, minimap/streamed TXD/Scaleform/Bink paths, scenario collision, game timer and signed-in script wrappers.

That is why GLUE 5 keeps r5 as one exact audited runtime variant instead of trying to recreate every internal change as a Rust hook. Host-side patches are added only where GLUE 5 masks an r5 body or where the fix belongs to external data.

## Native coverage map

| GrandV r5 area | GLUE 5 status |
|---|---|
| Critical lighting pipeline waits | Ported: deferred/tiled/corona/distant-light draws wait selectively for a cold pipeline instead of being silently dropped; wait count/time are measured. |
| Voice pressure diagnostics | Ported read-only for the audited `8b0b5899ed` layout. |
| Native audio ring pressure | Ported: underruns/silence are joined by ring fill min/max, capacity and overrun observations. |
| Photo/JPEG buffer repair | Preserved for `grandv-r5`: the host calls the repaired original body; baseline keeps its safe-failure workaround. |
| Mission drawable/page-map repairs | Covered generically by GLUE 5 page-map repair logic. |
| `phInstGta` 16384 physics instances | Ported as an r5-only same-size `gameconfig.xml` overlay. It only applies to the known base `gameconfig.xml` SHA-256 `f4651a736d3651c42976c161a6b5f5a94ced41b89809f1746908bc1dcb1371f6`; custom/already-patched layouts are left untouched. |
| `env_test` sandbox | Already native. |
| Stale cache refresh/integrity | Already native through SHA-aware package validation, size/mtime checks and cache invalidation. |
| Resource-cache deletion race | Browser-specific implementation is not transplanted: the native cache already serializes copy/eviction/writes with locks. |
| Browser AudioWorklet startup recovery | Browser-specific; native SDL output handles underrun silence and now exposes ring/voice diagnostics. |
| DAT10/DAT54/DAT151 + custom AWC import | Partial infrastructure only; do not claim full parity yet. |
| YPT68/YBN43 classic add-on compatibility | Resource decoding infrastructure exists, but safe stock-target indexing/mount remains separate work. |
| Missing texture-parent removal | Not ported automatically; it remains an explicit compatibility concern rather than a global host mutation. |

## CI compile evidence

On GitHub Actions run `43447126338`, the manual Android Build workflow selected
`GAME_RUNTIME=grandv-r5`, fetched the private `game-grandv-r5.wasm` asset as build
`8b0b5899ed`, and `glue5-game` explicitly reported the experimental
`grandv-r5` runtime variant. The Android release profile then completed
successfully in 16m34s. Packaging failed only after Rust compilation because
the auxiliary `libadrenotools.so` staging path was wrong; that Android packaging
issue is independent of the game WASM ABI/code-generation compatibility.

Therefore r5 has passed both the structural audit and a full Android release
compile. It has **not** yet passed APK packaging/device gameplay validation.
The packaging follow-up now stages the three Android support libraries through
an explicit absolute directory instead of relying on Cargo's internal build
directory layout; GitHub Actions remains the acceptance test for that fix.

## Build and test policy

`grandv-r5` requires `GLUE5_ALLOW_EXPERIMENTAL_BUILD=1`. The manual GitHub Build workflow can select it from the private draft asset `game-grandv-r5.wasm`; ordinary CI and manual builds default to the canonical `game.wasm` baseline. Never commit either game binary or publish the draft release.

Device A/B should record the exact runtime identity from diagnostics together with frame/GPU telemetry, critical-lighting pipeline waits, audio voice pressure, and BC→ETC2 workload/fan-out. Promotion from experimental requires a successful Android build plus S21+ runtime testing; structural compatibility alone is not gameplay acceptance.
