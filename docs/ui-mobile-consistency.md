# Mobile UI consistency — November 2026

Scope: GLUE 5's frontend and its settings/social/server/mod overlays. This is
not a rewrite of GTA V's own Scaleform menus, touch gameplay or renderer.
Audio diagnosis and the desktop Assets Manager remain separate work.

## Layout rules

- Flat dark surfaces, square controls, white selection with dark text,
  lime focus/adjustment accents. No added gradients, shaders or animations.
- At least 44-point standard touch targets. Keyboard opening reduces the
  content viewport, not the size of every control and font.
- Six primary destinations remain visible: a single strip in landscape,
  two rows on narrow screens. During a session, Play becomes Resume.
- Settings uses one full-size category picker instead of nine tiny buttons.
- Help is inline and wrapped, not dependent on hover. Narrow rows stack
  labels and controls. Content scrolls between the navigation and footer.
- One explicit footer Back/Resume action. Tapping the scene does not discard
  the open page. Input hints follow touch, keyboard or controller use.
- Confirmation text scrolls independently; its actions stay visible above
  the keyboard. The safe initial choice is Back; hover does not confirm.

## Behavior corrections

- Play has a visible primary action and four compact mode choices. Launch,
  mode and source changes are gated during preparation. No second launch
  is offered while a session runs.
- The second mode selector in Settings was removed: its choice used to be
  overwritten by Play's stored selection. Settings now links to Play.
- Clear-copy controls are disabled while the game is running or no streamed
  files are cached. Unsupported resource imports are visibly disabled.
- Download-all refers to files actually offered by the server, not a promise
  of complete GTA V audio/content. A disconnected partial cache is identified
  as partial. Network-sharing instructions point to Files, not a missing Game
  category. The duplicate sharing switch was removed.
- Secondary tabs, action groups, friend-code and server-address forms reflow
  on narrow screens; settings help no longer overlaps the following row.

## Verification and integration gate

### Follow-up: source-aware Play and narrow option groups

Second iteration:

- Session menus expose Exit game beside Resume on mobile too. Exit confirms
  that it closes GLUE 5 and may lose unsaved progress; it does not promise a
  return to the launcher or create a save automatically.
- Settings replaces its category dropdown with an independently scrolling
  sidebar on wide panels and an All settings index on narrow panels.
- Touch fire settings say Aim + fire / Separate aim rather than Auto / Manual;
  stored settings remain compatible. Aim-toggle gestures are explained.
- Held actions re-resolve game mappings; combined fire sets either analog
  trigger required by the aim binding. Device validation remains necessary.
- Touch actions keep circular Fire/Aim/wheels, bar-like pedals, and softly
  squared secondary actions. Existing hit targets and positions are preserved.

- Play's Game files action now opens Servers when Server is selected, instead
  of unexpectedly opening the local folder picker. Folder mode retains its picker.
- Missing-file guidance follows the selected source and surfaces the actual
  preparation problem when available; local guidance identifies WebAssembly files.
- Segmented options choose their column count from available width, including
  a single-column layout rather than squeezing five options into narrow cells.
- Added regression cases for both file-action routes and narrow option groups.
  These changes require the Actions checks; no Rust compilation runs locally.

Use `.github/workflows/ui-tests.yml` for lightweight component regression
tests and screenshots; see `tools/ui-check/README.md` for coverage limits.
Rust runs on GitHub Actions, not on the owner's Inspiron.

Before integrating, the owner still needs to build Android from the reviewed
branch and check on the Galaxy S21+ SM-G996B:

1. Launch with files ready, missing and preparing; switch all four modes.
2. Open every Settings category and reach its last row by scrolling.
3. Open Friends/Servers fields with the keyboard and paste/type text.
4. Open Settings during a game and use Resume without starting a new session.
5. Check confirmation cancellation and disabled cache deletion during gameplay.
6. Check cutouts, larger system font/DPI settings and controller focus if used.

No automatic merge to main, Android build, installation or release is part
of the UI component workflow. Existing saves and gameplay assets are untouched.
