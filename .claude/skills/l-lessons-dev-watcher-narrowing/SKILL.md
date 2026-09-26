---
name: l-lessons-dev-watcher-narrowing
description: "Project lessons learned for the zfb dev-server watcher and tick narrowing (zfb-watcher ChangeKind coalescing, zfb-build orchestrator fan_out_safe gate, commands/dev.rs tick-candidate derivation, the PageSelection::All fallback, the dependency graph's Content edges). Read PROACTIVELY before planning or implementing work touching crates/zfb-watcher/, crates/zfb-build/src/orchestrator.rs, crates/zfb-build/src/policy.rs, or crates/zfb/src/commands/dev.rs — and before quarantining ANY dev-server flake. Contains traps, root causes, and \"watch for next time\" notes from previous attempts."
---

# Lessons — dev-server watcher & tick narrowing

> **Before quarantining ANY flake in this area, apply the three rules** (zudo-test-wisdom →
> Deflaking Recipe → "The three rules"):
>
> 1. **"Flaky" is not a diagnosis** — it is an admission you have not diagnosed it yet. Prove it
>    first: diff the product's INPUT on a failing run vs a passing run. Identical inputs → test
>    bug. **Different inputs → product bug, stop and fix the product.**
> 2. **"It's OK, it's just flaky" is never acceptable** — every accepted flake devalues red, until
>    a real bug that fails 30% of the time is indistinguishable from noise.
> 3. **If a test seems irreducibly probabilistic, you are asserting the wrong thing** — assert the
>    invariant, not a sampled point value.
>
> The entry below is the case that produced those rules: this repo's quarantine pipeline was one
> step away from burying a live macOS product bug behind an immaculate paper trail.

## 2026-07-13 — a "flaky test" was a real macOS product bug (issue #1581 / PR #1582)

### What we set out to do

Triage `e2e_out_of_root_edit_narrows_rerender_and_discovers_new_entry`, which failed
~2-3 of every 4 runs on macOS and passed deterministically on ubuntu CI. The issue
proposed two possibilities and demanded one be ruled in: (1) test-harness timing, or
(2) a genuine macOS product gap.

### Approach we tried first

The issue's own "Recommended remediation" — written by an agent correctly following
root CLAUDE.md's `flaky:` quarantine pipeline — said: tag the test
`#[ignore = "flaky: <url>"]`, add a `crates/CLAUDE.md` manifest row, add it
to `exam.yml`'s `quarantine-heavy` filterset, investigate later.

**Following that would have buried a real bug behind a paper trail that looked
responsible.** Quarantine suspends PRODUCT coverage, and the product was the thing
that was broken.

### Why it went wrong (root cause)

Two independent structural facts, neither visible from the test:

1. **macOS FSEvents nondeterministically varies the SHAPE of the input.** An in-place
   edit of an EXISTING content file is sometimes delivered as `ChangeKind::Created`
   instead of `Modified` (`zfb_watcher::merge_kind` keeps a pending `Created` over a
   coalesced `Modified`). The orchestrator's strict `modified_only_content` gate
   requires the tick be exclusively in-place `Modified` content edits, so a coalesced
   `Created` sets `fan_out_safe = false` → `compute_tick_narrowing` returns `Off` →
   FULL FAN-OUT: every route re-rendered and re-stamped on disk.
2. **The guard that was supposed to absorb (1) was DEAD CODE.** #1058 already added a
   `Created` → `Modified` normalization for exactly this FSEvents artifact — but keyed
   it on `graph.consumers_of(path)` being non-empty. **No collection entry has a
   `DepKind::Content` edge on a cold boot:** `seed_boot_module_edges` writes only
   `DepKind::Module` edges, and the dev server's ONLY `DepKind::Content` writer is
   `make_discovery_hook`, which fires only for newly-CREATED files. So the
   normalization could never fire for a pre-existing entry.

The bug is **perf-only** (bytes still end up correct), which is why it survived: no
test asserted narrowing *on disk* until this one did, under `ZFB_DEV_EAGER=1`.

It is **not** out-of-root-specific — in-root collections lose narrowing on macOS too.

### What worked instead

A session-live `KnownContentEntries` registry on `GranularityPolicy` (same
shared-interior-mutability shape as the existing `RawImportInvalidation`), seeded at
boot from the collection MEMBERSHIP walk and extended by discovery. The normalization
consults it; #1058's graph check stays as a secondary source (a warm persisted graph
CAN restore Content edges).

Scoped to the normalization ONLY. See the `PageSelection::All` trap below for why.

### Watch for next time

- **If a dev-server test fails intermittently on macOS but never on ubuntu, suspect the
  PRODUCT, not the test.** Run with `ZFB_DEV_TIMING=1` and diff the tick line between a
  failing and a passing run. `orchestrator.rs`'s own comment names the smoking gun: *"a
  `narrowing=false` line whose kinds include a `Created` for an already-known content
  file."* This takes minutes and is the first thing to do — before any quarantine.

      [zfb-timing] tick(): kinds=[alpha.mdx:Created] eager_hint=true fan_out_safe=false

- **If you are about to narrow `dirty_pages`, STOP.** An unknown content path trips the
  planner's `PageSelection::All` sentinel, and that over-broad fallback is currently the
  ONLY thing re-rendering AGGREGATE pages (a post index listing every entry, tag pages,
  pagination) on a content edit. Narrowing it without first deriving authoritative
  aggregate/tag/pagination provenance **silently under-renders them** — trading a perf
  bug for a correctness bug. There is NO aggregate-page regression test in this repo.
  Write one FIRST. Tracked in issue #1583.

- **`graph.add_node()` is not a shortcut for "make the graph know this path."** It makes
  `consumers_of` return `Some(vec![])` (known-but-unused). Check what the planner does
  with an empty consumer set before relying on it.

- **If you add a guard/normalization keyed on a registry, assert the registry is actually
  POPULATED in the scenario you are guarding.** #1058's guard was correct in shape and
  dead in practice for two releases. A guard that can never fire is worse than no guard:
  it reads as coverage.

- **The watcher can report a created DIRECTORY whose children never surface as individual
  events.** Never register the watcher's raw event paths as your source of truth for
  collection membership — re-walk. (Caught by codex review; would have reintroduced the
  full fan-out one directory-create later.)

- **A `Removed` must purge registry state BEFORE any `Created` normalization reads it.**
  The watcher can batch a removed directory and a `Created` beneath it into ONE tick, so
  purging afterwards is too late and a genuine delete→recreate would skip discovery.

- **`#[cfg(feature = "embed_v8")]` sitting between a doc comment and its `fn` is a
  footgun.** Insert a new function into that gap and the gate silently transfers to your
  function, leaving the original ungated. It still compiles with default features — only
  the no-v8 lane catches it.

- **`pnpm b4push` does NOT run the no-v8 lane** (it is a `B4PUSH_FULL=1` step). A green
  b4push is NOT sufficient before pushing anything that touches an `embed_v8` cfg
  boundary. Run `cargo check --no-default-features -p zfb --tests` by hand, or it costs a
  CI round-trip.

### Would-skip-if-redoing

- Reading the FSEvents/`notify` internals and theorising about directory-granularity
  event coalescing. The issue's hypothesis (candidate-set broadening) was wrong, and
  ~an hour went into reasoning about it from source. `ZFB_DEV_TIMING=1` — instrumentation
  that already existed, documented in a comment right next to the bug — answered it in
  one run. **Instrument before theorising.**
- The initial theory that the handshake's `__warmup-N.mdx` files were the whole story.
  They were a real second defect, but ablation proved they were not the primary cause.

## 2026-07 — a SECOND dynamic-watch registry: `css_mirror_roots` (epic #1799, issues #1801/#1802/#1805)

The watcher API surface grew a second dynamic-registration channel alongside the
file-parent `watch_additional_files`/`dynamic_dependency_paths()` pair this file
already covers: `Watcher::sync_recursive_dir_watches` (zfb-watcher, #1801) plus a
`css_mirror_roots` registry on `RawImportInvalidation`/`GranularityPolicy`, exposed via
`css_mirror_root_paths()` (zfb-build, #1802). Both channels are reconciled from the
SAME `orchestrator.rs` function, `register_dynamic_dependency_watches`, and both reuse
the SAME `watch-extra registered:` `ZFB_DEV_TIMING` signal — but they are watching for
structurally different things:

- `watch_additional_files` / `dynamic_dependency_paths()` — **non-recursive, file-parent**
  watches for out-of-root `?raw`/worker/plain-module import targets discovered by the
  browser pipeline (#1678/#1710/#1711).
- `sync_recursive_dir_watches` / `css_mirror_root_paths()` — **recursive-directory**
  watches for `zfb_build::SiblingMirrorPlan` mirror roots (tsconfig/plugin alias claims,
  computed by `build_default_css_payload_with_source_plan` on EVERY CSS-triggering tick,
  regardless of whether Tailwind is enabled or anything actually imports the alias
  target).

**Watch for next time:** a sibling directory can be claimed by BOTH channels
simultaneously (e.g. this repo's `dev_sibling_watch_1678_e2e.rs` fixture's `sub/shared`:
it is both a `?raw`/worker/plain-module import target AND carries a tsconfig alias
pointing at itself). If you write a confirm-e2e for ONE of the two channels reusing a
sibling directory already covered by the OTHER, the test proves nothing — it will keep
passing even with the channel under test fully reverted, because the other channel
already keeps the directory watched. `e2e_dev_sibling_tailwind_utility_class_refreshes_served_css`
(issue #1805) sidesteps this by using a fixture whose sibling is reached ONLY through
the tsconfig alias — no import touches it — verified by actually reverting
`sync_recursive_dir_watches`'s call site and confirming the test times out on
`wait_for_watch_extra` before restoring it. When adding a new dynamic-watch confirm-test,
check whether your chosen sibling path is already claimed by a channel you are not
trying to test.

## 2026-09 — one live SSR-dependency source; D4 retired (issue #3179 / #3171)

After #3162 the metafile-derived SSR dependency set lived in three places, all written by
`populate_module_edges` (`crates/zfb/src/commands/dev.rs`): the graph's `DepKind::Module` edges,
`RawImportInvalidation::ssr_module_deps`, and the #1284 D4 `out_of_root_watch_targets` extras.
D4 only grew, never filtered `node_modules` (a nested site's hoisted deps are "out of root"), and
registered its watches once at watcher construction — and in #3163's revert proof it MASKED the
registry for boot-imported nested-site deps, so the e2e could not tell whether the registry worked.

**Decision:** the registry is the single live source for the watch set and the SSR-reload
predicate; graph Module edges stay for page selection only; D4 is gone (`with_extra_watch_paths`
stays — config `extraWatchPaths`, CSS `@import` targets and out-of-root collections still feed it).
Deriving the watch set from graph edges was rejected: the reverse index has no edge kinds, a warm
persisted graph carries stale previous-session Module edges, and edges lack the registry's
`node_modules`/shadow filtering and alias expansion.

### Watch for next time

- **Retiring a backup channel re-opens the window it covered.** D4 was registered at watcher
  construction; the registry was registered only AFTER the boot hook, which runs the whole eager
  render. The fix calls `register_dynamic_dependency_watches` BEFORE the boot hook too (the eager
  bundle has already published into the registry by then). When you delete a redundant channel,
  list the time windows it was the only cover for.
- **Two overlapping channels make a revert proof lie.** If a confirm-e2e keeps passing with the
  channel under test reverted, look for another channel covering the same path (same lesson as the
  `css_mirror_roots` entry above) — here it was D4.
