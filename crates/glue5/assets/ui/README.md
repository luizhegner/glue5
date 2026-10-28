# UI assets

- `Barlow-SemiBold.ttf`: interface text.
- `BarlowCondensed-Bold.ttf`: headings and the home menu.
- `scenes/`: the four curated, layered defaults used by Home. The art was supplied from the user's GTA V collection; it is not relicensed by this project.
- `home.png`: an older generated background retained in the source tree for reference; the app no longer loads or embeds it.

Fonts are from the Barlow project, distributed under the SIL Open Font License
in `OFL.txt`. Sources: [Barlow](https://github.com/google/fonts/tree/main/ofl/barlow)
and [Barlow Condensed](https://github.com/google/fonts/tree/main/ofl/barlowcondensed).

## Curated Home scenes

The bundle keeps only a small, varied set from the supplied archive:

- **Story:** `ls5_background` with Michael's `ls5_foreground`.
- **Online:** `ls1_background` with the car from `ls1_foreground`.
- **Sandbox:** `ls8_background` with the ATV foreground.
- **Benchmark:** the unobstructed Los Santos skyline from `ls12_background`.

The remaining loading-screen cuts, beach composite, Rockstar logo, Chalet font,
and spinner are not bundled. Barlow stays as the interface font, and the app
keeps its own GLUE 5 identity. The scene art remains Rockstar material and is
not covered by this repository's license; do not redistribute these files or
ship them in a public build unless you have the necessary rights.

## Optional custom scenes

To replace individual modes, put a `backgrounds/` folder in Glue 5's SDL
per-user application-data directory, next to `glue5.json`, and add a
`scenes.json` manifest. It merges over the built-in scenes, so unspecified
modes keep their defaults. If the custom manifest or a custom scene is
invalid, the affected default scene remains visible.

Each scene is a stack of transparent PNG layers, ordered from farthest to
nearest. The default `fit` is `cover` for a full-screen background. A character
or other cutout can use `fit: "contain"`, with `scale` relative to screen
height and `x`/`y` as its center in viewport fractions. `depth` controls slow
parallax (0 to 1); `opacity` is optional. Filenames must be relative PNG paths
inside `backgrounds/`.

```json
{
  "scenes": {
    "story": { "layers": [
      { "file": "story-sky.png", "depth": 0.05 },
      { "file": "story-city.png", "depth": 0.35 },
      { "file": "story-character.png", "depth": 0.9, "fit": "contain", "scale": 0.85, "x": 0.82, "y": 0.54 }
    ] },
    "online": { "layers": [{ "file": "online.png", "depth": 0.4 }] },
    "sandbox": { "layers": [{ "file": "sandbox.png", "depth": 0.4 }] },
    "benchmark": { "layers": [{ "file": "benchmark.png", "depth": 0.4 }] }
  }
}
```

All four keys are optional. Scenes load on a worker thread, are reduced to a
maximum 2048-pixel edge before upload, and crossfade when selected. At most six
layers are accepted per scene; decoded textures are capped at 48 MiB per
scene (at most 96 MiB while crossfading), and each source file is limited to
32 MiB and 16 million pixels. Reduce Motion stops parallax, transitions, and
the stage animation.

## Visual direction: research notes

This is an original GLUE 5 theme, not a claim to reproduce an official Rockstar
style guide. The green accent and square geometry are the requested direction
for this app; they are not presented as the GTA V menu's documented global palette.

- [Fonts In Use: Grand Theft Auto V](https://fontsinuse.com/uses/55702/grand-theft-auto-v)
  identifies Chalet 1960 and Chalet Comprimé 1960 in the game's interface, with
  House Script in some elements. The community [gta-v-fonts repository](https://github.com/DrunkSonic/gta-v-fonts)
  describes extracted Scaleform fonts; those files are not bundled here.
- Rockstar's [GTAV settings support article](https://support.rockstargames.com/articles/1FKIriVNBHL72WZ0hrVT8u/adjusting-settings-within-gtav)
  documents the Pause Menu and Controls, Audio, Camera, and Display categories.
  Official [GTAV PC screenshots](https://www.rockstargames.com/newswire/article/1748koo9o829a9/screens-from-grand-theft-auto-v-for-pc)
  informed the broad dark presentation, not a pixel-for-pixel copy.
- The community [FiveM HUD color reference](https://fivemdocs.com/game-references/hud-colors)
  lists `#72CC72` among several HUD colors; this does not establish green as the
  GTA V main menu's universal accent.
- Rockstar's [PC controls and settings tips](https://www.rockstargames.com/newswire/article/51974aa3a724o2/rockstar-game-tips-tailoring-your-settings-and-controls-in)
  informed the emphasis on clear navigation and functional settings.

No public official GTA V UI design guideline was located in this research.

Bundled assets are embedded in the application and need no network access to
draw. Optional custom scenes are read locally from the user's data folder.
