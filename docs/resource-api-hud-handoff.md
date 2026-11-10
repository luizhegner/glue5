# Resource API and native-resolution HUD: boundaries for the next agent

Related tracking: GLU-21 (resource access), GLU-24 (font conversion), GLU-25
(HUD render resolution). Implementation API: `tools/resource-api/README.md`.

## Resource workflow

1. Approve the user's Web build `data` as source and a separate local cache.
2. Query `api.capabilities`; do not assume PC NG or resource decoders exist.
3. Inspect `common/data/ui/frontend.xml` for HUD placements and minimap groups.
4. List relevant archives under `x64/data/cdimages`: `scaleform_minimap.rpf`,
   `scaleform_generic.rpf`, `scaleform_frontend.rpf`, `scaleform_platform_pc.rpf`.
   Follow pagination; record archive/entry URIs from actual results rather than
   guessing internal names. Look for HUD/minimap/wanted and font-library movies.
5. Inspect/extract only selected entries. Retain their receipts with the local
   research cache; commit code and synthetic tests, never the extracted bytes.

Library providers can later cover PC NG, nested archives, streamed reads,
RSC7 textures, AWC decoding and Scaleform metadata/font conversion without
changing every agent's scripts. Do not call these planned features implemented.

## The separate renderer problem (current evidence)

- `crates/glue5-runtime/src/upscale.rs`: the hook after
  `PostFX::ProcessNonDepthFX` emits `gpu::op::SCENE_DONE`.
- `crates/glue5-runtime/src/gpu.rs`: documents that the scene is finished in
  the back buffer at that marker, with HUD following.
- `crates/glue5-gpu/src/census.rs`: records HUD targets, samples, depth/stencil,
  position-dependent shaders and back-buffer reads. It explicitly warns that
  scene and HUD currently share `window × render_scale` resolution.
- `crates/glue5-gpu/src/present.rs`, `COMPOSITE_WGSL`: temporal presentation
  adds the scaled difference between the complete frame and pre-HUD scene onto
  the upscaled scene. **This does not rasterize the HUD at native resolution.**
- `crates/glue5-gpu/src/draw.rs`: HUD masks use specialized shader/vertex paths;
  these cannot be treated as ordinary screen-space sprites without checking.

Extracting a sharper texture or redrawing a guessed wanted-star icon is not a
general fix. The useful implementation path is to preserve the engine's HUD
semantics while directing compatible HUD draws to a native-size target and
compositing after scene upscale. Needs correct viewport/scissor transforms,
matching attachment dimensions, read-back/blur handling and pixel-coordinate
shader constants. Minimap terrain render targets may themselves be low-res:
moving only the final movie does not increase their detail.

## Validation gate for that future change

Keep original rendering as fallback. Compare low scene scale with native HUD
across normal/expanded minimap, GPS, wanted stars, text, health/armour, pause
menus, translucent overlays, cutscenes, safe zones and aspect ratios. Measure
GPU time, bandwidth and memory on Mali-G78; no claim of “free” native HUD or
FPS improvement without measurements. Do not enable unsafe redirect paths
globally based only on extracted assets or a passing CPU test.

This delivery changes resource tooling only, not GPU passes, the game build,
server data, renderer defaults or Android settings.
