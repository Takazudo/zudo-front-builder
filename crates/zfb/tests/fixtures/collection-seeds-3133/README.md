# collection-seeds-3133

Repro fixture for [#3133](https://github.com/Takazudo/zudo-front-builder/issues/3133)
("Out-of-root content collection seeds whole source dir, triggering full
node_modules staging walk"), authored for sub-issue
[#3138](https://github.com/Takazudo/zudo-front-builder/issues/3138) (epic
#3135, Track B0) and made faithful in
[#3141](https://github.com/Takazudo/zudo-front-builder/issues/3141) (Track D).
Driven by `crates/zfb/tests/collection_seed_3133_baseline.rs`.

## Layout

```
workspace/                                  # pnpm-workspace-shaped, copied into a
  pnpm-workspace.yaml                       # tempdir per test run (never mutated
  apps/site/                                # in place)
    package.json                            # deps: preact, @takazudo/zfb{,-runtime},
                                            #   shared-utils (workspace), ui (workspace)
    tsconfig.json                           # a non-empty `paths` map — LOAD-BEARING,
                                            #   see "Why the site needs tsconfig paths"
    zfb.config.with-collection.json         # -> renamed to zfb.config.json for
    zfb.config.control.json                 #    the "with-collection" / "control"
    pages/index.tsx                         #    variant respectively; the OTHER
    layouts/default.tsx                     #    one is deleted after the copy
  packages/ui/
    package.json                            # deps: shared-utils (workspace), leftpad-priv
    src/components/button/button.tsx        # imports "leftpad-priv" + shared-utils
    src/components/button/button.mdx        # imports ./button.tsx (real zudo-sg shape)
    src/components/orphan/orphan.tsx        # NO .mdx imports this; imports shared-utils
  packages/shared-utils/                    # workspace pkg; deps: shared-icons, leftpad-priv
    package.json
    index.js
  packages/shared-icons/                    # workspace pkg; deps: leftpad-priv
    package.json
    index.js

pnpm-store-template/leftpad-priv/           # the ONE physical pnpm-private package;
  package.json                              # copied into a synthetic
  index.js                                  # node_modules/.pnpm/leftpad-priv@1.0.0/... at
                                             # test setup time (see below)
```

`apps/site/zfb.config.with-collection.json` declares:

```json
{
  "framework": "preact",
  "collections": [
    {
      "name": "componentDocs",
      "path": "../../packages/ui/src/components",
      "include": ["**/*.mdx"],
      "allowOutsideRoot": true
    }
  ]
}
```

— the exact shape from the issue. `zfb.config.control.json` is the same app
with no `collections` key at all.

## node_modules is NOT committed

Per the repo's `.gitignore`, `node_modules/` is ignored everywhere except a
short explicit allowlist this fixture deliberately does not join. The
synthetic pnpm-store / workspace-symlink tree is built at **test setup
time** instead (`materialise_fixture` in `collection_seed_3133_baseline.rs`),
copied fresh into a tempdir on every run:

- `node_modules/.pnpm/leftpad-priv@1.0.0/node_modules/leftpad-priv/` — the
  one physical copy of `leftpad-priv`, copied from
  `pnpm-store-template/leftpad-priv/`.
- `packages/ui/node_modules/leftpad-priv` — symlink to the physical dir
  above (**logical path 1**).
- `packages/ui/node_modules/shared-utils` — symlink to `packages/shared-utils`
  (pnpm workspace-protocol link).
- `packages/shared-utils/node_modules/leftpad-priv` — symlink to the
  physical dir (**logical path 2**).
- `packages/shared-utils/node_modules/shared-icons` — symlink to
  `packages/shared-icons` (workspace link).
- `packages/shared-icons/node_modules/leftpad-priv` — symlink to the
  physical dir (**logical path 3**).
- `node_modules/.pnpm/@takazudo+zfb-runtime@embedded/node_modules/` —
  `@takazudo/zfb-runtime` and `@takazudo/zfb` **copied** from the zfb
  binary's own vendor snapshot (`ZFB_VENDOR_DIR`, exported by
  `crates/zfb/build.rs`; `package.json` + `src` only), plus a `hono` symlink
  to this monorepo's installed `node_modules/.pnpm/hono@*/…` copy — pnpm's
  `.pnpm/<pkg>@<ver>/node_modules/{<pkg>,<deps…>}` sibling layout.
- `apps/site/node_modules/` — what `pnpm install` would give the site:
  `preact` and `preact-render-to-string` (symlinks to this monorepo's
  installed `.pnpm` copies, found by prefix), `@takazudo/zfb` and
  `@takazudo/zfb-runtime` (symlinks to the store copies above), and the
  `shared-utils` / `ui` workspace links `apps/site/package.json` declares.

Three distinct _logical_ `node_modules/leftpad-priv` paths resolving to one
_physical_ directory is the exact shape `extend_node_modules_dependency_staging`
(`crates/zfb-build/src/bundler.rs`) visits once per logical path (the
`visited` set is keyed on the logical root) and — since #3139 — import-scans
once per physical directory.

### Why the runtime is copied into the fixture's own store

After workspace staging flips on, `<shadow>/node_modules` is no longer a
live link but an isolated view of **staged real copies**, and
`workspace_package_source_is_eligible` only stages an installed package
whose canonical path lies inside a `node_modules` (or is a claimed member
of _this_ workspace). A symlink straight to this repo's
`packages/zfb-runtime` (the trick `client_bundling_cross_pipeline.rs`
uses in a root-claimed workspace, where eligibility short-circuits) would
canonicalise outside every `node_modules`, be rejected, and make the
post-flip esbuild pass fail on `@takazudo/zfb-runtime/server`. Copying the
vendored package into a `.pnpm/…/node_modules/@takazudo/zfb-runtime` dir
makes it look exactly like the registry install a real consumer has.

### Why the site needs a real `node_modules` AND `tsconfig.json` paths

Diagnosed in #3141 — the first cut of this fixture (no
`apps/site/node_modules`, no `tsconfig.json`) built fine but never flipped
workspace staging (`physical_scans=1 logical_visits=1
workspace_staging_activated=false`), for two independent reasons:

1. With no project-local `node_modules`, `commands::bundler_input` uses the
   binary-embedded vendor tree and sets `node_modules_preserve_symlinks =
true`. In `extend_node_modules_dependency_staging` that disables both
   `resolve_from_canonical_package` and `allow_workspace_physical_fallback`,
   so the closure walk can never reach a workspace package's pnpm-private
   siblings (`shared-utils` was scanned once; its `leftpad-priv` /
   `shared-icons` imports were simply unresolved).
2. With a project-local `node_modules` but an **empty** tsconfig `paths`
   map, `esbuild_will_preserve_symlinks` (bundler.rs) is true, so those
   siblings are only **deferred** (`deferred_physical_dependencies`) until
   staging flips — and it never does: the seeded workspace package lands in
   `staging_dirs` as `<ws>/packages/ui/node_modules/shared-utils`, which is
   **outside `project_root`**, and the flip predicate only inspects
   `staging_dirs` entries under `project_root` (plus every
   `staging_alias_dirs` value).

A non-empty `paths` map — the same gate `bundler_staging_scan_memo.rs`
(#3139) documents — makes the walk resolve through canonical package
dirs, which follows the store's private siblings immediately and records
the workspace-package aliases
(`…/shared-utils/node_modules/shared-icons -> packages/shared-icons`) in
`staging_alias_dirs`, where the flip predicate _does_ see them.

A second, independent real-world trigger also flips staging: any in-root
source importing a workspace package (e.g. `pages/index.tsx` importing
`shared-utils`). Measured in #3141 on this fixture without the tsconfig:
with-collection `physical_scans=5 logical_visits=10
workspace_staging_activated=true` (the collection's whole closure is
walked once the in-root import has flipped staging). It is not the shape
this fixture uses because it would flip the `control` variant too; the
tsconfig gate keeps the control at zero so the delta isolates the
collection.

Also learned there: `resolver.resolve(&collection.root)` joins the
collection path onto the project root **unnormalised**
(`<ws>/apps/site/../../packages/ui/src/components`), so the out-of-root
seeds pass `seed.starts_with(project_root)` lexically, become their own
logical importers, and resolve their bare imports by walking up through
the `..` into `packages/ui/node_modules/`. That is the only reason an
out-of-root seed reaches a workspace package at all — with an absolute
collection path the seeds would fall back to the synthetic
`<project>/entry` importer and resolve from `apps/site/node_modules`
instead.

## Measured stats (merged #3138 + #3139 base, #3141, 2026-09-25)

Debug `cargo build -p zfb` (embedded V8 + esbuild 0.25.12 + Tailwind v4.2.0
staged via `ZFB_ESBUILD_BIN`/`ZFB_TAILWIND_BIN`), 3 runs each, real
`zfb build` against the tempdir tree above — the numbers the harness now
**asserts**:

| Metric                              | with-collection             | control                     |
| ----------------------------------- | --------------------------- | --------------------------- |
| `[zfb-staging-stats]`               | `physical_scans=7 logical_visits=9 workspace_staging_activated=true` | `physical_scans=0 logical_visits=0 workspace_staging_activated=false` |
| same, with #3139's memo reverted    | `physical_scans=9 logical_visits=9 …=true` | unchanged                   |
| `zfb build` wall time (3 runs)      | 3582 / 3512 / 3709 ms       | 2822 / 3012 / 2939 ms       |
| `[zfb-timing] bundle(): materialise=` | ~12 ms                    | ~10 ms                      |
| Peak RSS                            | not measured — no `/usr/bin/time` in this sandbox | —      |

`with-collection`'s 9 logical visits, in closure order:
`packages/ui/node_modules/shared-utils` (seeded by both `button.tsx` and
`orphan.tsx`), its `leftpad-priv` and `shared-icons` aliases,
`shared-icons`' own `leftpad-priv` alias → **staging flips** on the
workspace aliases → the deferred live dependencies drain:
`@takazudo/zfb-runtime`, its `hono` sibling, `preact`,
`preact-render-to-string`, and `packages/ui/node_modules/leftpad-priv`
(button.tsx's own, deferred as an ordinary dep). The 7 physical scans are
one per distinct canonical dir — `leftpad-priv`'s three logical paths
share one scan (9 scans with the memo reverted; the two saved scans are a
two-file package here, but in #3133's tree every pnpm-private dep with N
importers is scanned N times without it).

The ~600 ms (+20 %) wall-clock gap is the post-flip drain: `preact`'s
whole install (every `.js`/`.mjs`/`.d.ts` under `compat/`, `hooks/`,
`debug/`, `test-utils/`, …) plus `hono`, the runtime and
`preact-render-to-string` are walked and import-parsed by swc. That is
#3133's "opening files one at a time under `node_modules/.pnpm/**`" at toy
scale; the issue's tree (codemirror, several workspace packages) is what
turns it into minutes.

## What the collection actually contributes to the esbuild graph: nothing

The kept shadow's `.zfb-metafile.json` (with-collection variant) lists
`content/componentDocs/button/button.mdx` as an input — and **neither
`button.tsx`, `shared-utils`, `shared-icons` nor `leftpad-priv`**. Two
mechanisms, both by design:

- `materialise_collection` (bundler.rs, "Apply include / exclude globs to
  recognised content extensions") counts `.tsx` as a content extension, so
  a `.tsx` the `include` glob does not match is **not copied into the
  shadow at all** (`<shadow>/apps/site/content/componentDocs/orphan/` is
  an empty dir).
- zfb's MDX compiler (`crates/zfb-content/src/mdx_jsx_emit.rs`,
  `mdx_to_jsx_module_inner`) parses `import Button from "./button.tsx"`
  as an `MdxjsEsm` node "so the emitter can silently drop" it — the
  compiled module reads
  `const Button = _components.Button ?? components.Button; if (!Button)
throw new Error("MDX requires \`Button\` to be passed via the
\`components\` prop")`. Components reach MDX through the `components`
  prop / `mdx-components.tsx` (#616), never through MDX-level imports.

So under `include: ["**/*.mdx"]` every seed the whole-root walk adds
(`button.tsx` **and** `orphan.tsx` alike), every package it stages and
every post-flip scan is work esbuild can never use. That is the finding
#3141 locks into #3142's spec: seed a collection root by the same filter
the shadow materialisation applies, and do not chase MDX imports — there
are none to chase.

To reproduce by hand, mirror `materialise_fixture` — the `.pnpm` copies
and the site's `node_modules` links are what the earlier hand recipe in
this README lacked.
