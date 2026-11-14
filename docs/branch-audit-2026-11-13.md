# Branch audit — 2026-11-13

Baseline: `origin/main` at `1789e1c`. Fetch all remote heads explicitly:
the checkout's configured fetch only retrieved its current branch, leaving
other remote-tracking references stale. Use
`git fetch origin '+refs/heads/*:refs/remotes/origin/*' --prune` before comparisons.

## Removed remote branches

All five tips were ancestors of main. Their commits remain in main and local
worktrees were not deleted or modified.

| Branch | Preserved tip |
| --- | --- |
| fix/audio-completeness | ea23981 |
| fix/mobile-ui-consistency | d58d986 |
| fix/mobile-ui-followup | 92f350e |
| integration/consolidate-android | 1789e1c |
| integration/s21-r5-mobile | c0be3c1 |

## Remaining remote branches

| Branch | Findings / action |
| --- | --- |
| main | Consolidated baseline; PRs 22, 24, 25 already merged |
| feat/unified-resource-extraction | PR 29; Legacy-first resolver, nested RPF ranges, audio bank identity; resource CI passed at cf742de |
| fix/unified-touch-haptics | PR 26; its commit 922a285 is also included in PR 27; do not implement twice |
| fix/modern-touch-phone-direct | PR 27; includes PR 26 and direct-phone/drag changes; Android touch tests fail |
| fix/loading-status-feedback | PR 28; loading progress and recovery UI; Android test still expects removed Behind button |
| feat/resource-api | Decoders already copied into main; main adds range-reader support and hash compatibility fix. Original branch still contains a resource-local .gitignore absent from main. Preserve until full-path audit is completed |
| feat/ai-game-control | Five unique commits: headless diagnostics/CLI/MCP; not interchangeable with Android diagnostic implementation |
| feat/gm45-d3d11-vertical-slice | Unique Windows implementation; outside current Android work, preserve |
| experimental/gm45 | Older unique GM45 work; compare with vertical-slice before removal |
| fix/radar-position | Only unique merge history remains; no unique non-merge commits reported; preserved pending cleanup |
| ci/build-diagnostics | Generated build output, not a feature branch |
| ci/diagnostics | Generated import-test output and temporary merge history; workflows may recreate it |

PR 26/27/28 layout checks passed, but Android import tests failed. Do not
interpret mergeable=true as validated. Latest reviewed failing runs:
43525070893, 43528261773, 43534305680. PR 27 failures include first-person
mapping, left-side manual-aim fire, and compact pause navigation.

## Local work not represented by remote tips

`git worktree list` also contains diagnostic, r5-physics, GM45 research and
UI-radio branches. Preserve these until their patches are compared explicitly.

Particularly relevant: `C:/k8r/glue5-unified-harvest` has uncommitted
`tools/resource-api/src/harvest.rs` and lib.rs dispatch changes. This implements
explicit GFX/YTD/AWC extraction plans, a content-addressed cache and a font-tag
index (not font conversion). Reuse/reconcile this work before introducing
another resource batching/cache layer. It has not been integrated or validated
by this audit.

Other dirty worktrees: `C:/zzn/projetos/glue5-btreemap-fix` (stream manifest
changes), `C:/k8r/glue5-gm45-fallback` (substantial unpublished work), and
`C:/k8r/glue5-gm45-implementation` (standalone validation dependencies).
No uncommitted work was reset, stashed, moved, or deleted.
