# #1284 — dev dependency-invalidation diagnosis (epic #1285, Wave-1 #1286)

Reproduces and pins the root cause of the three symptoms in `zfb dev`, locks the
infrastructure decisions the two Wave-2 fix agents (#1287 routes, #1288 css)
depend on, and ships one failing reproduction test per symptom.

All findings below were **empirically confirmed** (not just code-read) via the
Level-1 reproduction tests added in this PR (run with `--no-default-features` to
skip the V8 first-compile):

```sh
cargo test -p zfb-graph                       --test dev_dep_invalidation_1284
cargo test -p zfb-build --no-default-features --test dev_dep_invalidation_1284
cargo test -p zfb-css                         --test dev_dep_invalidation_1284
```

---

## Symptom → exact failing code path

### Symptom A — component edit does not re-render the consuming route

Editing `src/components/**` (and, imprecisely, `components/**`), direct OR
transitively imported, does not correctly re-render the consuming route.

Two distinct gaps compound:

1. **Watch gap (`src/`).** `DEFAULT_WATCH_ROOTS`
   (`crates/zfb/src/commands/dev.rs:134`) = `pages, content, components, layouts,
   styles, data` (+ collection roots, + config). **`src/` is NOT watched.** So an
   edit under `src/components/**` produces **no FS event and no tick at all** —
   nothing re-renders. (`src/` *is* a classification/islands/client-script root —
   `policy.rs:237`, `policy.rs:273`, `GranularityPolicy::default().islands_roots`
   — but classification only matters once a tick fires, which it never does.)
2. **Graph-edge gap + blunt fallback (`components/`).** In the DEV path the graph
   is seeded with page nodes carrying **no dep edges**
   (`dev.rs:1222` → `PageDeps::new(page_id, vec![])`); thereafter only
   `DepKind::Content` edges are upserted (`dev.rs:5237`). **No `Module` (page→
   component) edges are ever populated in dev.** So `graph.dirty_pages(component)`
   returns the **empty set** (`crates/zfb-graph/src/lib.rs:591` — it reads only
   the `reverse` index). The orchestrator's `Page|Module|Content|Data` arm
   (`crates/zfb-build/src/orchestrator.rs:452-466`) then hits its
   `dirty.is_empty()` → `PageSelection::All` fallback.

   **Empirically confirmed:** a `components/Header.tsx` edit selects
   `PageSelection::All` today (`current_component_edit_selects_all_pages_imprecisely`).
   So a *directly-imported* `components/**` edit DOES re-render — over-broadly,
   every page — which is exactly why the existing
   `dev_serve_e2e.rs` *scenario 4* passes. The #1284 defect surface for symptom A
   is therefore: (i) `src/**` not watched (no tick), and (ii) the All-fallback is
   imprecise (whole-site re-render on any component edit), masking the missing
   per-route edges.

**This is a route-SELECTION problem, confirmed — NOT a bundle-freshness problem.**
`reload_renderer`/`refresh_bundle_and_routes()` re-bundles SSR from disk on every
page-selecting tick (`dev.rs:~2470`), so once a route is *selected*, it renders
from fresh bundle bytes. The fix is to make `dirty_pages(component)` non-empty
(precise per-route selection), not to touch bundle freshness.

**Fix approach (#1287):** populate per-route `DepKind::Module` edges in the dev
graph from esbuild's metafile `inputs` (see D1); add `src` to the dev watch roots
so `src/**` edits fire a tick at all (see D4-component-part).

### Symptom B — transitively-imported CSS does not refresh `/assets/styles.css`

Editing a CSS file reached transitively (`@import './x.css'`, or a symlinked
workspace dep via `@import '@scope/design-system'`) does not refresh the served
`/assets/styles.css`.

Failing paths:

- **Style-arm asymmetry.** The `Style` arm
  (`orchestrator.rs:480-487`) sets `rerun_css` but, unlike the
  `Page|Module|Content|Data` arm, does **NOT** fall back to `All` when
  `dirty_pages` is empty — it selects `Specific({})` (zero pages).
  **Empirically confirmed:** `current_style_arm_selects_no_pages_without_edges`.
  (For pure CSS-asset refresh the page set does not matter — `rerun_css` is what
  rebuilds `/assets/styles.css` — but the asymmetry is the orchestrator-level
  tell, and it matters for CSS-Modules consumed by a `.tsx`.)
- **No `@import` resolution in Rust.** `zfb-css` does NOT resolve local CSS
  `@import` file dependencies. `pipeline.rs` only **hoists** `@import` at-rules
  (`hoist_external_imports`, `pipeline.rs:347`) and
  `build_synthesised_entry_css` (`engine.rs:392`) passes user CSS through
  verbatim; the Tailwind CLI resolves `@import` invisibly. So zfb never learns
  the real dependency path of an imported CSS file → no Style edge, and (critical
  for the workspace case) **no watch target** for the symlinked real file.
- **Watch gap (symlink).** `notify` is started `RecursiveMode::Recursive`
  (`crates/zfb-watcher/src/lib.rs:181`) and **does not follow symlinks**, so a
  workspace `@import '@scope/design-system'` resolving through a `node_modules`
  symlink (and `node_modules` is excluded anyway) is never watched (see D4).

**Fix approach (#1288):** resolve the CSS `@import` graph to canonicalised real
paths (D2), register those real paths as extra watch targets (D4), and add the
Module→`mark_css` re-scan trigger (see ownership split).

### Symptom C — new utility class in a component is not emitted

A NEW Tailwind utility class authored inside a component (e.g.
`gap-x-hgap-2xs`, `xl:grid-cols-[2.35fr_1fr]`) is not emitted into
`/assets/styles.css` until the CSS entry is touched.

Failing paths:

- **Scan roots omit `src/`.** `DEFAULT_CONTENT_ROOTS`
  (`crates/zfb-css/src/engine.rs:876`) = `pages, components, layouts, content` —
  **omits `src/`** — and `default_source_directives` (`engine.rs:880`) emits
  `@source` directives only for those. So a class in `src/components/**` is
  outside every scanned glob and never reaches the generated stylesheet.
  **Empirically confirmed:** `current_bug_src_root_is_not_a_scan_root`.
- **CSS pipeline never re-runs on a `.tsx` edit.** The CSS pipeline runs only on
  `plan.rerun_css`, which only a `.css` (`PathClass::Style`) edit sets today. A
  `.tsx` `Module` edit never sets `rerun_css`, so even a class authored in a
  *watched* `components/**` file is not re-scanned until the CSS entry is touched.

**Fix approach (#1288):** add `src` to `DEFAULT_CONTENT_ROOTS`/`@source`, and add
the single Module→`mark_css` edit so any component edit re-runs the content scan.

---

## LOCKED DECISIONS

### D1 — per-route module-dep source (for #1287)

**Decision: invoke esbuild with `--metafile=<path>` and parse `metafile.inputs`
to build the per-route transitive `DepKind::Module` edge set.**

`BundlerOutput` / `BundleManifest` (`crates/zfb-build/src/bundler.rs:733-769`)
expose route source paths (`RouteEntry.source_path`) but **no per-route
transitive-import list**. Today esbuild is invoked in `run_esbuild`
(`bundler.rs:5414`) **without** `--metafile` (confirmed: the arg list at
`bundler.rs:5421-5500` has `--bundle --format=esm --platform=neutral … ` but no
metafile). esbuild already runs once per dev bundle, the metafile is a free
by-product (`--metafile=<tmp>.json`), and `inputs` is the canonical
*transitive* import graph esbuild itself resolved — no second resolver pass to
drift from the real bundle.

- **Rejected:** a separate Rust import-resolve pass. It would re-implement
  esbuild's resolution (tsconfig paths, aliases, `.mdx`/`.md` loaders, plugin
  virtual modules) and is guaranteed to drift from what actually got bundled.
- **Mechanism:** add `--metafile` to `run_esbuild`; after the subprocess, read
  the JSON, and for each `output` whose entryPoint maps to a route, walk its
  `inputs` to the source files, then `graph.upsert(PageDeps::new(route_page,
  module_edges))`. Bundle-relative input paths are joined to the project root /
  shadow root and canonicalised so they match watcher event paths.
- **Lowest-risk note:** the metafile reflects the **shadow tree** esbuild reads;
  map shadow paths back to real project paths using the same
  shadow↔real mapping `run_esbuild` already maintains (the `copy_mode` /
  `--preserve-symlinks` logic at `bundler.rs:1559-1570`) so edges key on the
  paths the watcher will actually report.

### D2 — CSS `@import` dependency source (for #1288)

**Decision: parse the `@import` graph in Rust over the project's CSS sources,
following each `@import` target to a canonicalised real path; do NOT try to
harvest Tailwind's dependency output.**

Justification: the Tailwind v4 standalone CLI exposes **no machine-readable
dependency manifest** for what it pulled in (it has a `--watch` mode but emits no
dep list zfb can consume), and zfb already shells out to it as an opaque
`-i/-o` transform (`engine.rs:859`). A small Rust `@import` resolver is
deterministic, testable at Level 1, and — critically — lets us **canonicalise**
each target (`std::fs::canonicalize`) to follow the workspace symlink to the real
file, which is exactly the watch target D4 needs. zfb already owns CSS text
parsing for hoisting (`pipeline.rs:hoist_external_imports`), so the parsing
surface is familiar.

- **Scope:** resolve `@import "..."` / `@import url(...)` targets that resolve to
  on-disk files (relative paths, and bare specifiers resolved against
  `node_modules`/workspace), recursively; skip `@import "tailwindcss"` and other
  virtual/builtin specifiers. Return the canonicalised real path set.
- **Two consumers:** (1) register the real paths as Style edges / watch targets;
  (2) feed them into the dev watcher as extra watch roots (D4).

### D3 — symptom-A observable under the lazy model

**Decision: the test observable is *served-HTML on the NEXT request* (polled via
`poll_until_contains` against the route), with the SSE `page` event asserted as a
secondary signal.**

`lazy_render_tick` (`dev.rs:5017`) marks selected routes **STALE** (re-render on
next request) for a `.tsx`/non-content tick — it does NOT eagerly write the HTML.
This is exactly how `dev_serve_e2e.rs` *scenario 4* (`L1003-1080`) already
asserts a component edit: it (a) asserts an SSE `page` event fires (via the
`pages_stale` gate), then (b) `poll_until_contains` the route until it serves the
new marker (lazy: the first request renders + write-through). The acceptance
tests for #1284 use the **same** observable:

- **Primary (must):** `GET <route>` (or `GET /assets/styles.css` for B/C) serves
  the new marker/bytes on the next request → `poll_until_contains` /
  `poll_until_file_contains` after the request.
- **Secondary:** an SSE `page` event (`next_sse_event_name == Some("page")`).
- Do **NOT** assert an eager disk write for the lazy default (the sibling stays
  stale until requested — `assert_file_lacks` is the lazy discriminator).

### D4 — symlinked-dep watch decision

- **Confirmed:** `notify` does not follow symlinks (started
  `RecursiveMode::Recursive`, `zfb-watcher/src/lib.rs:181`; the watcher already
  supports out-of-root `extra` targets at `lib.rs:201`, the `extraWatchPaths`
  channel).
- **CSS (#1288):** the `@import` resolver (D2) yields **canonicalised real
  paths** following the workspace symlink; #1288 registers those real paths as
  `extraWatchPaths`-style watch targets automatically — **no manual user
  config**. An out-of-root real path then arrives at the orchestrator's
  `External`/`Style` handling; the resolved Style edge makes it map correctly.
- **Components (.tsx) (for #1287):** a symlinked workspace **component** dep does
  NOT need a separate manual watch story — it is covered by the **bundler/metafile
  path (D1)**. esbuild resolves through the symlink and records the real input in
  the metafile; that real path becomes a `Module` edge. **However**, the real
  path must also be **watched** for an edit to fire a tick — so #1287 must
  register the metafile-discovered out-of-root real `Module` deps as extra watch
  targets too (mirroring how #1288 registers CSS real paths). For in-repo `src/**`
  (the common case), the simpler fix is **adding `src` to `DEFAULT_WATCH_ROOTS`**
  so those edits are watched directly.

---

## Orchestrator ownership split (so #1287 and #1288 do not collide)

- **#1287 owns** the `Page|Module` page-SELECTION path: replacing the blunt
  `dirty.is_empty() → PageSelection::All` fallback
  (`orchestrator.rs:462-466`) with precise per-route selection backed by the new
  `Module` edges (D1), plus adding `src` to the dev watch roots and registering
  out-of-root `Module` real paths as watch targets (D4-component-part).
  **#1287 must NOT touch the `mark_css` Module trigger below** — that is #1288's.

- **#1288 owns** the `Style` arm (`orchestrator.rs:480-487`) AND the single
  **Module-edit → CSS content re-scan** edit. **Exact insertion point:** inside
  the `Page|Module|Content|Data` arm, immediately after the existing islands
  block at `orchestrator.rs:473-478`, add a sibling guard:

  ```rust
  // (existing)
  if matches!(class, PathClass::Module)
      && self.config.policy.is_islands_candidate(&path)
  {
      plan.mark_islands();
  }
  // (#1288 ADDS, right here at ~L478:)
  if matches!(class, PathClass::Module) {
      plan.mark_css(); // a component edit may author a new utility class (symptom C)
  }
  ```

  This is the **only** orchestrator edit #1288 makes inside the `Page|Module` arm;
  it is additive (a new `if`), so it does not conflict with #1287's rewrite of the
  *page-selection* lines (`L462-467`) in the same arm. The `mark_css()` helper is
  `plan.rs:218`. #1288 also adds `src` to `DEFAULT_CONTENT_ROOTS`/`@source`
  (`engine.rs:876`) and implements the D2 resolver + D4 CSS watch registration.

---

## Reproduction tests (all `#[ignore = "pending fix: #1284"]` for fixed-behaviour)

| File | Level | Symptom | What it pins |
|---|---|---|---|
| `crates/zfb-graph/tests/dev_dep_invalidation_1284.rs` | 1 | A, B | dev graph has no Module/Style edges → `dirty_pages` empty (2 current-bug asserts pass now; 3 fixed-behaviour ignored) |
| `crates/zfb-build/tests/dev_dep_invalidation_1284.rs` | 1 | A, B | `plan_for_changes` Page\|Module All-fallback vs Style-arm zero-pages asymmetry (2 current-bug pass now; 2 fixed ignored). **Build with `--no-default-features`.** |
| `crates/zfb-css/tests/dev_dep_invalidation_1284.rs` | 1 | C, B | `DEFAULT_CONTENT_ROOTS`/`default_source_directives` omit `src/`; `@import` resolver contract placeholder (1 current-bug passes now; 2 fixed ignored) |
| `crates/zfb/tests/dev_dep_invalidation_1284_e2e.rs` | 4 | A, B, C | Wave-3 acceptance gate stubs, fully `#[ignore]`d `todo!()` bodies wired to the D3 observable. Compiles (verified `--no-default-features`); full V8 dev-e2e run is the manager's job |

The non-ignored "current_bug_*" tests **assert today's broken behaviour** to lock
the regression boundary; they are NOT ignored and pass now. The fixed-behaviour
tests are `#[ignore]`d so `cargo test` stays green until the fix waves un-ignore
them.

### What was NOT tested (blind spots)

- The full Level-4 V8 dev-e2e loop was **not run** (first-compile 15-30 min; the
  e2e stubs are `#[ignore]`d `todo!()` and deferred to Wave-3 / the manager).
- The metafile-parsing path (D1) has no test yet — it does not exist until #1287.
- Symptom A's `src/**` watch gap is reasoned + reproduced at the config/graph
  level (`DEFAULT_WATCH_ROOTS` omits `src`), not via a live FS-watch e2e here.
