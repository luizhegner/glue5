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
the required setup screen now explains missing resources instead of silently
opening a partially prepared panel. Update the companion or select the full
local WebAssembly build; there is no Skip button.
Both the app and server executable need the new implementation for remote
artwork import. Building the Android APK does not update the PC server.
The **GLUE5 file server** workflow tests and builds only the Windows companion
server on GitHub, with `glue5-serve.exe` as an artifact. It does not compile
the game for Windows or require building anything on the owner's computer.
Shared streaming integration tests run on a Linux host, as do the Android
project's other host tests; this does not produce a Linux game build. The
Windows companion is gated on those tests plus resource-validation tests on
Windows. Running the Windows **cache client** tests exposed existing access
denied failures; that client is outside the Android app/Windows server scope.

The server advertises SHA-256 hashes for UI resources. A client reuses only
matching cached content, including when the file size is unchanged. New files
are validated and written through a temporary file before installation.

Currently transported: `chalet.woff`, `spinner.png`, and the 38 WebP layers
for the beach scene and `ls0` through `ls16`. The app converts WOFF to sfnt
in memory and decodes WebP images on worker threads. Images stay outside Git.

## Display and memory

On first opening (including an upgrade from settings without source
confirmation), the player must explicitly connect to a GLUE server or choose
the WebAssembly build folder. Automatic folder discovery does not complete
this step. Cancellation, an invalid folder and a failed connection cannot
unlock PLAY. A successful selection is remembered without deleting files,
cache or saves.

Before any menu tabs appear, the streamed base must be ready, Chalet and the
game's spinner must decode, and the first valid loading-art scene must be
uploaded. The same gate applies on later starts/source changes, including
keyboard/controller shortcuts and the app's engine-start path. Only source
confirmation is persisted: cached files are reopened, not blindly trusted as
already decoded. Setup keeps source selection/reconnection available on
failure. It is not story-mode loading. A corrupt later scene is still skipped;
all scenes are not decoded into memory just to pass the initial gate.

This gate currently covers **implemented visual resources**. It does not
claim that the pending SFX/music importer has run or mark audio as complete.

The resource decoder library is now also present in this integration branch
under `tools/resource-api`, with its own locked workspace and synthetic tests.
RPF opening accepts `Read + Seek` sources so a future GLUE stream adapter can
read the directory and selected bank without opening a sparse cache file as
complete, or downloading an entire pack. Its CI checks the Android ARM64
library and synthetic Web Opus interoperability on a remote host; it does not
build a Windows/Linux app. App dependency wiring, automatic key resolution,
event-to-bank import, player reload and pause-music selection are not yet
published or validated. The presence of these decoders is not an audio-ready
flag. No game data/keys/exports are included in the library or its tests.

Artwork fills the actual output rectangle. There is no fixed-size viewport
or letterbox. Source positions are normalized; foreground proportions are
preserved and backgrounds cover the output. Menu controls retain the existing
safe-area handling; the artwork does not derive its bounds from that area.

The menu chrome is also edge-to-edge: Android gesture insets must not turn
the header/tab/footer strips into a centred floating rectangle. Horizontal
cutout protection applies to interactive content; top/bottom gesture margins
do not letterbox the menu. An actual keyboard is tracked separately and still
shortens the menu's available height. In-game touch safe areas and the 3D
presentation/aspect-ratio path are not changed by this shell adjustment.
Without decoded game artwork, PLAY uses the full list width rather than
reserving a blank half-screen for an image that is not available.

PLAY's four modes share one horizontal strip; the selected mode's help and
launch action sit below it. The build identity
and input hint share one footer line (the hint yields space to identity and
actions on narrow layouts). Gameplay keeps its separate bottom-right identity.
Phone scrollbars indicate position but do not accept touch dragging; swiping
the contents, wheel scrolling and keyboard/controller navigation remain.
Choice arrows are vector strokes: Chalet maps the Unicode guillemets used by
older builds to game-specific artwork, including the Rockstar CREATED mark.

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
