# Interface resources from the WebAssembly build

The selected game build is the source for the native menu's game artwork,
Chalet font and loading spinner. Separate extraction folders are not required.
No game resource is embedded by this import path.

## Local and server sources

Local mode reads `b/<GAME_BUILD>/title/` inside the selected game folder.
Server mode transfers known optional resources from that same directory into
the client's build directory before declaring the streamed copy ready.

The optional resource blobs do not change the engine catalog, file IDs or
shader packs. Older clients ignore them. Older servers do not offer them;
the app remains usable with its ordinary font and code-drawn backdrop/spinner.
Both the app and server executable need the new implementation for remote
artwork import. Building the Android APK does not update the PC server.
The **GLUE5 file server** workflow tests and builds only the Windows companion
server on GitHub, with `glue5-serve.exe` as an artifact. It does not compile
the game for Windows or require building anything on the owner's computer.

The server advertises SHA-256 hashes for UI resources. A client reuses only
matching cached content, including when the file size is unchanged. New files
are validated and written through a temporary file before installation.

Currently transported: `chalet.woff`, `spinner.png`, and the 38 WebP layers
for the beach scene and `ls0` through `ls16`. The app converts WOFF to sfnt
in memory and decodes WebP images on worker threads. Images stay outside Git.

## Display and memory

Artwork fills the actual output rectangle. There is no fixed-size viewport
or letterbox. Source positions are normalized; foreground proportions are
preserved and backgrounds cover the output. Menu controls retain the existing
safe-area handling; the artwork does not derive its bounds from that area.

The layer ordering, perspective and tweens follow the build's title-screen
definitions, adapted to the full output. Scene changes take 0.85 seconds,
rotation takes 14 seconds, and layer movement uses a 42-second tween. Reduced
motion displays the first available scene without moving it.

Each WebP is limited to 2 MiB compressed, 2048 pixels per side, and 8 MiB
decoded. A scene has a 16 MiB decoded budget. Only the displayed, departing
and next scenes are retained. Artwork textures and pending decode results are
discarded when the game starts. A malformed or missing scene is skipped.

## Current limits

The regular Chalet is the font available directly in `title/`. Importing a
condensed face requires decoding the relevant game archive; a separate TTF
extraction is not used as an automatic substitute.

UI sound effects and pause-menu music still require archive decoding and
verified sound-to-event mapping. The legacy audio path has not yet been
replaced by this importer. Do not treat visual-resource import as complete
game-audio import.

Validation belongs in GitHub Actions and on the owner's Galaxy S21+; no local
Rust or Android builds are required on the Inspiron.
