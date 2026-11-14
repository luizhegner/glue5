# Unified original-resource extraction

The product obtains resources automatically from the configured WebAssembly
installation or GLUE server. Users should not prepare export folders or import
fonts, sound effects, artwork, or other resources separately. The server can
also use the owner's Legacy installation as an additional original source.

`sources::Sources` provides source-qualified reads and listings over existing
file-backed readers. It does not yet provide remote reads or automatic discovery.
Source registration does not imply format or engine compatibility.

Implementation order:

1. Implemented: range-backed stored nested RPF access, bounded to eight levels,
   including original sound-bank identity for REL binding. Compressed/encrypted
   nested entries and streaming large sound banks remain pending.
2. Legacy NG decoding with locally obtained key material, explicit format
   validation and no keys in logs, receipts, repository or CI artifacts.
3. DLC mount discovery from the installation's metadata, including update
   precedence. Preserve source, mount and version for every resolved resource.
4. Streaming audio extraction and REL sound graphs; integrate automatic menu
   SFX resolution through the same provider for local and server sources.
5. Loading artwork composition and fonts from original resources; extracting
   a flat texture alone does not reconstruct parallax layers or their motion.
6. Server resource endpoints and app bootstrap, with content hashes, resumable
   transfers, cancellation and atomic activation of complete resource sets.
7. Compatibility manifests for game restoration: distinguish missing assets,
   unsupported encodings and engine/script incompatibility. Mission or DLC
   presence in Legacy must not be reported as playable without validation.

Legacy is the preferred source; WebAssembly is the fallback. `Sources::resolve`
tries Legacy candidates first regardless of caller ordering, preserving mount
precedence within each source kind. A caller-supplied decoder/compatibility
check must succeed before selection. Failed reads or checks are included in
the result so fallback never hides unsupported Legacy content. Candidates can
use different original paths for the same logical resource. Automatic candidate
discovery and app/server integration are still pending.
Never mutate either installation during extraction. Keep generated resources
out of the repository. Compile and test on GitHub Actions, not the owner's PC.
