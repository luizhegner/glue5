# GLUE 5 Resource API — v1

Native Rust library and JSON-lines command-line adapter for agents. No web
panel, Electron, interpreter, running game or GPU is required. Independent
Cargo workspace: it does not compile the game or the patched renderer.

## Source and safety contract

- Explicitly approve one source directory and one **existing, separate** export
  cache. Source access is read-only. Neither may contain the other.
- Nothing is scanned across the disk automatically. Listing is one directory
  or one RPF at a time, paginated; links escaping the source are refused.
- Supported: loose files and RPF7 OPEN/AES tables, directory trees, both name
  offset shifts used by this project, AES-encrypted binary entries, raw and
  deflated binary entries.
- AES key: optional `--aes-key FILE`, exactly 32 user-supplied bytes. No key
  recovery, bundled key data or external extractor invocation takes place.
- RSC7 envelopes can be read from loose files or RPF resource entries (their
  page flags must agree), inflated within the byte budget and inspected as
  separate system/graphics address spaces. Raw extraction preserves the envelope.
- Nested RPFs stored as uncompressed, unencrypted binary entries are read through
  bounded ranges, up to eight archive levels. Child OPEN/AES tables are supported.
  PC NG, compressed/encrypted nested containers and RSC7 ciphers/extended headers are deferred.
  Encrypted script entries are refused.
  Unsupported inputs return errors, never automatic conversion or source edits.
- AWC mono SFX PCM16 (codecs 0/1) and IMA ADPCM (codec 4) decode to WAV.
  Blocked IMA music with 0x18-byte channel headers also decodes to WAV, with
  explicit selection/order of one or two channels, never automatic stem mixing.
  Encrypted payloads and other block layouts are explicit errors, not guessed
  PCM. Input/output stay capped at
  32 MiB; long music needs the still-pending range/stream export provider.
- DAT54 REL resolves a direct SoundSet event to a SimpleSound's container
  hash, full waveform hash, 29-bit AWC stream ID and slot. It parses optional
  headers, validates bounds and rejects ambiguous records/events. It does not
  render wrappers, random/layered sound graphs or their pitch/effects/envelopes.
- GFX/SWF framing (GFX/CFX/FWS/CWS), bounded nested-tag inventory and embedded
  lossless format-5 XRGB/ARGB32 bitmaps to PNG are implemented. No ActionScript
  is executed. External Scaleform image resolution, fonts, vector rendering,
  JPEG/palette/RGB15 and LZMA remain unsupported.
- YTD legacy PC64 version-13 dictionaries can be inventoried and a selected
  ordinary 2D texture's base mip decoded to PNG: BGRA8/BGRX8/RGBA8, A8/L8,
  BC1/DXT1, BC2/DXT3 and BC3/DXT5. Gen9/console layouts, volume/cube textures,
  BC4/5/7, other formats and selecting lower mip levels are unsupported.
- Raw XML/resource **bytes are not decoded fonts or textures**. Soundset names such
  as SELECT are not waveform IDs; use `audio.resolve` with the installation's
  REL metadata, not a hash-name guess. Real-build bindings and automatic app
  import/reload remain pending. See [decoder integration](../../docs/resource-decoder-integration.md).
- For explicitly selected build `8b0b5899ed`, mono codec-12/48kHz waveforms
  can be repackaged from u8-length-prefixed 60ms Opus packets into Ogg Opus,
  without recompression or PCM allocation. This is container adaptation, not
  a new Opus codec. Encrypted/streamed AWC and other builds remain unsupported.
- Extracted data and receipts stay local. Never commit/export them to a PR,
  issue, public artifact, or agent prompt without reviewing proprietary content
  and personal information first. Tool errors can disclose local filenames.

## Protocol adapter (development interface, not a desktop product)

The product target is Android ARM64. CI publishes the reusable Android Rust
library, not Windows/Linux applications or an installable APK. The stdio
adapter remains a development interface; do not run it on the owner's PC.

For an explicitly approved development environment, create an export directory outside the game beforehand. For a Web build, use
its `data` directory as source. Example placeholders:

```powershell
'{"jsonrpc":"2.0","id":1,"method":"api.capabilities"}' |
  .\glue5-resource-api.exe --source 'D:\OwnedGame\data' --cache 'D:\GLUE5-Exports'
```

Keep stdin open to send several requests to the same process. Every request
and response is a single JSON line. Stdout contains only protocol responses;
startup errors/help go through the CLI. This is a JSON-RPC 2.0 **subset**, not
an MCP server: no notifications, batches, HTTP listener, authentication bypass,
arbitrary commands or automatic tool/URL execution.

## Protocol

| Method | Params | Result |
|---|---|---|
| `api.capabilities` | `{}` | version, methods, limits, supported/deferred formats |
| `resources.list` | `path`, optional `offset`, `limit` | entries, metadata, total, next_offset |
| `resources.inspect` | `uri` | decoded byte count, SHA-256, signature/format hint |
| `resources.extract` | `uri` | new local bundle and provenance receipt |
| `audio.list` | `uri`, optional `offset`, `limit` | AWC streams, tags, codec/rate/sample metadata |
| `audio.extract` | `uri`, `stream_id` (29-bit integer) | new bundle with `audio.wav` and receipt |
| `audio.resolve` | REL `uri`, `soundset`, `event` | direct waveform reference and source hash; no sound playback |
| `audio.extract_event` | REL `uri`, `bank_uri`, `soundset`, `event` | verified direct binding plus SFX WAV, receipt with both source hashes |
| `audio.extract_music` | AWC `uri`, `channels` (one/two distinct indices) | selected IMA channels as mono/stereo WAV and receipt |
| `audio.extract_web_opus` | AWC `uri`, `stream_id`, `build: "8b0b5899ed"` | packet-preserving mono `audio.ogg` with source receipt |
| `movie.list` | `uri`, optional `offset`, `limit` | movie metadata and bounded flattened tag inventory |
| `movie.extract_bitmap` | `uri`, `character_id` (16-bit integer) | new bundle with `image.png` and receipt |
| `rsc7.inspect` | `uri` | validated page sizes, flags, version, system/graphics hashes |
| `textures.list` | `uri`, optional `offset`, `limit` | legacy YTD texture indices, names, dimensions, formats and per-texture support/reason |
| `textures.extract` | `uri`, `texture_index` (nonnegative integer) | selected base mip as `image.png`, with source/selection receipt |

All paths are relative with `/` separators. Archive entries use
`archive.rpf::folder/entry.gfx`; nested entries use
`update.rpf::nested.rpf::folder/entry.gfx`. No globbing or implicit descent.
List an archive first to discover its actual entry names. RSC7 resource reads
preserve their on-disk envelope, while `rsc7.inspect` explicitly validates and
inflates it. Page-space access is exposed in Rust through `rsc7::Resource`;
the `textures.*` methods explicitly parse a supported YTD layout on top of
those pages. They never execute script resources.
Limits: 32 MiB decoded resource, 16 MiB archive table, 100,000 entries,
directory depth 32, 512 entries/page, 64 KiB protocol request. Large sound banks
must be listed and their individual binary entries requested, not copied whole.

```json
{"jsonrpc":"2.0","id":2,"method":"resources.list","params":{"path":"x64/data/cdimages/scaleform_minimap.rpf","limit":100}}
{"jsonrpc":"2.0","id":3,"method":"resources.inspect","params":{"uri":"common/data/ui/frontend.xml"}}
{"jsonrpc":"2.0","id":4,"method":"resources.extract","params":{"uri":"common/data/ui/frontend.xml"}}
```

Raw resource extraction exclusively creates `extract-…/resource.bin` and
`extract-…/receipt.json`. Prior results are never replaced. The receipt records
relative source URI, source-container size/mtime, decoded SHA-256/size, extractor
version and conversion boundary. It is **not** a whole-container content hash
or an authenticity claim. Failed I/O can leave an incomplete new bundle for
inspection; it is not reported as a successful extraction.

Audio extraction writes `audio.wav` instead, with source-AWC SHA-256 and the
selected waveform ID in the receipt. No resampling, silence trimming, fake
sound mapping or writes to the live app cache occur. A bank over the 32 MiB
input limit must await a range/stream provider; it is not loaded unboundedly.

The explicit Web Opus method writes `audio.ogg`. It verifies codec/rate, packet
bounds, mono TOC and 60ms duration, packet count versus declared samples, and
Ogg output limits. Packets are copied unchanged; the final granule trims the
last packet to the declared sample count. No additional preskip/gain is added.
Selection of build is required because AWC codec 12 has different meanings in
other releases. Full Opus payload validation belongs to the playback decoder.
This adapter does not yet assemble streamed stereo/multistem music banks.

`audio.list` also exposes music metadata in header channel order. Channel
indices in `audio.extract_music` refer to this order, not sorted stream IDs.
No implicit choice of the pause soundtrack or pair of stems is made. The
current IMA block decoder requires equal rate/sample counts in the selected
channels, zero skip and reserved fields, exact full blocks and exact sample
totals; incompatible files fail before an export bundle is created.

`audio.resolve` accepts DAT54 only. It resolves SoundSet -> SimpleSound, not
arbitrary child graphs. The resulting reference is not proof of audible parity:
the native engine may additionally apply pitch, gain, delay or processing.
`audio.extract_event` checks the selected bank's original container identity
(`RESIDENT.rpf::frontend.awc` or loose `RESIDENT/frontend.awc`) against the REL
container hash before decoding the referenced waveform. A different bank with
the same waveform ID is rejected. It records the REL URI/hash and the bank
URI/hash in the receipt. Renamed exports are not accepted as original banks.
Automatic bank discovery and real Web-build bindings are still needed.
REL format reference: [CodeWalker RelFile.cs](https://github.com/dexyfex/CodeWalker/blob/master/CodeWalker.Core/GameFiles/FileTypes/RelFile.cs).
Music block reference: [vgmstream blocked_awc.c](https://github.com/vgmstream/vgmstream/blob/master/src/layout/blocked_awc.c).

Movie extraction writes `image.png`, with the source-movie SHA-256, bitmap ID
and dimensions. It decodes the selected embedded bitmap, NOT the movie as a
screenshot, and cannot infer loading-screen parallax/depth from a flat image.
Movie decompression is capped at 32 MiB, 100,000 tags and sprite depth 16;
decoded image rows plus RGBA pixels must fit the same per-image byte budget.
ARGB is unpremultiplied for PNG; fully transparent pixels have zero RGB.
Zlib streams must finish exactly, with no unaccounted trailing payload.

Texture extraction addresses dictionary indices from `textures.list`, avoiding
ambiguity when names/hashes repeat. It preserves stored channels and alpha;
it does not infer premultiplication, gamma, parallax or movie composition.
Dictionary count/capacity is capped at 4096, names at 255 UTF-8 bytes, all
pointers are confined to their system/graphics domains, and RGBA plus PNG row
markers must fit the 32 MiB image budget. Metadata exposes unsupported entries
and reasons; attempting to export one returns an error before creating a bundle.
BC data must be tightly packed (legacy nominal or block-row pitch); uncompressed
rows may contain declared padding. Only the base mip's range is decoded and
validated, not every lower mip in the resource.

Example after discovering a real `.ytd` URI through archive listing:

```json
{"jsonrpc":"2.0","id":5,"method":"textures.list","params":{"uri":"textures.rpf::example.ytd","limit":100}}
{"jsonrpc":"2.0","id":6,"method":"textures.extract","params":{"uri":"textures.rpf::example.ytd","texture_index":0}}
```

This is a decoder capability, not proof that loading artwork in a particular
Web build uses this layout. Extraction from the owner's files, automatic import,
image/music activation in the app and native-resolution game UI remain pending.

## Rust interface

`Resources::open(source, cache, optional_key)` creates a source provider;
`list`, `read`, `inspect`, `extract`, `audio_list`, `audio_extract`,
`audio_resolve`, `audio_extract_event`, `audio_extract_music`, `audio_extract_web_opus`,
`movie_list`, `movie_extract_bitmap`, `rsc7_inspect`, `textures_list`,
`textures_extract` and `dispatch` are reusable public methods.
`Archive::open(File, optional_key)` exposes bounded RPF metadata and reads.
No SDL, JNI, engine globals or GPU are involved. A future stream provider must
fetch/validate required blocks before reusing this file-backed implementation;
a sparse Android cache is **not** an approved complete source yet.

## Validation

Rust compilation runs only on GitHub Actions for the owner's environment.
The `Resource API (Android)` workflow runs synthetic tests on an Ubuntu CI
host and cross-compiles the library for `aarch64-linux-android`. Ubuntu is the
remote build machine, not a supported game platform. No Windows distribution
is generated, and nothing is executed on the owner's PC. The reviewed lockfile is retained from the first
successful CI build; subsequent tests and builds use `--locked`.
No real keys or extracted game bytes are CI inputs.

Tests cover AES/OPEN, shift 0/1, deflate, malformed names/trees/ranges, absent or
incorrect keys, source boundaries, export isolation, receipt hashes and protocol.
YTD fixtures cover channel order, row padding, BC colors and both BC3 alpha
palettes, cropped edge blocks, name/pointer/count validation, unsupported
formats, pagination, protocol types, PNG output and source integrity.
See [the HUD handoff](../../docs/resource-api-hud-handoff.md) for what extracting
resources does and does not solve for independent HUD resolution.

