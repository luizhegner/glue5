# Taxi touch controls

The taxi profile is selected only while the player is a passenger, the taxi's
display is drawn and the script has recently checked group 2 inputs 176, 177
or 179. Phone, pause, cutscene, alerts and loading exclude it. It hides the
generic passenger's weapon, aiming, seat-change and character-wheel controls.
Exit, radio, phone and camera controls remain available.

The original script's instructional icons are associated with their logical
inputs as `GET_CONTROL_INSTRUCTIONAL_BUTTON` returns them. This also covers
non-clickable keyboard slots that carry sentinel 361 instead of a usable input.
The bar's translated labels are preserved. Associations are bounded, expire
after 250 ms during construction and refuse conflicting input mappings.

Reference: [taxiservice.c](https://github.com/YimMenu/GTA-V-Decompiled-Scripts/blob/master/decompiled_scripts/taxiservice.c)
uses 179 for hurry, 177 for stop/resume and 176 for destination acceptance or
trip skip depending on the stage. This public script is a reference, not proof
that every stage is identical in the supported WebAssembly build. Live queries
are required. If a usable bar slot is missing, a fallback uses the checked
input, with neutral wording where its role is ambiguous.

Confirmation stays at the lower centre, stop/resume goes to its left and hurry
to its right. Logical inputs do not send weapon triggers. Context changes
release held actions, and disappearing prompts discard pending taxi inputs.

Tests cover query expiry, ambiguous icon mappings, direct input routing, the
centred confirmation and release on entry/exit in all three control profiles.
Run them through GitHub Actions. Real-device taxi detection, translated labels,
hurry, stop/resume and skip still require validation on the owner's S21+.
