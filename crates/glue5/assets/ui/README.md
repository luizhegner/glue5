# UI assets

- `Barlow-SemiBold.ttf`: interface text.
- `BarlowCondensed-Bold.ttf`: headings and the start-page mode labels.
- `OFL.txt`: the fonts' SIL Open Font License.

Sources: [Barlow](https://github.com/google/fonts/tree/main/ofl/barlow) and
[Barlow Condensed](https://github.com/google/fonts/tree/main/ofl/barlowcondensed).

The app embeds the fonts only. The start-page backdrop (deep night gradient,
fine lines and the orange accent) is drawn in code. Optional scene layers are
read from the player's `glue5-scenes` folder; they are not embedded or
redistributed. Without that folder the same page has a complete code-drawn
fallback. See [`docs/scenes.md`](../../../../docs/scenes.md) for placement and
format details.

## Visual direction

Home and utility pages share the same palette from the supplied Claude review:
blue-black surfaces, warm off-white text, and one restrained orange accent.
Barlow is used for body text and Barlow Condensed for display headings. This is
an original GLUE 5 visual direction, not a claim to reproduce an official
Rockstar style guide.

For historical context, the broad dark presentation was informed by Rockstar's
[GTAV settings support article](https://support.rockstargames.com/articles/1FKIriVNBHL72WZ0hrVT8u/adjusting-settings-within-gtav)
and [official PC screenshots](https://www.rockstargames.com/newswire/article/1748koo9o829a9/screens-from-grand-theft-auto-v-for-pc). No public official GTA V UI design guideline was located in the earlier research.
