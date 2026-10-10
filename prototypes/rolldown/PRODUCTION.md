# Bounded production follow-up for #318

The real production path works for this mixed SSG/SSR fixture with both esbuild and native Rolldown. Keep the prototype opt-in and the PR draft. This supports continued ownership evaluation, not adoption or deployment. The copied-glue URL contract is **incomplete for SSR in the baseline too**; see the reproduction below.

## Exact path and execution

`crates/zfb/examples/rolldown_production.rs` copies `production-fixture` into an owned ignored directory, extracts the binary's real embedded SDK, and links the existing workspace Cloudflare adapter. It runs the actual `zfb build` CLI. No deployment command runs. Production first renders the full SSG bundle and then uses the existing `worker_only_routes` path to create `bundle-runtime.mjs` for the adapter. The checked-in adapter writes its normal `_zfb_inner.mjs` and Wasm files.

The probe executes **both** scratch `bundle-runtime.mjs` and emitted `dist/_zfb_inner.mjs` with the existing `EmbeddedV8RenderHost` and unchanged module loader:

| Contract | Observed result, both backends |
| --- | --- |
| SSG `/` | Actual static HTML contains `SSG_ONLY_318:17`; the runtime fetch returns 404 |
| SSR `/runtime` | 200 HTML contains consumed Wasm 29, CJS 31, MDX bold text, actual CSS Module class and emitted rule, ordinary glue module export 42 |
| Deferred `/api/admin/[...adminPath]` | `/a/b` and `/c` return distinct captured paths, consumed Wasm 43 and CJS 31 |
| Filtering | Runtime graph, code and emitted assets omit the SSG route and Wasm 17; full graph contains them |
| Assets | Runtime and catch-all Wasm have actual graph/output source provenance; adapter emits only29/43 and records them in `.assetsignore`; V8 executes them |
| Original diagnostics | Actual scratch throw maps through its linked map to `pages/throw.tsx:3:9` |
| Adapter maps | Existing adapter strips the external map URL; no deployed-inner original-map claim |
| Policies | Real main-only CJS package plus configured `mainFields`, tsconfig alias, MDX/CSS preparation and Wasm copy imports; no general CJS/external parity claim |
| Negatives | Real CLI exclusion, unresolved import and root-workspace live-source escape all reject during the **initial full pass**; second pass is never reached |
| Determinism | Two complete builds per backend compare actual runtime and adapter-inner bytes |

The graph is the bundler's actual retained `.zfb-metafile.json`, selected by the output named `bundle-runtime.mjs`. Existing `ZFB_KEEP_BUILD_SHADOW` retains fixture-owned temporary stages; no product graph hook, audit exception or alternative resolver was added. Positive graphs and responses are saved before negative runs. Cross-backend regular and catch-all responses are compared; identical emitted engine bytes are not required.

## Run it

Use the tool setup in [README.md](README.md), including pinned esbuild. `pnpm` must be on PATH for the existing local adapter invocation (recorded version 12.8.2). On a fresh checkout, install the repository lockfile first: CLI `build.rs` embeds Hono 4.12.25 from `node_modules/.pnpm`, plus current SDK 4.3.0 source/package metadata. No SDK release pack is needed for this probe. This differs from the original direct fixture’s separately installed Hono 4.12.8. Compilation is outside runtime measurements:

```sh
pnpm install --frozen-lockfile
bash "$HOME/.codex/scripts/heavy-guard.sh" -- cargo build --locked -p zfb --features rolldown-prototype --bin zfb --example rolldown_production
bash "$HOME/.codex/scripts/heavy-guard.sh" -- node prototypes/rolldown/production.mjs
```

The wrapper explicitly unsets native selection for the esbuild leg and sets `ZFB_ROLLDOWN_PROTOTYPE=1` only for the native leg. Logs, both actual graphs, and response reports live in `target/rolldown-production/{esbuild,rolldown}/evidence`. The harness only replaces its owned directories. Manual rollback is simply unset the environment variable, or rebuild without `rolldown-prototype`; a native error has no fallback to esbuild.

`prepare.mjs` now refreshes its owned SDK copies on every run. The earlier 8e029dd rerun did invoke preparation, but its old copy-if-absent behavior left package metadata at 4.2.0 (the relevant JS was unchanged). The follow-up checks actually use refreshed 4.3.0 copies and freshly embedded CLI SDK. This corrects the prior freshness claim.

## Concrete copied-glue limitation

The positive SSR route imports `answer` from `runtime-glue.zfb-resource.mjs` as ordinary JavaScript and executes 42. It does **not** claim copied-resource behavior.

The same harness then changes that import to a used default URL import and runs the real CLI again. Both backends fail in the initial bundle:

- esbuild: `No matching export ... for import "default"`.
- Rolldown: `[MISSING_EXPORT] "default" is not exported ...`.

See each `limitation-ssr-glue-url.log`; the checked-in fixture and the harness's small mutation are the minimal reproduction. The Worker SSR prepared loaders do not install `.zfb-resource.mjs=file`; that compound loader belongs to the islands path. Public `bundle.loaders` intentionally rejects asset-emitting `file`/`copy` overrides. Thus this run never reaches an adapter copied-glue handoff. We did not widen host disk authority or alter adapter/staging policy. Browser opaque glue copying/execution remains covered separately by the original Chromium fixture. Any future SSR resource contract needs a separate bounded design and baseline acceptance decision.

## Private compatibility ownership

`native/prepared.rs` isolates the existing strict argv decoder; `native/copy_reference.rs` isolates the upstream-private copy-edge prefix and provenance restoration. The initial extraction was mechanical, not a new neutral API or an elimination of coupling. Unknown prepared flags remain errors; copy edges require one authoritative emitted source.

At the pin, `ModuleInfo` has no typed copied-asset relationship. Builtin `CopyModulePlugin` resolves, emits a file and exposes an external private-prefix reference, then rewrites it in its render hook. Replacing this losslessly today would mean owning those copy/render/source-map semantics. A small upstream typed graph API is preferable to a local fork or broad replacement. The selected real Wasm fixture tests the isolated compatibility module. Stronger review assertions found a local metadata defect: importer-to-Wasm edges were correct, but orphan private input nodes and private IDs in chunk output imports remained. The adapter now uses the captured `ctx.get_file_name` mapping for those output imports, replaces pseudo-nodes with actual emitted source provenance, and rejects absent/ambiguous provenance. The fixture asserts actual importer edges and no residual private IDs. Both revisions were rerun after this correction; this was a local adapter fix, not upstream API churn.

## Adjacent revision rehearsal

The inspected upstream head was 24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9; no newer revision existed. The bounded exercise therefore uses its direct parent fd62a5427196b8ecab82e0f24543560912e1e1ba and upgrades back to 24bc2d0. This is an actual adjacent historical transition, not evidence of compatibility with future changes. Both identify as 1.2.13. The upstream delta is 23 files, 347 insertions/47 deletions concerning namespace-member write diagnostics, including internal scanner/linker and common member-reference types. No API consumed by this adapter changed; upstream Cargo manifests did not change.

The parent pin compiled without any upstream-driven adapter API edits (35 seconds guarded) and passed the same browser/direct SSR/real production matrix (14 seconds guarded). The stronger graph correction then required an 11-second rebuild and a passing 14-second repeat of the same matrix. The initial current-pin matrix passed in 15 seconds. Cargo updated 24 Git source entries and reselected 9 Windows-only edges to already-locked `windows-sys 0.61.2`; no package versions/checksums changed. Those incidental edges are not part of this Linux runtime result. Upgrading back restores the original manifest, backend evidence marker and complete lockfile byte-for-byte; no final dependency delta remains. The exact final repository SHA and final matrix/CI results are recorded in the PR evidence ledger. The compatibility-module relocation was local ownership work, not a required upstream adaptation.

The subsequent authorized release-profile comparison and stronger fail-closed copy boundary are documented in [RELEASE-MEASUREMENT.md](RELEASE-MEASUREMENT.md). Original dev-profile samples remain historical evidence and cannot support speed claims.

## Progression decision

**Go:** retain the explicit opt-in and broaden to one named real consumer only while this runtime/graph/audit matrix and normal-path lock/size contracts remain satisfied. Treat any actual regression as a stop for that affected path. Keep SSR copied-resource work separate until its baseline contract is accepted.

**No-go for a default switch:** supported-platform and caller coverage, sustainable ownership of private/API changes, and broader controlled release measurements are still missing. A historical adjacent pass and this dev-profile microfixture do not fill those gaps. Config/plugin/client-script migration, broader external/CJS policies, and performance outside the selected Linux fixtures remain unverified. Keep the draft PR and issue318 open.
