# Start page scenes

The active menu now imports WebP layers from the selected WebAssembly build;
see [build UI assets](build-ui-assets.md). The separate PNG-folder loader below
is retained as historical documentation and is no longer the menu's source.

The menu now fills the output. Any scene loaded from the user's folder is
softened behind the entire menu; there is no sharp artwork strip at the side.
Without scene files, the backdrop is a flat neutral colour, without animated
arcs or gradients. The build identity sits at the bottom right, below the menu
actions and inside the display's safe area.

Automatic extraction of loading artwork from the user's game installation is
still pending. The existing PNG folder loader described below does not extract
game resources. No game artwork is bundled in the application.

The start page can show scenes: layers of PNG images with transparency that
drift slowly at different speeds. They take
turns on their own clock — every nine seconds, whatever the mode chosen on the
page (or `rotate.seconds` in `scenes.json`, and `rotate.order` for the order).
The folder names below are the ones the app looked for first; they are names, not
modes. Nothing is embedded and
nothing is downloaded: the scenes are pictures you put in a folder. The supplied
`glue5-scenes.zip` is kept outside the repository; extract its `glue5-scenes`
folder to one of the locations below. Without the folder, the page keeps its
drawn backdrop.

## Where

The app looks for a folder named `glue5-scenes`, in this order, and uses the
first that has a `scenes.json` or a folder named after a mode:

1. inside the game's folder;
2. beside the game's folder;
3. one folder above that;
4. the app's own `scenes` folder (beside the mods on Android; in the user
   directory elsewhere). The app makes this one, with a note in it, if none is
   found.

The game-files chip at the top of the start page shows the scene folder in
its tooltip (long-press on touch), or where to put one. The folders are looked for again when
the game's folder changes and after a game session; restarting the app also
does.

## What

```
glue5-scenes/
  scenes.json            (optional)
  story/     1-background.png  2-character.png
  online/    ...
  sandbox/   ...
  benchmark/ ...
```

Without `scenes.json`, each scene's folder holds its layers as PNGs, back to
front by file name. A scene whose folder is missing is skipped: the ones that
exist take turns alone, so a single scene simply stays. The first covers the screen (it is cropped to fit and
overscanned a little); each of the others is a figure with transparency,
standing at the foot of the screen on the side the text panel leaves free.
The softened copy of each layer is prepared off the UI thread and drawn behind
the full-screen dark translucent panel.
The blur is fixed; it is not animated during scene transitions. The original
and softened copies count towards the same 32 MiB per-scene budget.

PNG only (RGB or RGBA, 8 or 16 bits), up to 2048 pixels on a side and 16
million pixels per image. A layer file is limited to 32 MiB, a scene to six
layers and 32 MiB of decoded images. At most three scenes are held at once;
they are let go when the game starts. Paths are confined to the scene folder,
including when a layer is a symlink.

## scenes.json

Everything in it is optional.

```json
{
  "amp": [32, 9],
  "period": [16, 23],
  "scenes": {
    "story": {
      "panel_side": "right",
      "focus": [0.5, 0.45],
      "tint": [1.0, 0.95, 0.9],
      "layers": [
        { "file": "story/background.png", "depth": 0.2, "overscan": 0.06 },
        { "file": "story/hero.png", "depth": 1.0, "height": 0.97, "x": 0.22, "bottom": 1.04 }
      ]
    }
  }
}
```

- `amp`: how far the nearest layer drifts (points, across and down).
  `period`: how long a swing takes (seconds, across and down). By default the
  nearest layer swings 32 points across and 9 points down, with periods of 16
  and 23 seconds. Existing manifests can override these values.
- Wide figures reserve a little room at both sides of their swing, so the
  parallax stays visible without crossing into the menu or shrinking the art
  excessively on phones.
- `panel_side`: accepted for compatibility with older scene packs. The current
  menu stays on the left; figures are fitted into the space on its right, even
  when an older pack specifies a different position.
- `focus`: the point of a background kept when the screen is narrower.
- `tint`: a colour grade (each channel multiplies).
- A layer without `height` covers the screen; `overscan` is the margin it
  keeps to move within. With `height` (a fraction of the screen's), it is a
  figure: `x` is where its centre is (a fraction of the width), `bottom` where
  its foot is (a fraction of the height; over 1 hides the foot).
- `depth`: how much the layer moves (0 to 1.5): the farther, the less.
- File names are relative to the folder and may not leave it.

Settings > Game > Reduce motion keeps the layers and the lines still.
