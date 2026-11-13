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

## Build and test policy

`grandv-r5` requires `GLUE5_ALLOW_EXPERIMENTAL_BUILD=1`. The manual GitHub Build workflow can select it from the private draft asset `game-grandv-r5.wasm`; ordinary CI and manual builds default to the canonical `game.wasm` baseline. Never commit either game binary or publish the draft release.

Device A/B should record the exact runtime identity from diagnostics together with frame/GPU telemetry, critical-lighting pipeline waits, audio voice pressure, and BC→ETC2 workload/fan-out. Promotion from experimental requires a successful Android build plus S21+ runtime testing; structural compatibility alone is not gameplay acceptance.
