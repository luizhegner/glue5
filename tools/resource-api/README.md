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
  offset shifts used by this project, raw and deflated binary entries.
- AES key: optional `--aes-key FILE`, exactly 32 user-supplied bytes. No key
  recovery, bundled key data or external extractor invocation takes place.
- PC NG, nested RPF, per-entry crypto and RSC7 decoding are explicitly deferred.
  Unsupported inputs return errors, never automatic conversion or source edits.
- AWC/GFX/XML/YTD **bytes are not the same as decoded sound, fonts or textures**.
  This API provides bounded access and provenance, not those future decoders.
- Extracted data and receipts stay local. Never commit/export them to a PR,
  issue, public artifact, or agent prompt without reviewing proprietary content
  and personal information first. Tool errors can disclose local filenames.

## Run (prebuilt native binary from Resource API workflow)

Create an export directory outside the game beforehand. For a Web build, use
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

All paths are relative with `/` separators. Archive entries use
`archive.rpf::folder/entry.gfx`. No globbing or implicit nested-archive descent.
List an archive first to discover its actual entry names. Metadata for RSC7
entries can be listed; extracting those resource entries is not yet implemented.
Limits: 32 MiB decoded resource, 16 MiB archive table, 100,000 entries,
directory depth 32, 512 entries/page, 64 KiB protocol request. Large sound banks
must be listed and their individual binary entries requested, not copied whole.

```json
{"jsonrpc":"2.0","id":2,"method":"resources.list","params":{"path":"x64/data/cdimages/scaleform_minimap.rpf","limit":100}}
{"jsonrpc":"2.0","id":3,"method":"resources.inspect","params":{"uri":"common/data/ui/frontend.xml"}}
{"jsonrpc":"2.0","id":4,"method":"resources.extract","params":{"uri":"common/data/ui/frontend.xml"}}
```

Each extraction exclusively creates `extract-…/resource.bin` and
`extract-…/receipt.json`. Prior results are never replaced. The receipt records
relative source URI, source-container size/mtime, decoded SHA-256/size, extractor
version and conversion boundary. It is **not** a whole-container content hash
or an authenticity claim. Failed I/O can leave an incomplete new bundle for
inspection; it is not reported as a successful extraction.

## Rust interface

`Resources::open(source, cache, optional_key)` creates a source provider;
`list`, `read`, `inspect`, `extract` and `dispatch` are reusable public methods.
`Archive::open(File, optional_key)` exposes bounded RPF metadata and reads.
No SDL, JNI, engine globals or GPU are involved. A future stream provider must
fetch/validate required blocks before reusing this file-backed implementation;
a sparse Android cache is **not** an approved complete source yet.

## Validation

Rust compilation runs only on GitHub Actions for the owner's environment.
The `Resource API` workflow tests synthetic fixtures on Linux and Windows and
uploads the standalone binary. Its lockfile is generated with the first build;
retain/review it before integration so subsequent builds can use `--locked`.
No real keys or extracted game bytes are CI inputs.

Tests cover AES/OPEN, shift 0/1, deflate, malformed names/trees/ranges, absent or
incorrect keys, source boundaries, export isolation, receipt hashes and protocol.
See [the HUD handoff](../../docs/resource-api-hud-handoff.md) for what extracting
resources does and does not solve for independent HUD resolution.
