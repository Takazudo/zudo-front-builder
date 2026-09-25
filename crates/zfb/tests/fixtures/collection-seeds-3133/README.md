# collection-seeds-3133

Repro fixture for [#3133](https://github.com/Takazudo/zudo-front-builder/issues/3133)
("Out-of-root content collection seeds whole source dir, triggering full
node_modules staging walk"), authored for sub-issue
[#3138](https://github.com/Takazudo/zudo-front-builder/issues/3138) (epic
#3135, Track B0). Driven by
`crates/zfb/tests/collection_seed_3133_baseline.rs`.

## Layout

```
workspace/                                  # pnpm-workspace-shaped, copied into a
  pnpm-workspace.yaml                       # tempdir per test run (never mutated
  apps/site/                                # in place)
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

Three distinct *logical* `node_modules/leftpad-priv` paths resolving to one
*physical* directory is the exact shape `extend_node_modules_dependency_staging`
(`crates/zfb-build/src/bundler.rs`) re-scans once per logical path today
(the `visited` set is keyed on the logical root) — see the code-map at
`_temp-resource/3135-provenance-guard-and-collection-seeds/code-map.md`.

To reproduce this tree by hand (e.g. to poke at it outside the test):

```sh
cp -r crates/zfb/tests/fixtures/collection-seeds-3133/workspace /tmp/repro
cd /tmp/repro
mv apps/site/zfb.config.with-collection.json apps/site/zfb.config.json
rm apps/site/zfb.config.control.json
STORE=node_modules/.pnpm/leftpad-priv@1.0.0/node_modules/leftpad-priv
mkdir -p "$STORE"
cp ../../pnpm-store-template/leftpad-priv/* "$STORE/"   # adjust the relative path
mkdir -p packages/{ui,shared-utils,shared-icons}/node_modules
ln -s "$PWD/$STORE" packages/ui/node_modules/leftpad-priv
ln -s "$PWD/packages/shared-utils" packages/ui/node_modules/shared-utils
ln -s "$PWD/$STORE" packages/shared-utils/node_modules/leftpad-priv
ln -s "$PWD/packages/shared-icons" packages/shared-utils/node_modules/shared-icons
ln -s "$PWD/$STORE" packages/shared-icons/node_modules/leftpad-priv
```

## Baseline (pre-fix), recorded 2026-09-25 at commit `707485e` + this branch

Measured with a debug `cargo build -p zfb` (embedded V8 + esbuild 0.25.12 +
Tailwind v4.2.0, staged locally via `ZFB_ESBUILD_BIN`/`ZFB_TAILWIND_BIN`
because this sandbox has no direct route to `registry.npmjs.org`/GitHub
Releases for the build script's own download step — CI stages these
normally). 3 runs each, real `zfb build` / `zfb dev --port 0` against the
tempdir tree above:

| Metric | with-collection | control |
|---|---|---|
| `zfb build` wall time (3 runs) | 3326 / 3343 / 3438 ms | 3261 / 3132 / 3246 ms |
| `zfb dev --port 0` time-to-ready (`ZFB_DEV_TIMING=1`, 1 run) | 3842 ms | 3543 ms |
| `[zfb-timing] bundle(): materialise=` | 14ms | 37ms |
| Peak RSS | not measured — no `/usr/bin/time` in this sandbox | — |
| `[zfb-staging-stats]` line | absent (#3139's hook not on this branch yet) | absent |

**No measurable gap at this fixture's scale.** Both variants cost ~3.1-3.8s,
dominated by the embedded-V8/esbuild process startup and the debug build's
extraction of the embedded framework packages — NOT by the staging walk
(`materialise=14-37ms` either way). This is expected and does not contradict
#3133: the issue's real-world tree has a `node_modules/.pnpm` closure with
tens of thousands of files (codemirror, preact, several workspace
packages); this fixture's `leftpad-priv` is two files. The fixture proves
the *structural* repro shape (out-of-root collection + MDX-adjacent
sibling `.tsx` + an orphan `.tsx` + a >= 3×-logical-path pnpm-private dep)
that B1's memoization and B2's seed-scoping fix are meant to short-circuit;
it is not, by itself, sized to demonstrate the multi-minute hang. A future
wave (B3, #3143) may want a second, larger fixture (or a generated
synthetic `node_modules` with thousands of files) specifically to measure
wall-clock/RSS at a scale where the bug is visible — out of scope for B0.

Once #3139 (Track B1, `bundler.rs`) merges, re-run
`cargo test -p zfb --test collection_seed_3133_baseline -- --nocapture`
(with `ZFB_ESBUILD_BIN`/`ZFB_TAILWIND_BIN` set as above, or an embedded
release build) to capture the `[zfb-staging-stats]` line — expect
`logical_visits=3`, `physical_scans=1` after memoization for
with-collection, `workspace_staging_activated=false` for `control`.
