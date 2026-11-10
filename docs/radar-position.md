# Original radar positioning experiment

Display settings offers Original (default) / Top left (experimental).
Choose before starting the game; changing an existing session requires closing
and reopening GLUE 5. The engine supports one game per process.

The option patches a scratch copy of `common/data/ui/frontend.xml` through the
existing runtime overlay. Original server files, saves, audio and WASM remain
untouched. Turning it off restores the original data path on the next launch.
The existing notification-feed width correction is retained.

Six original components move together: minimap, mask, blur, and their expanded
bigmap equivalents. Their sizes and X offsets stay unchanged; the pause-menu
map, gallery, loading/feed movie and unrelated HUD components are not modified.
Health/armour inside the original movie are not separately reimplemented.

Evidence: the served WASM and inspected copy have SHA-256
`11ca8d2c04c5e843d18ff4aea4899d72c86973c6b031df334e67c446b2ae83e0`.
Its function 35029 (`CHudTools::CalculateHudPosition`) resolves bottom alignment
as safe-bottom minus component height plus Y offset, and top alignment as
safe-top plus Y offset. Merely changing B to T would misalign rectangles with
different heights. The patch translates each normal/expanded group together,
placing its main rectangle 0.04 normalized units below the upper safe edge.
Original numeric field precision and total byte length are retained; rounding
error is at most half the smallest field unit. Unknown/missing/duplicate layouts
are rejected as a unit, leaving the original radar and feed-width fix in place.

Synthetic tests cover relative geometry, unchanged sizes/length and refusal
of unsupported layouts. GitHub Actions runs Rust; no local Rust build is used.

Device validation is still required before claiming completion: normal and
expanded radar, GPS driving, masks/blur, health/armour, interiors, HUD safe-zone
and aspect ratios, mission instructions and notification-feed interactions.
Existing game help can occupy the top-left corner; this experiment does not
silently move all the other HUD elements or claim that overlap is solved.
