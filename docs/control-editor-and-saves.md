# Touch editor and save library

The Controls settings open a separate editor for actual gameplay controls.
It is not the diagnostic click-region editor. Choose the current context,
drag a control, adjust its size, and Save or Cancel. Reset affects only that
context and preset. The editor uses a scaled view of the actual Touch draw
geometry; hit testing and occupied regions use the same resolved overrides
in gameplay. Positions are normalized to the existing safe control area;
the 3D viewport is not resized or replaced.

Overrides are versioned in `glue5.json`. Invalid/unknown entries are ignored
without resetting the other settings. Rendering uses the same icons/shapes
as gameplay. No gameplay input is injected by the editor. Customized controls
that overlap are outlined and cannot be saved until separated. Existing
defaults are not treated as user-created collisions. Preset XML import/export
and editing GTA's own menus/camera gesture zones are not implemented here.

## SAVES

The SAVES tab centralizes Android profile import (the existing importer and
its backup rules), refresh, and selecting a primary `SGTA5NNNN` file to start
Story mode. Backup files are not offered as primary saves. Inventory is
bounded and contains names, sizes and dates, not guessed mission progress.
Import/loading is disabled while a game is running.

Explicit selection creates a fresh writable snapshot under `save-sessions/`.
The selected bytes are copied to the new profile's sole startup slot,
`Profiles/selected/SGTA50000`; the original profile and slot are not replaced.
For that runtime only, VFS maps GTA's Default/Profiles paths to the snapshot.
Game settings outside those profile directories keep their original mapping.
New save progress remains in this session and appears in SAVES afterwards.
Normal Play keeps the previous, unredirected profile behavior.

Tests must separately verify filename validation, snapshot independence,
VFS read/write redirection, layout persistence and actual control hit tests.
These synthetic tests do not prove GTA accepted/autoloaded the selected save:
that and editor touch ergonomics require the owner's S21+ build validation.
Do not claim an Android build or device test until its result is observed.
