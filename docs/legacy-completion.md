# Complementing the WebAssembly game from GTA V Legacy

The target is the owner's Galaxy S21+ (G996B, Exynos 2100 / Mali-G78 MP14).
The source is the owner's GTA V Legacy installation; the destination is the
selected WebAssembly build or its GLUE server. Existing game files and saves
must remain intact until an independently validated replacement is explicitly
chosen. Do not include extracted game assets, executable data, or keys in Git.

## Evidence collected on 2026-11-13

- The installed Legacy executable's version information reports `1.0.3889.0`.
  The GLUE runtime is tied to the selected WebAssembly build `8b0b5899ed`;
  matching a filename is not enough to establish that their resources match.
- The Legacy `update/x64/dlcpacks` directory has 92 immediate subdirectories;
  the Web build's `data/x64/dlcPacks` has 11. Their immediate directory names
  did not match in this comparison. The two layouts differ: this is not a
  missing-DLC count, proof of availability, or a list of importable packages.
- Only 16-byte RPF headers were inspected. Legacy `common.rpf` and
  `update.rpf` have table mode `0x0fefffff` (NG); the Web build's
  `audio_rel.rpf` and `RESIDENT.rpf` have mode `0x0ffffff9` (AES). These are
  samples, not a classification of every file. Copying retail RPFs into the
  server as-is is not an established conversion or compatibility method.
- The owner reports game audio, including music, working. Keep that result;
  the prior investigation into optional GLUE menu effects is separate.

## Observing what the current game substitutes

`script_assets.rs` has intentional fallback paths for missing/unknown script
banks, textures, animations, clip sets, particles, models, text, cutscenes,
movies and a suppressed script. Some return success to scripts or substitute
an object so the game can continue. These workarounds can hide incompleteness.

The support report now includes a bounded, process-lifetime list of those
fallback observations. The list is limited to 128 distinct, sanitized game
identifiers and approximate occurrence counts; recording never waits on the
diagnostic mutex. It is displayed only when the owner requests the existing
diagnostics report, which can also contain local paths and must be reviewed
before sharing. An approved AI diagnostic session can also read the same
observations in its `context` response. Neither path records save contents or
streams game assets.

An observation is **a candidate**, not confirmation of a missing file: some
engine stores can be queried before registration, a mod can request a name,
and nonblocking recording can omit concurrent observations. A model may be
identified only by hash. The report tracks the current GLUE process, so it
does not represent the complete game or survive a process restart.

## Restoration workflow

1. Exercise a specific story mission or gameplay scene and collect its
   fallback observations. Compare with the engine log and actual visible
   failure before calling the resource missing.
2. Resolve each identifier to the correct archive, update layer and original
   path in both builds. Preserve provenance; do not treat a retail file with a
   similar name as the same resource.
3. Investigate version, resource format, encryption, load order and linked
   assets. Models, animations, maps, scripts, movies, text and audio have
   different requirements; newer assets can require engine or script code
   absent from this WebAssembly build. A format reader or valid header alone
   does not establish that an asset can run.
4. Prepare an isolated, reversible overlay or staging directory with explicit
   manifests and integrity checks. Never replace the live server installation
   during investigation. Validate the dependency set and fallback behavior on
   the S21+ for the chosen scene, including saves and frame stability.
5. Expand scene by scene. GTA Online updates and newer interiors require their
   own compatibility analysis; restoring all content is not achieved by a
   bulk copy, directory count, or a successful synthetic decoder test.

Current status: observations and diagnosis infrastructure only. No Legacy
assets have been imported by this change, and no mission, DLC, conversion or
on-device restoration is claimed complete.
