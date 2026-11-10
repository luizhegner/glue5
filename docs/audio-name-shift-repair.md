# Audio name-offset repair — 2026-11-09

## Confirmed cause

The full PC-derived AES audio archives kept byte-based name offsets, but an
earlier conversion forced bit 28 of the names-length word at offset `0x08`.
Bits 28–30 encode a name-offset shift, not a mandatory Web compatibility flag.
The engine therefore looked for bank names at twice their correct offsets.

The inspected WASM implements `names_base + (entry.name_offset << shift)` in
`rage::fiPackfile::GetEntryName` (function 1907). `ReInit` (1882) obtains the
shift using `(names_word >> 28) & 7`. Original Web archives legitimately use
shift 1; these PC-derived tables require shift 0. Do not force either value
globally or copy it from an unrelated reference archive.

## Field repair and validation

- All 54 served full audio RPFs failed name validation at their declared shift;
  all resolved unambiguously with shift 0.
- Corrected only the four-byte header word; encrypted tables and audio payloads
  were not rewritten. All 54 then validated, with no further repairs proposed.
- The user retested in Server mode and reported that all audio categories they
  tested worked. This is not an exhaustive playthrough of every mission/track.
- Seven synthetic Python tests cover name resolution, both valid layouts,
  rejection by installers, payload preservation, restore and write-failure rollback.

## Safe tooling

`tools/repair_audio_name_shift.py DIRECTORY --aes-key KEYFILE` is read-only by
default. Add `--apply --backup NEW_MANIFEST.json` only with the game stopped.
Use `--restore --backup MANIFEST.json` to roll back the recorded changes.
The key is user-supplied; neither game files nor keys belong in this repository.
The tool requires PyCryptodome and accepts only flat AES audio archives with a
uniquely valid name layout. Raw NG archives are rejected, not converted.

Both audio installers now require `--aes-key` and validate actual name tables
before installing, rather than trusting the AES marker and bit 28 alone.
Retain updated modification times after a repair so streaming clients invalidate
the old archive cache. No app reinstall or save modification is required.

Local rollback record (not tracked):
`C:\GTA5\Backups\audio-name-shift-before-20261109.json`.
The untouched `audio-full-aes` backup still contains the old incorrect headers;
do not reinstall it without auditing/repairing first.
