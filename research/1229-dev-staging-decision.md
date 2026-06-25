# Z0 Decision Record — Dev-server rendering of package-owned injected routes

_Gate output for epic #1228 (the dev-half follow-up to the now-closed build-half #1191/#1192). Wave 2 (#1230 S2) through Wave 7 (#1235 S7) implement against this. Verified against the live tree on `base/inject-route-dev-render`; every anchor below was re-confirmed against the actual source. This note mirrors the #1192 build-half decision record's format. **No production code in S1** — decision + the downstream sub-issue body edits._

## TL;DR

- **Verdict: B1 (multi-root) — keep `pages_dir` = the real `project_root/pages`, additionally feed an injected-only synthesized module set to the dev bundler via the EXISTING `build_pages_root` seam, and seed the injected routes into the dev route universe.** B2 (a re-synced overlay rebuilt every `refresh_bundle_and_routes`) is rejected: it would re-copy the user's entire `pages/` into a temp dir on every tick (cost scales with project size, the exact thing #1182/#1161 fought), and — worse — it changes the dependency-graph **path identity** of every user page from `pages/x.tsx` to `<overlay>/pages/x.tsx`, which silently breaks the watcher's source-keyed HMR (`routes_by_source`, the orchestrator graph, and `derive_watch_roots` all key on the real `pages/` paths). B1 touches user-page identity **not at all**.
- **The injected modules are staged the way the build already stages them, but WITHOUT copying user `pages/`.** Reuse `package_routes.rs::resolve_build_pages_root`'s synthesizers (`synthesize_static_overlay_module` / `synthesize_dynamic_overlay_module` + `pattern_to_pages_rel`) and its FULL validation (`collect_user_pages_shape_keys` user-precedence drop, package-vs-package shape-key collision, case-insensitive `dest.exists()` guard, trailing-`index` rejection, `.client` rejection, the documented optional-catchall limitation). The injected-only module set is materialized into a session-lifetime temp dir (one `tempfile::TempDir` held on `DevRebuildInputs`, NOT rebuilt per tick) and threaded into BOTH the dev bundler (`assemble_bundler_input`'s `build_pages_root: Option<&Path>`, which dev currently passes `None` at `dev.rs:3327-3330`) AND the dev route scan.
- **Concrete-URL lookup contract (the critical decision): HYBRID.** `DevRouteTables.url_index` (`dev.rs:2253`) is keyed by **concrete** URL. **Static** injected routes (URL == pattern, e.g. `/preset-about`) are seeded directly into `url_index` + `routes_by_source` at boot and on every table swap, exactly like a normal static page. **Dynamic** injected routes (`/preset-docs/[slug]`) have no concrete URL until the request, and dev does NOT enumerate `paths()` Rust-side — so they are handled by a **request-time pattern-aware fallback** in `lazy_render_adapter.rs`: on a `url_index` miss (`lookup_by_url` → `None`), consult the `InjectedRouteSet` (threaded into the adapter), and if an injected **pattern** matches the request URL via the existing `injected_routes::pattern_matches`, **synthesize a `RouteUniverseEntry` on the fly** (`url_path` = the concrete request URL, `output_path` derived from it) and run it through the unchanged `render_one` → guarded-write → `html_root` flow. This reconciles "no Rust-side `paths()` enumeration" (the epic's pinned fact — Hono inside the bundle extracts params and matches `paths()` on the fly) with the concrete-keyed lookup.
- **`output_path` derivation (pinned):** reuse `render_pipeline.rs::build_output_path_for_resolved_url` (`render_pipeline.rs:650`) — `/preset-docs/a` → `preset-docs/a/index.html`, the root `/` → `index.html`, a non-HTML extension URL → the bare path. The synthetic entry's `route_key` = the injected **pattern** (the template), `static_html=false`, `source_path=None` (V8-rendered like any dynamic SSG route).
- **Staleness seeding (pinned):** static injected routes are seeded stale on boot AND on every route-table swap (`note_table_swap`, `dev.rs:2112`) — a content edit refreshing an injected route depends on stale-marking, not just dependency-graph membership. Dynamic injected routes are stale-on-miss by construction: the request-time synthetic path renders unconditionally when `url_index` misses and the file is absent/stale (the same claim/guarded-write discipline the existing adapter uses).
- **HMR seam (pinned):** content edits reach injected routes through the existing `with_external_invalidation` / `PageSelection` seam (`OrchestratorConfig::with_external_invalidation`, `orchestrator.rs:272`; wired at `dev.rs:471-472`). **`node_modules` is NOT in the watch roots** (`DEFAULT_WATCH_ROOTS`, `dev.rs:116-122`: `pages`/`content`/`components`/`layouts`/`styles`/`data` + collection roots), so an injected entrypoint living under `node_modules/@takazudo/...` is **restart-only** — editing the package source itself requires a `zfb dev` restart. This is the correct contract (a published package is not a project source); content the package route READS (collections under watched roots) DOES live-refresh.
- **Trailing-slash + base-prefix normalization (pinned):** the render-on-request hook already receives **prefix-stripped, query-stripped** paths (`render_hook.rs:99-104`, `routes.rs:990-998`); injected pattern-matching and `output_path` derivation run on that same normalized shape, so trailing-slash/base-prefix parity with normal pages is automatic. `lookup_by_url` already normalizes trailing slash + `index.html` duality + percent-encoding (`dev.rs:2703-2722`).
- **Render-dispatch wiring (pinned): extend the dev route universe + the lazy adapter — do NOT add a render branch to `routes.rs`.** The `routes.rs` #255 block (`routes.rs:1062-1076`) currently only logs the match; it must be **demoted to the fallback-only role** (or removed) because the render-on-request hook (`routes.rs:985-1026`), which fires *before* it, will now do the actual rendering. Putting a real render in `routes.rs` would duplicate the renderer-mutex / scratch-dir / guarded-write / lock-ordering discipline the adapter already owns (`lazy_render_adapter.rs:48-62`) and would break the documented lock ordering.
- **Multiple-preset collision (pinned):** reuse the build-half's exact policy via `resolve_build_pages_root` — user `pages/` always wins (pre-scan shape-key drop), two package routes with the same shape hard-error naming both plugins + patterns, case-only path collisions hard-error. Dev gets this for free by reusing the same function; the only dev addition is that the SAME survivor/precedence result must also seed `url_index` and the `InjectedRouteSet` so the lookup and the fallback agree with the bundle.

---

## 1. Staging mechanism — B1 (multi-root) vs B2 (re-synced overlay)

### The decision: B1.

The dev pipeline reads/keys the pages tree from several places, and the load-bearing constraint is **source-path identity**:

| Consumer | Anchor | Keys on |
|---|---|---|
| Dev route scan (boot + every refresh) | `dev.rs:3932`, `dev.rs:2885` `Router::scan(&pages_dir)` | a real dir; `Route.source_path` |
| Dev bundler `pages_dir` | `dev.rs:3327-3330` → `bundler_input.rs:196,211` (`build_pages_root: Option<&Path>`) | the walked root |
| `routes_by_source` (HMR fan-out narrowing) | `dev.rs:2237`, `dev.rs:3690-3696` | the page module's **project-relative `source_path`** |
| Orchestrator dep graph + watcher | `dev.rs:471-474`, `orchestrator.rs:320` `plan_for_changes` | source paths under the watch roots |
| `derive_watch_roots` | `dev.rs:152-171` | literal `"pages"` (+ content/components/etc.) |

**Why not B2 (re-synced overlay, the build-half's exact model):** the build runs `resolve_build_pages_root` ONCE and the overlay lives for one build. Dev is a long-lived session that re-scans + re-bundles on every watcher tick (`refresh_bundle_and_routes`, `dev.rs:2863`). Porting the build overlay verbatim means:

1. **Per-tick full `pages/` copy.** `resolve_build_pages_root` copies the user's entire real `pages/` into the temp dir (`copy_dir_recursive`, `package_routes.rs:678`). Doing that on every tick reintroduces a size-scaling per-tick cost — the precise class of regression #1182/#1161/#1166 spent effort removing.
2. **Path-identity break (the real HMR hazard).** If dev pointed `Router::scan` + the bundler at the overlay, every user page's `source_path` would become `<overlay-temp>/pages/x.tsx` instead of `pages/x.tsx`. But `routes_by_source`, the dependency graph, and the watcher all key on the REAL `pages/` paths (the watcher physically watches `project_root/pages`, `dev.rs:152`/`471`). The overlay copies would never match a watcher event → **user-page HMR silently dies** the moment any preset registers a route. This is the dev-specific trap the build half never faced (it has no watcher).

**Why B1 works:** keep `pages_dir` = the real `project_root/pages` for the route scan and watcher (user-page identity untouched — HMR/watch paths are byte-identical to today). Separately materialize an **injected-only** module set (NO user-page copy) into a session-lifetime temp dir, and:

- thread its root into the dev **bundler** via the existing `build_pages_root` arg so the injected entrypoints land in the dev bundle (today dev passes `None`, `dev.rs:3327-3330`) — this closes epic gap (1), "injected entrypoint not in the dev bundle";
- scan it (a second, tiny `Router::scan` over just the injected modules, or a direct synthesis of route entries from the survivor set) and **merge** the resulting routes into `DevRouteTables` — this closes gap (2), "injected routes not in `DevRouteTables`".

Because the injected set excludes user pages, there is no per-tick `pages/` copy and no user-page path-identity change. The injected modules' own source paths live under the temp root; they are `node_modules` entrypoints by nature (restart-only, §4), so they never need to match a watcher event.

### Validation reuse (non-negotiable for B1).

The injected-module synthesis MUST reuse `package_routes.rs::resolve_build_pages_root`'s **full** semantics, not a weaker subset. Concretely, S2 calls a thin variant/wrapper of `resolve_build_pages_root` that:

- runs `collect_user_pages_shape_keys(real_pages_dir)` and drops user-shadowed injected routes (user precedence) — `package_routes.rs:139-192`;
- hard-errors package-vs-package shape-key duplicates naming both plugins + patterns — `package_routes.rs:193-205`;
- guards each write with the case-insensitive `dest.exists()` check — `package_routes.rs:258-294`;
- rejects trailing-`index` (`pattern_to_pages_rel`, `package_routes.rs:408-414`) and `.client` segments (`package_routes.rs:168-178`);
- carries the SAME documented optional-catchall + `bundle.exclude` limitations (`package_routes.rs:46-64`) — dev inherits them verbatim; do not silently diverge.

The cleanest path is to **factor the survivor-selection + synthesis out of `resolve_build_pages_root` so dev and build share it**, with a flag for "copy user pages (build)" vs "skip the copy (dev)". This keeps byte-for-byte parity with `zfb build` for the same URL (the non-negotiable): the synthesized `.tsx` for a given pattern is produced by the identical `synthesize_*_overlay_module` call, so the dev bundle's injected module is the same module the build bundles.

---

## 2. Concrete-URL lookup contract (CRITICAL) — HYBRID

`url_index` is keyed by **concrete** URL (`dev.rs:2253`, consumed by `lookup_by_url`, `dev.rs:2703`). The two injected-route classes are handled differently:

### Static injected routes (URL == pattern) → seed `url_index` directly.

`/preset-about` has a single concrete URL identical to its pattern. It is a normal static SSG page once staged. S3 seeds it into `routes_by_source` + `url_index` (via the existing `build_url_index`, `dev.rs:3622`) at boot and on every table swap, with `output_path = build_output_path_for_resolved_url("/preset-about", None)` = `preset-about/index.html`. From that point `lookup_by_url("/preset-about")` hits, the lazy adapter renders it through `render_one`, and the dev server serves it from `html_root` — no new code path beyond the seeding.

### Dynamic injected routes (URL unknown until request) → request-time synthetic entry.

`/preset-docs/[slug]` has no concrete URL at boot, and dev intentionally does NOT enumerate `paths()` Rust-side (the epic's pinned fact: Hono inside the live bundle extracts params + matches `paths()` on the fly). So S4 adds a **fallback in `lazy_render_adapter.rs::render_stale_route`** (`lazy_render_adapter.rs:189-201`), right where `lookup_by_url` currently returns `NoRoute`:

```
let entry = match self.session.lookup_by_url(url_path) {
    Some(e) => e,                                  // existing concrete hit (incl. seeded static injected)
    None => match self.injected_routes.find_match(url_path) {   // NEW: pattern-aware fallback
        Some(rec) => synthesize_entry(url_path, &rec.pattern),  // on-the-fly RouteUniverseEntry
        None => return LazyRenderOutcome::NoRoute,
    },
};
```

where `synthesize_entry` builds:

```
RouteUniverseEntry {
    url_path:    url_path.to_string(),                                  // the CONCRETE request URL
    output_path: build_output_path_for_resolved_url(url_path, ext),    // render_pipeline.rs:650
    route_key:   rec.pattern.clone(),                                   // the template (Hono lookup key)
    static_html: false,
    source_path: None,
}
```

The rest of `render_stale_route` is unchanged: `claim_stale(&entry.output_path)` → `render_claimed_entry` (`render_one` GETs the concrete `url_path` against the live V8 host, which already has the injected module in its bundle from §1, so Hono matches `paths()` and renders) → `request_write_guarded` into `html_root`. The `InjectedRouteSet` is **threaded into the adapter** (it is already constructed at `dev.rs:332-342` and handed to `ServeOpts.injected_routes`; S4 also passes a clone — or the `DevRenderSession`'s handle — into `make_render_on_request_handle`, `lazy_render_adapter.rs:114`).

**Reuse `injected_routes::pattern_matches` / `InjectedRouteSet::find_match`** (`injected_routes.rs:68-152`) for the matching — it already implements the full `pages/` grammar (literal, `[slug]`, `[...rest]`, `[[...rest]]`) and first-registered-wins ordering. Do NOT write a second matcher.

---

## 3. `output_path` derivation + staleness seeding

- **Derivation (single source of truth):** `render_pipeline.rs::build_output_path_for_resolved_url(url, extension)` (`render_pipeline.rs:650`). HTML → `<trimmed-url>/index.html` (root → `index.html`); non-HTML extension → bare path. Use it for BOTH the seeded static entries (S3) and the request-time synthetic entries (S4) so the output layout is identical to a normal page and to `zfb build`. The `extension` is derived from the URL's final segment exactly as the normal dynamic path does.
- **Static seeding stale-marking:** seed the static injected routes' `output_path`s as stale on boot AND in `note_table_swap` (`dev.rs:2112`, runs on every full refresh). Rationale (pinned in the spec, mirrors #1025): a content edit that should refresh an injected route's HTML depends on the route being **marked stale**, not merely present in `routes_by_source`. The boot seed makes the first request render; the per-swap seed makes a post-edit request re-render.
- **Dynamic seeding:** no boot seed is possible (no concrete URL). The request-time synthetic path is stale-by-construction — when `url_index` misses, the file is (by definition for a dynamic injected URL) absent or stale, and `render_stale_route` renders it. A subsequent content edit re-triggers via the HMR seam (§4) which re-stales the concrete output paths the watcher knows about; a never-before-requested dynamic URL simply renders on first request.

---

## 4. HMR seam — `with_external_invalidation` / `PageSelection`; node_modules restart-only

- **Live content refresh:** content edits propagate through `OrchestratorConfig::with_external_invalidation` (`orchestrator.rs:272`, wired in `dev.rs:471-472`). A change under a watch root resolves to `PageSelection::Specific(pages)` (or the conservative `PageSelection::All`); the tick re-renders the affected sources. An injected route whose page READS a watched collection refreshes through this seam like any page reading that collection — the content snapshot is rebuilt every tick (`assemble_and_bundle_dev` embeds a fresh snapshot, `dev.rs:3300-3301`), so the injected route's `getCollection(...)` sees the edit.
- **node_modules entrypoints are restart-only (pinned contract).** `DEFAULT_WATCH_ROOTS` (`dev.rs:116-122`) is `pages`/`content`/`components`/`layouts`/`styles`/`data` (+ collection roots). `node_modules` is deliberately absent. An injected entrypoint at `node_modules/@takazudo/zudo-doc/dist/...` is therefore **not watched** — editing the package's own source requires a `zfb dev` restart. S5 must document this explicitly (not a bug; a published dependency is not project source). The S6 fixture should assert: editing the package entrypoint does NOT hot-refresh (restart-only), while editing watched content the route reads DOES.

---

## 5. Trailing-slash + base-prefix normalization

The render-on-request hook receives the **prefix-stripped, query-stripped** path (`routes.rs:990-998` strips `state.base_prefix`, then drops `?…`; `render_hook.rs:99-104` documents "does NOT include the `base` prefix"). So:

- injected `pattern_matches` runs on the same normalized `/preset-docs/a` shape the production adapter would see — base-prefix parity is automatic;
- `lookup_by_url` already collapses trailing-slash, `index.html` duality, and percent-encoding before consulting `url_index` (`dev.rs:2703-2722`, `url_index_lookup_keys`), so seeded static injected routes inherit that normalization;
- the request-time synthetic `output_path` is derived from the already-normalized URL, so `/preset-docs/a` and `/preset-docs/a/` resolve to the same `preset-docs/a/index.html` — matching a normal dynamic page.

No new normalization code is needed; both paths ride the existing one.

---

## 6. Render-dispatch wiring — extend the universe/adapter, NOT a routes.rs branch

The render-on-request hook (`routes.rs:985-1026`) fires **before** the #255 injected-route block (`routes.rs:1052-1076`) and before the page-cache/disk legs (the precedence chain in `render_hook.rs:26-36`). With §2 in place, the hook (the lazy adapter) renders the injected route into `html_root`, and `serve_page` then serves it from the `html_root` disk leg on the same request.

Therefore:

- **S3/S4 extend the dev route universe + `lazy_render_adapter`** (seed static entries; add the dynamic synthetic-entry fallback). This reuses the adapter's renderer-mutex capture, scratch-dir render, `request_write_guarded`, and the documented lock ordering (`lazy_render_adapter.rs:48-62`) verbatim.
- **The `routes.rs` #255 block is demoted.** Its current job (log + fall through) is superseded by the hook actually rendering. It should either be removed or reduced to a pure diagnostic that no longer implies "renderer integration is a follow-up" (its log message at `routes.rs:1073` becomes stale). Do NOT add a render call here: it would re-implement (and risk inverting) the renderer-mutex → exclusion-lock ordering the adapter already gets right, and a request-time write outside the exclusion lock can land inside a tick's deferred-prune window (the exact hazard `lazy_render_adapter.rs:64-72` exists to avoid).

---

## 7. Multiple-preset collision

Inherited wholesale from the build half by reusing `resolve_build_pages_root`'s survivor selection (§1):

- **User `pages/` wins** over any injected route of the same shape (pre-scan shape-key drop, `package_routes.rs:186-192`) — including the dev-only `/` reservation the epic calls out (a user root page beats an injected `/`).
- **Two presets, same shape** (`/blog/[slug]` + `/blog/[id]`) → hard error naming both plugins + patterns (`package_routes.rs:193-205`).
- **Case-only path collision** on a case-insensitive FS → hard error (`package_routes.rs:274-285`).
- **First-registered-wins** among non-colliding overlapping patterns is the runtime tiebreak the `InjectedRouteSet::find_match` linear scan already implements (`injected_routes.rs:68-72`, declaration-ordered) — used by the §2 dynamic fallback when two patterns could match one URL.

The dev-specific addition: the SAME survivor set that seeds the bundle (§1) must seed `url_index`/`routes_by_source` (static) and back the `InjectedRouteSet` (dynamic fallback), so the three views never disagree. Build the `InjectedRouteSet` from the post-precedence survivors, not the raw registration list, so a user-shadowed injected pattern does not leak into the request-time fallback.

---

## Sharp edges for the implementer waves

1. **Do NOT re-point `Router::scan`/the watcher at an overlay that contains user pages.** That is the B2 trap — it breaks user-page HMR by changing source-path identity. Keep the real `pages/` for the scan + watcher; stage ONLY the injected modules elsewhere (B1). (§1)
2. **Reuse the build's FULL validation, not a subset.** Factor `resolve_build_pages_root`'s survivor selection + synthesizers so dev and build share them; a dev-local re-implementation will drift from the byte-for-byte-parity requirement and miss a precedence edge (case-insensitive, shape-dup, trailing-`index`, `.client`). (§1)
3. **The dynamic fallback's `route_key` is the PATTERN, the `url_path` is the CONCRETE request URL.** Swapping these breaks the prerender-map join and the V8 `paths()` match. (§2)
4. **Thread the post-precedence `InjectedRouteSet` into the adapter**, not the raw registration list — a user-shadowed pattern must not match in the request-time fallback. (§2, §7)
5. **Stale-seed static injected routes on EVERY table swap, not just boot** — otherwise a content edit won't refresh an already-rendered injected page (the route stays in `routes_by_source` but is never re-claimed). (§3)
6. **node_modules is unwatched by design** — editing the injected entrypoint itself is restart-only; only watched content the route reads live-refreshes. Document it; the fixture asserts both halves. (§4)
7. **Demote the `routes.rs` #255 block; do not render there.** The hook renders before it; a render in `routes.rs` duplicates and risks inverting the adapter's lock ordering. (§6)
8. **Parity guarantee:** with no injected routes, dev is byte-identical to today — `build_pages_root` stays `None`, no temp dir, no `url_index`/`InjectedRouteSet` additions. Gate every new path on a non-empty survivor set. (§1)
