# UI assets

- `Manrope-Regular.ttf` (400): descriptions, values, and the rows' secondary text.
- `Manrope-SemiBold.ttf` (600): the rows, the tabs, the footer, the labels.
- `Manrope-ExtraBold.ttf` (800): `GLUE 5`, page titles, the description's title.
- `OFL.txt`: the fonts' SIL Open Font License.
- `Barlow-SemiBold.ttf`, `BarlowCondensed-Bold.ttf`: the previous type, kept while
  the last pages move to the shell's rows.
- `OFL.txt` covers both families.

Source: [Manrope](https://github.com/google/fonts/tree/main/ofl/manrope) and
[Barlow](https://github.com/google/fonts/tree/main/ofl/barlow),
[Barlow Condensed](https://github.com/google/fonts/tree/main/ofl/barlowcondensed).
The three Manrope files are static instances of the variable font, cut for the
weights the shell uses (OFL allows it).

The app embeds the fonts only. The start-page backdrop (the player's scenes,
with a code-drawn fallback) is drawn at run time; optional scene layers are read
from `glue5-scenes`, and the menus' sounds from `sfx`/`glue5-sfx` — neither is
embedded or redistributed. See [`docs/scenes.md`](../../../../docs/scenes.md) and
[`docs/menu-sfx.md`](../../../../docs/menu-sfx.md).

## Visual direction

Every menu is drawn in one shell, modeled on the GTA V pause menu's anatomy (a
header, a strip of tabs, a list of rows, a description, a footer of buttons) and
on Arena.ai's interface (1 px rules, dense rows, a solid light selection, one
accent). Flat panels, square corners, no floating windows. Manrope stands in for
Chalet London 1960, which is Rockstar's and not available under a free license —
this is an original GLUE 5 visual direction, not a reproduction of an official
Rockstar style guide. Sources and the design rationale are in
[`docs/ui-redesign-gta-v.md`](../../../../docs/ui-redesign-gta-v.md).
