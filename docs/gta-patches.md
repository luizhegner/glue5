# GTA V runtime patches: durable notes for future work

This document is the starting point for any future Arena chat or coding task
about patching GTA V behaviour from GLUE 5. It records facts, hard limits, open
questions, and the evidence still needed. Read it before proposing a patch.

The project diary is chronological and may contain superseded hypotheses. When
the diary and this document disagree, prefer the evidence and status recorded
here, then update both.

## Non-negotiable rules

1. **Do not put Rockstar game assets in this repository.** No extracted `.gfx`,
   `.ysc`, textures, audio, RPF files, decompiled scripts, or whole game-data
   trees. The user may supply files for local analysis outside Git. Record only
   file names, sizes, hashes when useful, format facts, offsets, and behaviour.
2. **Patch the user's own runtime copy, never the original game files.** The
   existing model is a generated copy under the app's scratch/overlay area. A
   patch must fail open to the untouched file if its preconditions do not match.
3. **Keep the game's scripts and mission state in charge.** Do not rebuild the
   phone or another game system natively unless the user explicitly changes
   that scope. A patch should alter a narrow boundary and let the original
   scripts continue to own state.
4. **Do not fight memory every frame when a load-time or call-boundary patch is
   possible.** The confirmed "phone keeps opening and closing" bug came from
   rewriting the phone position every frame while the game wrote its own. See
   "Phone position patch warning" below.
5. **A device report is evidence; CI is not device validation.** There is no
   Rust/Android toolchain in the usual Arena sandbox. Do not call a gameplay
   patch fixed until the user tests it on the Galaxy S21+ and says so.
6. **External decompiled scripts are hypotheses, not ground truth for this
   build.** The game data and scripts can be from different revisions. The
   user's files and Import reports from the actual build override them.

## Current patch mechanism

GLUE already changes some game files as they are read:

- `crates/glue5-runtime/src/overlay.rs`
  - `OVERLAYS` lists paths below the game `data/` directory.
  - `overlay::game_file(rel, real, scratch)` writes a changed copy under
    `scratch/overlay/` and has the engine read that copy.
  - It currently handles text/XML/control metadata only.
  - It enforces the **same byte length** as the source because the engine or
    its manifest may know the original size. Do not assume a longer or shorter
    binary patch is accepted until that loader path is proven.
  - Current examples are `gameconfig.xml`, `pausemenu.xml`, `frontend.xml`, and
    controller `.meta` files.
- `crates/glue5-runtime/src/vfs.rs` calls the overlay for eligible game files.
- A binary Scaleform patch is not yet implemented. Extending the overlay to a
  `.gfx` requires knowing whether this build reads that movie as a loose file,
  from an extracted directory, from an archive/image, or from memory after the
  archive is parsed.

Other useful intervention points already present:

- `crates/glue5-runtime/src/script_assets.rs`
  - `scaleform_request` hooks `REQUEST_SCALEFORM_MOVIE` and detects movies this
    data lacks.
  - `method_begin` hooks `CScaleformMgr::BeginMethodInternal`.
  - `param_int` hooks `CScaleformMgr::AddParamInt`.
  - `PHONE_MOVIE` is identified when the phone movie receives
    `SET_TITLEBAR_TIME`.
  - The phone home-screen workaround moves an unsupported app out of view 1.
- `crates/glue5-runtime/src/prompts.rs`
  - `scaleform_call_method` hooks
    `CScaleformMgr::CallMethodInternal(...)`, including GFxValue arguments.
- `crates/glue5-runtime/src/input.rs`
  - `scaleform_begin_method` and `scaleform_add_param_float` hook the other
    Scaleform method-building path.
  - They currently rewrite a browser `SET_INPUT_EVENT(1001.0)` to accept
    (`16.0`). Be careful: phone `SET_INPUT_EVENT` calls observed in scripts use
    integer parameters through a different path.
  - `control_update` hooks `CControl::Update()` and is the point where several
    runtime state patches are applied.
- `crates/glue5-game/hooks/*.rs`
  - Every hook must match an exact function name in the user's `game.wasm`.
  - A guessed hook name can fail the Android import/codegen build. Prefer
    existing hooks; add a new hook only after identifying the exact symbol and
    letting CI verify it.
- `crates/glue5-runtime/src/prompts.rs::hold_input`
  - Can pulse a `rage::InputType` for a minimum press. It is useful for a real
    game input, but repeatedly stepping a selection is not a direct touch.

## The user-supplied file tree received on 2026-10-31

The user supplied only the first level of a `playgta5.com` folder:

- It is the browser/launcher package, not a full expanded GTA V data listing.
- Important visible roots are `data/`, `_mods/`, `_movie/`, `glue5-scenes/`,
  `branding/`, and launcher JavaScript/CSS/HTML files.
- Inside `data/`, the supplied tree only shows `bootset.json`,
  `bootset_low.json`, `common/`, `manifest.json`, and `x64/`; it does not expand
  `common/` or `x64/`.
- It reported 25 directories and 83 files, so it cannot locate the phone movie
  or scripts. Do not infer from this tree that a file is absent.

### What to request next

Ask for a targeted listing rather than the whole tree. On the machine that has
the extracted data, search below `playgta5.com/data` (or the actual game-data
root if different) for:

```text
cellphone_*.gfx
cellphone_controller*
cellphone_flashhand*
scaleform_generic*
script_rel*
```

A useful Windows command from inside the data root is:

```bat
dir /s /b cellphone_*.gfx cellphone_controller* cellphone_flashhand* scaleform_generic* script_rel* > phone-files.txt
```

If nothing is found, the files are probably still inside an archive/RPF and a
filesystem tree will not show them. Ask which archive contains Scaleform movies
or scripts and request an export listing from OpenIV/CodeWalker instead.

The actual files wanted for local, out-of-Git analysis are:

- `cellphone_ifruit.gfx`
- `cellphone_badger.gfx`
- `cellphone_facade.gfx`
- `cellphone_prologue.gfx`
- `cellphone_controller.ysc` or its decompiled output, if available
- `cellphone_flashhand.ysc` or its decompiled output, if available

They should come from the same data/build the user runs in GLUE, not from a
newer public dump.

## Phone Scaleform facts established so far

The following comes from public decompiled `cellphone_controller` and
`cellphone_flashhand` scripts and must be rechecked against the user's build:

- One movie handle is used for the story phone. The script requests:
  - `cellphone_badger` for Franklin;
  - `cellphone_ifruit` for Michael and other normal story use;
  - `cellphone_facade` for Trevor;
  - `cellphone_prologue` in the prologue.
  A patch for only `cellphone_ifruit` is incomplete.
- The phone is selection-driven, not pointer-driven:
  - the script sends `SET_INPUT_EVENT(1)` for up, `(2)` for right, `(3)` for
    down, and `(4)` for left;
  - in keyboard-and-mouse mode, controls 180 and 181 (phone scroll forward and
    backward) walk the selection through the 3 x 3 home grid in sequence;
  - confirmation reads the movie with `GET_CURRENT_SELECTION`;
  - the script maps that returned slot to an app it populated with
    `SET_DATA_SLOT`, then starts the app script.
- No `GET_MOUSE_POSITION`, `SET_MOUSE_INPUT`, or cursor method was found in the
  phone script or in public `cellphone_ifruit` API lists. Other Scaleform
  movies (`CONTROLLER_TEST`, `ATM`, warehouse/browser-like screens) do have
  mouse methods; do not transfer those APIs to the phone without evidence.
- Therefore, writing only absolute mouse position or a synthetic relative mouse
  delta into the engine input block is **not established** as a phone-home tap
  solution. The earlier delta change remains mouse-like input for other paths,
  but must not be described as the confirmed phone fix.
- The user's desired interaction is direct: touching a visible app should make
  the game open that app at the moment of the tap. Visible selection stepping
  through intermediate apps is not the desired final design.
- Candidate future injection points, in order of preference:
  1. a load-time movie patch that can set or expose the current selection for
     the exact tapped slot, followed by the game's normal confirmation path;
  2. a call-boundary intervention around `DISPLAY_VIEW` or
     `GET_CURRENT_SELECTION`, if traces prove it synchronises both the movie
     and the script's saved selection;
  3. a proven engine/Scaleform method invocation for the phone movie.
  Selection stepping is only a fallback or diagnostic, not the goal.
- Current diagnosis in PR #12 adds an Import report section, "Recent phone
  movie methods", recording `SET_INPUT_EVENT(n)`, `DISPLAY_VIEW(view,
  selection)`, and `GET_CURRENT_SELECTION`, bounded to 64 lines. Use it to see
  what the actual build does around a tap before choosing an injection point.

## Phone position patch warning

The old 1.25x phone enlargement rewrote `CPhoneMgr`'s position from
`control_update` while the game rewrote its own swaying position every frame.
The renderer sampled either value and the phone appeared to open and close,
while the manager's state bytes stayed stable. The user confirmed that turning
**Bigger phone** off stops the cycle.

Durable lesson: a safe enlargement must happen at a single-writer boundary
(for example the script-native call that sets the phone position,
`SET_MOBILE_PHONE_POSITION`) or through a patched movie/script asset. Today's
per-function engine hooks do not automatically cover script natives.
`Settings::phone_enlarge` is therefore off by default and labelled experimental.

## General patch design checklist

Before writing a GTA patch:

1. Name the observable symptom and the exact user test that proves success.
2. Identify the owner of the state: engine code, a named script, a Scaleform
   movie, a data file, or GLUE's touch layer.
3. Find the narrowest boundary: file read, asset request, movie method,
   script-native return, control update, or input value.
4. Log first if ownership or timing is uncertain. Bound every trace and keep
   personal content, typed text, save contents, and precise finger coordinates
   out of reports.
5. Check every variant: character, mission/prologue, story/online, view/app,
   and data revision. A phone patch must cover all four movies above.
6. Preserve size and encoding unless the loader is proven to accept a change.
7. Make the patch conditional on a signature/hash/size and fail open.
8. Avoid per-frame writes that race the game. If a value must be maintained,
   find the game's own write or call and patch there once.
9. Add unit tests for pure transformations. Runtime memory patches need a
   device protocol in the PR.
10. State clearly what is proven, what is a hypothesis, and what still needs the
    user's device report.

## Evidence protocol for a phone test

Use Bigger phone **off** and Phone touchscreen input **on** unless the test is
specifically about Bigger phone. Ask the user to:

1. open the phone;
2. swipe once on the phone screen;
3. tap an app other than the pre-selected one;
4. press Back;
5. copy the Import report immediately.

Read, in order:

- touch trace: what GLUE emitted (but remember directional steps there can be
  swipes on the phone screen, not touches on the side D-pad keys);
- phone/input-device trace: `CPhoneMgr` bytes, published phone state, device,
  and `pos_z`;
- phone movie methods: the script's actual `SET_INPUT_EVENT`, `DISPLAY_VIEW`,
  and `GET_CURRENT_SELECTION` sequence.

Do not interpret a directional pulse as use of the layout's phone keys without
asking; the touch layer converts a swipe on the phone into directional steps.
