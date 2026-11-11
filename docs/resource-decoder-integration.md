# Resource decoders and Android import integration

Tracking: GLU-21 (decoders/import), GLU-20 (menu SFX), GLU-24 (fonts),
GLU-25 (all game UI independent of scene resolution).

## Current boundary

`tools/resource-api` is reusable native code, not a web service or a new
Asset Manager. The Android ARM64 library is compiled remotely. The app does
not yet depend on it. Synthetic CI is not validation with the owner's banks.

Implemented audio boundary: bounded AWC ADAT/TADA inventory, optional tag-start
table, 29-bit stream IDs, chunk bounds, mono SFX PCM16 codecs 0/1 and IMA codec 4
to WAV. IMA uses 2048-byte blocks, inverted little-endian state headers,
low-nibble first and no extra predictor sample. Invalid indices/truncated data
are refused. WAV samples retain source rate/amplitude; no guessed trimming.

Blocked IMA music now has metadata/channel order and explicit mono/stereo
selection, decoded through the 0x18-byte per-channel block layout. Sources and
PCM output remain bounded at 32 MiB. Other codecs/layouts, nonzero skip,
unequal selected channel lengths and inconsistent block/sample totals are
refused, not silently truncated. This is not yet a streaming long-track decoder.

DAT54 REL resolves direct SoundSet -> SimpleSound references with proper
optional-header sizing and AA sentinels. Event hashes are kept distinct from
the child sound, container hash and 29-bit waveform ID. Duplicate/overlapping
records and duplicate events fail. Complex sound graphs are explicitly not
rendered; a direct reference still does not include native pitch/effects.

Implemented movie boundary: GFX/CFX/FWS/CWS framing and bounded zlib expansion,
paginated tag inventory including nested sprites, and embedded lossless
format-5 XRGB/ARGB32 to straight-alpha PNG. No ActionScript execution, movie
rendering or fabricated parallax layers. JPEG/palette/RGB15/LZMA and external
Scaleform image resolution remains unsupported. Synthetic fixtures validate PNG chunks,
CRCs, transparency, truncation and resource limits, not real loading imagery.

Implemented resource boundary: strict RSC7 raw-deflate envelopes, validated
system/graphics page lengths and confined virtual-address slices. RPF resource
reads preserve the envelope and verify its flags against the archive table.
The separate YTD reader now interprets legacy PC64 version-13 dictionaries:
bounded names, hash/pointer arrays and per-entry decode availability. It exports
ordinary 2D base mips from BGRA8/BGRX8/RGBA8, A8/L8 and BC1/2/3 to PNG. Unknown
formats, pitches, flags, dimensions or page ranges are reported/refused. No
Gen9, console layout guessing, volumes/cubemaps or lower-mip selection. Resource
ciphers, extended headers and encrypted scripts are unsupported.

Not implemented: AWC payload encryption (different from RPF AES), other blocked
music layouts/codecs, long-music range/stream export, Web custom Opus codec,
REL graph rendering/real-build bindings, unsupported YTD
layouts/formats, GFX font conversion, Android setup/stream provider and live cache
activation. Keep unsupported formats visible, never replace them with silence
and report import success.

## Menu effects are part of import, not a manual setup requirement

Existing `crates/glue5/src/sfx.rs` already has 12 menu events, per-event gains,
rate limiting and a separate SDL mixer. Its current file lookup/startup-only
load is not an importer. Keep manual overrides, but the intended normal flow is:

1. Resolve the selected installation (folder or server), then obtain verified
   complete source ranges through the provider. Sparse cache file size alone
   does not prove data is present.
2. Resolve soundset/event metadata to actual bank + 29-bit waveform ID. The
   script name SELECT is NOT automatically the waveform's JOAAT hash. Bindings
   must be verified for the selected Web build; do not pick a random short clip.
3. Decode selected clips off the UI thread into a fresh private staging bundle.
   Include bank hash, entry URI, waveform ID, decoder version and output hash.
4. Validate the required clips, then activate a complete cache generation
   atomically. Cancellation/error must keep the prior working generation and
   leave manual override files untouched. Do not overwrite sfx/*.wav piecemeal.
5. Notify/reload the SFX player on the audio-owning thread without restarting
   GLUE5; show which resources are ready or unavailable. User volume/mute choices
   still apply. First-launch silence before assets are ready is expected.

Verified event *names* are in `docs/menu-sfx.md`: NAV_UP_DOWN, SELECT, BACK,
NAV_LEFT_RIGHT. Refusal/alert sounds require their own verified bindings;
do not call all twelve mapped just because these four names are known.

## Home imagery, parallax and music

Decode original loading images/layers from the selected installation into the
existing scene-loader contract (bounded PNG layers + manifest with depth).
Retain layer composition rather than turning a flat image into invented layers.
Extract/assemble the actual pause-menu music through the audio provider; an AWC
metadata listing or mono-SFX decoder does not yet handle streamed music.

Use a preparation/loading state when necessary: discovery, transfer, decoding,
validation, ready, partial/unsupported, error and cancellation. Work must not
block first paint or monopolize the S21+'s memory; process one resource at a
time, cache by source/decoder hash, and allow the deliberate code-drawn fallback.

## All game UI stays in scope

The resolution goal is ALL original game UI, including subtitles, hints,
mission text, full maps, menus, phone, HUD, radar, health and wanted indicators.
These examples are not a limited whitelist. Extraction supports the work, but
does not itself separate 3D/UI rendering. Preserve the engine's UI semantics;
native-target rendering needs the separate GPU work and Mali-G78 measurements
described in `resource-api-hud-handoff.md`.

No keys, original audio, textures, fonts, saves or extracted bundles belong
in Git, Linear or CI artifacts. CI uses generated synthetic fixtures only.

YTD layout references: [CodeWalker Texture.cs](https://github.com/dexyfex/CodeWalker/blob/master/CodeWalker.Core/GameFiles/Resources/Texture.cs)
and [YtdFile.cs](https://github.com/dexyfex/CodeWalker/blob/master/CodeWalker.Core/GameFiles/FileTypes/YtdFile.cs).
Block compression follows [Microsoft's Direct3D specification](https://learn.microsoft.com/en-us/windows/win32/direct3d10/d3d10-graphics-programming-guide-resources-block-compression).
These describe the implemented formats; they do not replace validation with the
owner's Web build or prove native-resolution HUD rendering.
