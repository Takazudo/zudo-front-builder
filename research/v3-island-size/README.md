# V3 island size measurement (#3468)

This directory freezes eight small consumer recipes and two measurement paths. `measure-real.mjs` invokes the actual `zfb build` binary against temporary consumer projects and inventories **every emitted JS file** in `dist`, including the hashed islands entry, shared chunks, and lazy chunks. Each pass saves the complete `dist` tree, not only HTML and JavaScript, so the guard can compare the recorded inventory with the bytes on disk. The `no-island` build is real and must emit no JavaScript. The workspace and packed runs each execute that same CLI against a consumer whose `node_modules/@takazudo/zfb` and `node_modules/@takazudo/zfb-runtime` point to matching workspace or packed packages; `hono` points to the exact lockfile-installed 4.12.25 dependency. `measure.mjs` independently reproduces the shared-islands esbuild flag set and generated registration semantics with a modeled entry to retain esbuild JSON metafiles and input-byte attribution. Its byte totals are *sidecar diagnostics*, not the shipped-size baseline. Neither script reads, copies, changes, or weakens the bundler's private resource metafile or stage-escape audit.

The recipes are frozen in `fixtures/`: no island; event-only button; scalar signal; Show/For; modelValue binding; a blog menu plus theme toggle; JSON API fetch; and a scalar plus theme-toggle multi-island page. They are controlled current-source recipes, not exact historical #3383 inputs. Do not label their values as reproduced v2/v3 numbers. The multi-island comparison reports the difference from scalar alone, while each page's shipped total deduplicates chunks by filename.

## Pinned run after the topic merges

Run on the merged source SHA, with one guarded heavyweight command at a time. The pinned esbuild binary comes from the repo's toolchain; the runner verifies version `0.25.12` and records its digest. Use `CARGO_TARGET_DIR` shared with the manager's existing build. `zfb` must be freshly built from the SHA being measured. Install from `pnpm-lock.yaml` with `--frozen-lockfile` if dependencies are absent; avoid changing the lockfile.

```sh
export CARGO_TARGET_DIR=/Users/takazudo/repos/myoss/zfb/target
export CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1
export ZFB_SIZE_SHA="$(git rev-parse HEAD)"
export ZFB_SIZE_ESBUILD=/Users/takazudo/repos/myoss/zfb/crates/zfb/binaries/esbuild/esbuild
export ZFB_SIZE_BIN="$CARGO_TARGET_DIR/debug/zfb"
export ZFB_SIZE_HONO="$PWD/packages/zfb-runtime/node_modules/hono"
export ZFB_SIZE_OUT="$PWD/research/v3-island-size/results"
mkdir -p "$ZFB_SIZE_OUT/pack" "$ZFB_SIZE_OUT/packed-package" "$ZFB_SIZE_OUT/packed-runtime"
bash "$HOME/.codex/scripts/heavy-guard.sh" -- cargo build -p zfb
pnpm -C packages/zfb build
pnpm -C packages/zfb-runtime build
pnpm -C packages/zfb pack --pack-destination "$ZFB_SIZE_OUT/pack"
pnpm -C packages/zfb-runtime pack --pack-destination "$ZFB_SIZE_OUT/pack"
tar -xzf "$ZFB_SIZE_OUT/pack/takazudo-zfb-3.0.0.tgz" -C "$ZFB_SIZE_OUT/packed-package"
tar -xzf "$ZFB_SIZE_OUT/pack/takazudo-zfb-runtime-3.0.0.tgz" -C "$ZFB_SIZE_OUT/packed-runtime"
bash "$HOME/.codex/scripts/heavy-guard.sh" -- node research/v3-island-size/measure-real.mjs --case event-only --mode workspace --package "$PWD/packages/zfb" --runtime-package "$PWD/packages/zfb-runtime" --hono-package "$ZFB_SIZE_HONO" --zfb-tarball "$ZFB_SIZE_OUT/pack/takazudo-zfb-3.0.0.tgz" --runtime-tarball "$ZFB_SIZE_OUT/pack/takazudo-zfb-runtime-3.0.0.tgz" --zfb "$ZFB_SIZE_BIN" --esbuild "$ZFB_SIZE_ESBUILD" --source-sha "$ZFB_SIZE_SHA" --out "$ZFB_SIZE_OUT/smoke-event"
bash "$HOME/.codex/scripts/heavy-guard.sh" -- node research/v3-island-size/measure-real.mjs --mode workspace --package "$PWD/packages/zfb" --runtime-package "$PWD/packages/zfb-runtime" --hono-package "$ZFB_SIZE_HONO" --zfb-tarball "$ZFB_SIZE_OUT/pack/takazudo-zfb-3.0.0.tgz" --runtime-tarball "$ZFB_SIZE_OUT/pack/takazudo-zfb-runtime-3.0.0.tgz" --zfb "$ZFB_SIZE_BIN" --esbuild "$ZFB_SIZE_ESBUILD" --source-sha "$ZFB_SIZE_SHA" --out "$ZFB_SIZE_OUT/real-workspace"
bash "$HOME/.codex/scripts/heavy-guard.sh" -- node research/v3-island-size/measure.mjs --mode workspace --package "$PWD/packages/zfb" --esbuild "$ZFB_SIZE_ESBUILD" --source-sha "$ZFB_SIZE_SHA" --out "$ZFB_SIZE_OUT/modeled-workspace"
bash "$HOME/.codex/scripts/heavy-guard.sh" -- node research/v3-island-size/measure-real.mjs --mode packed --package "$ZFB_SIZE_OUT/packed-package/package" --runtime-package "$ZFB_SIZE_OUT/packed-runtime/package" --hono-package "$ZFB_SIZE_HONO" --zfb-tarball "$ZFB_SIZE_OUT/pack/takazudo-zfb-3.0.0.tgz" --runtime-tarball "$ZFB_SIZE_OUT/pack/takazudo-zfb-runtime-3.0.0.tgz" --zfb "$ZFB_SIZE_BIN" --esbuild "$ZFB_SIZE_ESBUILD" --source-sha "$ZFB_SIZE_SHA" --out "$ZFB_SIZE_OUT/real-packed"
bash "$HOME/.codex/scripts/heavy-guard.sh" -- node research/v3-island-size/measure.mjs --mode packed --package "$ZFB_SIZE_OUT/packed-package/package" --esbuild "$ZFB_SIZE_ESBUILD" --source-sha "$ZFB_SIZE_SHA" --out "$ZFB_SIZE_OUT/modeled-packed"
```

Check the tarball basenames produced by `pnpm pack` before extraction if the package version changed. Record both SHA-256 digests and the installed Hono package version/digest in the final report. The packs must contain `dist/zudo-react/client.js` and `dist/server.js`, proving the packed path uses built output. The real run saves `report.md`, `measurement.json`, and the complete `dist` tree for both passes under `real-{workspace,packed}/`. The modeled run saves the same report formats plus raw `metafile.json` under each nonempty case and pass. Both runners fail if any repeated result differs. The real result compares the **entire dist inventory**, not just JS. All outputs live in ignored `results/`; commit the final measured `REPORT.md` with the source SHA, tarball digest, four report paths, actual totals, sidecar attribution, any differences, and limitations.

After both complete real runs, enforce the committed contract:

```sh
node research/v3-island-size/check-budget.mjs --workspace "$ZFB_SIZE_OUT/real-workspace/measurement.json" --packed "$ZFB_SIZE_OUT/real-packed/measurement.json" --contract research/v3-island-size/decision.json
```

The guard requires the full eight-case matrix in both modes, the requested checkout SHA, the pinned versions and fixture/runner/lockfile hashes, two identical saved passes, and matching full-`dist` inventories. It recomputes each emitted JavaScript file's SHA-256, raw length, and gzip-9 length from the saved bytes, then sums all entry, shared, and lazy chunks. A `--case` smoke run is useful for diagnosis but cannot satisfy the full gate.

The `--case event-only` smoke executes only two real consumer builds before the full matrix. It has a separate output directory and reports no multi-island delta. Any of the eight case names may be selected this way.

## Reading the measurements

`initial` is the entry and its static-import closure plus any HTML-linked JS; `later` is every other emitted JS file, including lazy imports. `all shipped` adds every emitted JS file once, so dynamic splitting cannot reduce the reported total by hiding bytes. Raw is file byte length. Gzip is Node `zlib.gzipSync` level 9, `mtime: 0`, applied to each file before summing; HTTP transfer compression may differ. The JSON reports retain every chunk hash, raw/gzip size, phase, and all-dist reproducibility inventory. The modeled metafiles preserve each input's `bytes` and each output's `bytesInOutput`; those fields describe reachability and approximate contribution after tree shaking, not independent feature savings. Inspect `structure.ts`, `forms.ts`, `diagnostics`/reporting modules, and runtime inputs across the event/signal/Show-For/model cases before proposing a carve-out. A single scalar case with `--keep-names` disabled probes the global flag's whole-bundle cost; production always keeps it enabled.

The sidecar copies the production namespace import, named/default export fallback, function and identity checks, module label, registration closure, and `mountIslands` call. It still uses a fixed valid 16-character build identity instead of the real build hash; imports fixture components through relative temporary paths instead of production's absolute staged/shadow paths; and does not run the CLI's staging, resource read-back, or final entry-hashing steps. Its module labels use the fixture project's `components/<name>.tsx` spelling, matching the CLI's project-relative label for these fixtures. The sidecar excludes client-router activation because no fixture uses it. These differences can affect exact bytes and path metadata, so compare retained modules and input contributions as diagnostic evidence only. The real CLI output remains the size authority.

`measure-real.mjs` records measurements but does not enforce thresholds. `check-budget.mjs` enforces the per-fixture raw and gzip ceilings in `decision.json`; its allowance is zero bytes. This is baseline protection, not an optimization result.

The recorded baseline was measured on Darwin arm64. Linux equivalence has not yet been validated. Before relying on the CI gate, replay the unchanged baseline twice on Linux with the pinned Node, zlib, esbuild, Hono, and package versions. Compare both passes for each fixture, workspace and packed resolution, emitted totals, and retained modules. If the results differ, keep the gate from enforcing these ceilings until an independently reviewed Linux baseline records the measured difference; do not add speculative headroom.
