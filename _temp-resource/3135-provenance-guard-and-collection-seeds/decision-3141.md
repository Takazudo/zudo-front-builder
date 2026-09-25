# Decision #3141 — collection-seed fix shape (Track D, epic #3135)

Measured on `claude/relaxed-dirac-irobue` at `c322e31` (= #3138 fixture + #3139
memoization), 2026-09-25, debug `cargo build -p zfb` with esbuild 0.25.12 /
Tailwind 4.2.0 staged via `ZFB_ESBUILD_BIN` / `ZFB_TAILWIND_BIN`, 4 shared CPUs.

## Decision: (a′) — include-filter-scoped seeding, WITHOUT MDX-import following

Choose **(a) as refined below**. Not (c): B1 alone does not touch the flip or the
post-flip drain (see numbers). Not (b): no contingency is needed to hit the
#3133 target on the faithful fixture; two follow-up candidates are listed at
the end but are out of #3142's scope.

The refinement matters: #3142's default spec (and plan.md B2) assumed the
realistic chain `button.mdx → import ./button.tsx → workspace package` keeps
`button.tsx` a seed. **It does not, because zfb's MDX compiler drops MDX-level
ESM imports by design.** `crates/zfb-content/src/mdx_jsx_emit.rs`
(`mdx_to_jsx_module_inner`, the `mdx_esm_parse` comment): import/export
statements are classified as `MdxjsEsm` nodes "so the emitter can silently drop
them". The compiled module for the fixture's `button.mdx` is

```js
const Button = _components.Button ?? components.Button;
if (!Button) throw new Error("MDX requires `Button` to be passed via the `components` prop");
```

Components reach MDX through the `components` prop / `mdx-components.tsx`
(#616). Confirmed at the esbuild level: the kept shadow's `.zfb-metafile.json`
for the with-collection build lists `content/componentDocs/button/button.mdx` as
an input and **none of** `button.tsx`, `shared-utils`, `shared-icons`,
`leftpad-priv`. The non-included `.tsx` is not even copied into the shadow
(`materialise_collection` treats `tsx` as a content extension and applies the
include filter). So there is nothing to "follow" from MDX, and
`compile_mdx_to_jsx_module_cached` + swc parsing of its output +
`discover_module_preprocessing_with_context` chasing — the expensive half of the
default spec — must be dropped, not implemented.

## Why the #3138 fixture did not reproduce (and what made it faithful)

Manager's run on the merged base: with-collection `physical_scans=1
logical_visits=1 workspace_staging_activated=false`, control `0/0/false`, equal
wall time. Traced with temporary (uncommitted) `eprintln!`s in
`extend_node_modules_dependency_staging`:

1. **No `apps/site/node_modules`** → `commands::bundler_input` uses the
   embedded vendor tree and sets `node_modules_preserve_symlinks = true`, which
   makes both `resolve_from_canonical_package` and
   `allow_workspace_physical_fallback` false. The seeded workspace package
   (`shared-utils`) is scanned once, but its pnpm-private `leftpad-priv` /
   `shared-icons` imports are simply UNRESOLVED (the lexical resolver is bounded
   to `project_root`, the physical fallback is disabled).
2. **Empty tsconfig `paths`** → `esbuild_will_preserve_symlinks` is true, so even
   with a real `node_modules` the private siblings are only DEFERRED
   (`deferred_physical_dependencies`) until staging flips — and it never does:
   the seeded dir lands in `staging_dirs` as
   `<ws>/packages/ui/node_modules/shared-utils`, **outside `project_root`**, and
   the flip predicate only inspects `staging_dirs` entries under `project_root`
   (`project_path_is_inside_node_modules`) plus `staging_alias_dirs` values.
3. Side finding (pre-existing, production): `resolver.resolve(&collection.root)`
   joins the relative collection path UNNORMALISED
   (`<ws>/apps/site/../../packages/ui/src/components`), so out-of-root seeds pass
   `seed.starts_with(project_root)` lexically, become their own logical
   importer, and resolve bare imports by walking up through the `..` into
   `packages/ui/node_modules/`. That is the only reason an out-of-root seed
   reaches a workspace package at all; an absolute collection path would use the
   synthetic `<project>/entry` importer instead. Behaviour depends on path
   spelling.

Test-only fix (this PR): `apps/site/{package.json,tsconfig.json}` added to the
template; `materialise_fixture` now builds a real `apps/site/node_modules`
(monorepo `preact` / `preact-render-to-string` / `hono` `.pnpm` entries found by
prefix, the binary's vendored `@takazudo/{zfb,zfb-runtime}` copied into the
fixture's OWN `.pnpm` store so they canonicalise inside a `node_modules` and
survive the post-flip isolated view, workspace links for `shared-utils` / `ui`).
Soft print → hard assertions.

## Numbers (faithful fixture, 3 runs each, clean binary)

| Variant                | `[zfb-staging-stats]`                                                   | `zfb build` wall     |
| ---------------------- | ----------------------------------------------------------------------- | -------------------- |
| with-collection        | `physical_scans=7 logical_visits=9 workspace_staging_activated=true`    | 3582 / 3512 / 3709 ms |
| with-collection, B1 memo reverted (local switch, not committed) | `physical_scans=9 logical_visits=9 …=true` | — |
| control                | `physical_scans=0 logical_visits=0 workspace_staging_activated=false`   | 2822 / 3012 / 2939 ms |

with-collection visit order: `packages/ui/node_modules/shared-utils` (seeded by
`button.tsx` AND `orphan.tsx`) → its `leftpad-priv` and `shared-icons` aliases →
`shared-icons`' `leftpad-priv` alias → **flip** (workspace aliases) → deferred
live drain: `@takazudo/zfb-runtime`, `hono`, `preact`, `preact-render-to-string`,
`packages/ui/node_modules/leftpad-priv`. `leftpad-priv`: 3 logical visits, 1
physical scan (3 without B1). The ~600 ms (+20 %) gap is the drain — `preact`'s
whole install (`compat/`, `hooks/`, `debug/`, `test-utils/`, every `.d.ts`)
import-parsed by swc. That is #3133's "opening files one at a time under
`node_modules/.pnpm/**`" at toy scale.

Second independent trigger, measured without the tsconfig (ws4: `pages/index.tsx`
imports `shared-utils`): with-collection `5/10/true` — the in-root import flips
staging, and the collection's whole closure is then walked on top. This is the
"if `apps/site` already imports workspace packages, staging is on regardless and
the cost is the extra closure" case from #3141's body: the collection adds 5
visits (0 extra scans thanks to B1; +5 scans without it).

Why B1 alone is not enough (rejects (c)): B1 removes only the N× multiplier
(2 of 9 scans here; at #3133 scale N = number of importers of each pnpm-private
dep). The flip and the drain — every deferred live dep and every workspace
package's closure walked once — remain, and they are caused by seeds esbuild
provably never uses.

## Spec for #3142 (file:symbol)

1. **`crates/zfb-build/src/bundler.rs` `bundle()` ~3113-3128 (`source_graph_roots`)**:
   stop pushing collection roots into `source_graph_roots`. Instead pass each
   collection to the seed walk as a filtered root:
   `(normalize_path_lexical(&resolver.resolve(&collection.root)), CollectionFilter, logical root = project_root.join("content").join(&collection.name))`.
   `content_dir` default (no collections), `injected_pages_root`,
   `KNOWN_SOURCE_DIRS`, first-party staging dirs: unchanged.
2. **`collect_project_source_module_graph_seed_files` (~10387)**: accept the
   filtered roots and apply, per file, the SAME predicate
   `materialise_collection` uses at ~8961-8975: when the collection has a glob
   filter and the file's extension is `md` / `mdx` / `tsx`, keep it only if
   `filter.matches_relative(rel_posix)`; every other extension passes through
   unchanged (it is materialised, so it can be an esbuild input and must keep
   seeding). `md` / `mdx` are still never seeds (`raw_source_extension`). Build
   the filter with `zfb_content::collection::CollectionFilter::new(include, exclude)`
   exactly as `materialise_collection` does, so the seed set equals the
   shadow's set of module-graph-reachable files by construction. Do NOT add an
   MDX compile / swc pass / `discover_module_preprocessing_with_context` chase.
3. **Same function, resolution base**: register the collection's logical root
   in `logical_source_roots` so an out-of-root matched file gets the logical
   importer `<project>/content/<name>/<rel>` — the location esbuild resolves it
   from in the shadow. With the root normalised in step 1, the lexical
   `seed.starts_with(project_root)` no longer misfires on `..` paths, so
   `extend_node_modules_dependency_staging`'s existing
   `root_entry_dependency_logical_importers` lookup (~10556-10570) takes over
   with no change there. This is separable: if it trips the stage-escape audit
   lanes, land steps 1-2 and file step 3 as a follow-up with the same test.
4. `crates/zfb/src/commands/bundler_input.rs:352-362` already carries
   include/exclude in `ContentCollectionSpec`; no change. `allow_outside_root`
   is not needed for seeding.
5. **Disclose in the PR**: the "MDX bare imports now seed" behaviour in #3142's
   default spec item 4 is VOID — MDX ESM is dropped by the compiler; nothing new
   is staged. A `.tsx` an `include` glob matches keeps seeding (and is the only
   collection file that can flip staging, legitimately).

## Target stats after #3142 (the fixture harness must be updated to these)

- with-collection: `physical_scans=0 logical_visits=0 workspace_staging_activated=false`
  (identical to control — the collection contributes no seeds under `**/*.mdx`).
- control: unchanged `0/0/false`.
- Wall time: with-collection within run-to-run noise of control (control here:
  2822-3012 ms; the pre-fix +600 ms drain disappears). B3 (#3143) derives its
  dev-ready deadline from a FRESH `zfb dev --port 0` control measurement on the
  faithful fixture (rule 8) — #3138's 3543 ms was embedded-vendor mode and is not
  comparable.
- Revert-and-fail: the current assertion (`7/9/true`) is the RED state.

## Regression tests #3142 must add (revert-and-fail each)

- **(a) sibling never flips** — unit-level, `crates/zfb-build/tests/` beside
  `bundler_staging_scan_memo.rs` (mock esbuild, `node_modules_dir` set,
  non-empty `tsconfig_paths`, the same 3-logical-path store shape): out-of-root
  collection `include: ["**/*.mdx"]`, `button.mdx` + non-included `button.tsx` /
  `orphan.tsx` importing a workspace package → `workspace_staging_activated == false`,
  `logical_visits == 0`. Plus flip `collection_seed_3133_baseline.rs`'s
  `expected_stats` to `0/0/false` (real-binary confirmation).
- **(b) matched `.tsx` still seeds** — same fixture with `include:
  ["**/*.{mdx,tsx}"]`: under a non-empty `bundle.exclude` the matched
  `button.tsx`'s bare dep is staged; under an empty exclude its workspace import
  flips staging. With step 3, assert the staged logical path sits under
  `<project>/node_modules/…` (the site-level location esbuild consults), not
  `<ws>/packages/ui/node_modules/…`.
- **(c) materialised non-content sibling still seeds** — a `helper.ts` beside an
  included `.mdx` importing a bare dep under non-empty `bundle.exclude` is
  staged (pins the "same predicate as materialisation" rule so the two cannot
  drift).
- **(d) MDX imports stage nothing** — an `.mdx` with `import x from "pkg"` yields
  `logical_visits == 0`; a future MDX-import-preserving change must surface here.
- **(e) `content_dir` default unchanged** — keep
  `glob_fixed_point_stages_plain_dependency_closure_of_a_matched_file`,
  `workspace_package_staging_*`, `bundler_out_of_root_import*.rs`,
  `collections_outside_root_build_check_snapshot.rs` green; run the
  stage-escape env-gate lane
  (`bundler_root_workspace_stage_escape_audit_armed_regression`) locally or note
  it for CI.

## Risks

- **Step 3 changes the resolution base** for matched out-of-root `.tsx` from the
  file's physical package to the site's `node_modules`. That matches esbuild
  (which only ever sees the shadow copy), so a dep that only exists beside the
  sibling package was already unresolvable at bundle time; but a project that
  accidentally depended on the physical-path staging under `bundle.exclude`
  would now surface an esbuild resolution error instead of silently staging an
  unused copy. Separable, see spec step 3.
- **Include-glob semantics must match `materialise_collection` byte for byte**
  (`matches_relative` on the POSIX-relative path from the collection root). Test
  (c) and reusing the same `CollectionFilter` construction guard this.
- The unnormalised-root quirk (finding 3) is currently load-bearing for the
  flip; normalising it is what makes the seeds honest. Do not "fix" it in
  isolation without step 3's logical root, or out-of-root matched files fall to
  the synthetic importer and lose their site-level resolution.
- Stage-escape audit: no live-tree escape is reopened — the change only removes
  seeds for files that never enter the shadow (l-lessons-client-bundling).

## Rejected / deferred

- (c) B1 alone: numbers above; the flip and drain stay.
- MDX-import following (default spec): nothing to follow; would add an MDX
  compile per entry to the seed path for zero seeds.
- (b) contingencies — deferred, not part of #3142:
  - Skip `.d.ts` / `test-utils` / `debug` subtrees in `scan_physical_package`:
    would cut the inherent post-flip drain (the +600 ms here is mostly `preact`'s
    `.d.ts`/variant files), but which dirs are non-runtime is package-specific
    and `.d.ts` can be imported explicitly. Only worth it if B3's in-root-import
    measurement (ws4 shape) at real scale shows the once-per-package scan is
    still unacceptable.
  - Persistent physical-scan cache keyed by dir+mtime: helps dev restarts only;
    #3133 is a cold-boot / CI problem.
- "Walk only `exports`-reachable files": reimplements esbuild's resolver
  (rejected in plan.md).

## Pre-existing bugs noticed, not fixed

- Finding 3 above (collection root joined unnormalised → resolution base depends
  on relative vs absolute `path` spelling). Folded into #3142 step 1/3.
- `workspace_package_source_is_eligible` refuses a package whose importer
  declares it `workspace:*` when the installed target is not a member of THIS
  workspace (the vendored runtime's `@takazudo/zfb` peer in the fixture). Correct
  for real installs; only a fixture artefact here.
