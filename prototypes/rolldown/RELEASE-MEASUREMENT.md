# Bounded release measurement and maintenance contract

This follow-up uses the existing semantic fixtures with the unchanged Cargo release profile: opt-level 3, default LTO disabled, default 16 codegen units, and workspace `strip = "symbols"`. The esbuild comparison uses its optimized 0.25.12 executable. Neither backend gets a persistent compiler service or reused bundler instance.

## Reproduce

Use the prerequisites in [README.md](README.md) and [PRODUCTION.md](PRODUCTION.md), including repository `pnpm install --frozen-lockfile`, pinned platform esbuild, Hono and Chromium. Keep `pnpm` on PATH for the local adapter. This does not publish or deploy.

```sh
node prototypes/rolldown/prepare.mjs
export CARGO_BUILD_JOBS=4
bash "$HOME/.codex/scripts/heavy-guard.sh" -- cargo build --release --locked \
  -p zfb -p zfb-build -p zfb-islands -p zfb-rolldown-prototype \
  --features zfb/rolldown-prototype,zfb-build/rolldown-prototype,zfb-islands/rolldown-prototype,zfb-rolldown-prototype/native \
  --bin zfb --example rolldown_production --example rolldown_ssr --example rolldown_browser --example prepared
bash "$HOME/.codex/scripts/heavy-guard.sh" -- cargo test --release --locked -p zfb-rolldown-prototype --features native --lib
bash "$HOME/.codex/scripts/heavy-guard.sh" -- node prototypes/rolldown/benchmark.mjs
ZFB_PROTOTYPE_PROFILE=release bash "$HOME/.codex/scripts/heavy-guard.sh" -- node prototypes/rolldown/verify.mjs
```

Compilation is separate. The measured producer order is esbuild/Rolldown, then Rolldown/esbuild. Each SSR/browser producer process makes three calls: first, then two calls in the same process with warmed dependencies/filesystem caches. Every native call creates a new bundler and Tokio runtime/thread; every esbuild call launches a new executable. No operating-system cache is flushed. These are two fresh-process observations and four subsequent-call observations per backend, not a statistical benchmark suite or an incremental-build comparison.

Direct SSR uses its existing **development bundle policy**, even though Rust is release-optimized. Its selected pipeline includes preparation, bundling, metadata and teardown. Browser uses its existing production policy, reports the selected main-plus-worker bundler call and separately measures `ProductionAssetPipeline.apply`. Production uses actual `zfb build`: each positive CLI subprocess includes startup, preparation, full SSG bundle/render, second runtime bundle and local adapter. The second CLI invocation retains prior dist output; it is still a fresh CLI process. Semantic execution and negative-case probes occur outside those positive CLI intervals. Whole probe process times are retained for diagnostics and must not be presented as build times.

Native `bundle_us` includes bundler construction/write and graph hooks; `metadata_us` includes compatibility metadata assembly/validation/write. The caller interval additionally includes command decoding and thread/runtime startup. Existing pipeline phase lines use milliseconds. Esbuild has no corresponding internal metadata timer: its command interval includes its own metadata generation. Do not subtract or compare these as identical engine phases. Browser native phases group the main and module-worker jobs in each caller interval. SSR positive phases exclude later negative probes.

The report under ignored `target/rolldown-release-measurement/` contains raw sample arrays, logs, binary hashes, input manifests and source state. The materialised fixture hash includes its top-level copied runtime packages, excluding nested SDK development-install `node_modules` links. Production authored inputs are hashed separately; embedded dependencies are covered by the common CLI binary hash and locked source state. These are source/preparation-input fingerprints, not identical temporary path/argv hashes. Both backends run the same preparation and selected policies. Each sample retains the existing deterministic-output/V8/graph/audit assertions; Chromium executes final browser outputs.

## Private-copy upgrade contract

The compatibility owner must review these together before changing the pin:

1. `crates/rolldown_plugin_copy_module/src/lib.rs` at `24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9`: private `__ROLLDOWN_COPY_MODULE__#` ID, emit-reference and render-hook behavior.
2. `ModuleInfo` imported IDs, `PluginContext::get_file_name`, and emitted asset `original_file_names`: each copy reference must have one nonempty authoritative source and a captured emitted filename. Do not infer source/hash names or independently resolve the graph.
3. The strict prepared-command decoder and selected loader/external/alias policies. A newly needed flag remains an explicit error until reviewed.
4. The focused malformed/missing/ambiguous-provenance tests and the actual Wasm browser/production graph fixture. Unit mocks alone cannot validate a new upstream API. Preserve importer-to-source edges, output imports, maps, source provenance, execution and audits across the pin transition.

The adapter now rejects empty references, malformed/missing temporary markers, unknown variants within the reserved copy namespace, empty/ambiguous sources, and residual private identifiers in final metadata identifier fields. It does not scan arbitrary JavaScript or diagnostic text. A real path containing the reserved `__ROLLDOWN_COPY_MODULE` substring is rejected conservatively. Completely renaming the upstream namespace is not universally detectable; source review and real pinned-engine fixtures remain mandatory. Validation follows `bundler.write()`: an error prevents successful metadata publication/return, but does not guarantee no output files were written. Callers must treat the entire failed operation as unusable; there is no silent fallback.

This strengthens the owned boundary; it does not remove the private dependency, make Rust APIs stable, or replace the existing host/staging audits. A typed upstream copied-edge API remains preferable. Broader external classification and CommonJS edge-kind fidelity remain outside the selected contract.

## Deferred platforms

- [#4123](https://github.com/Takazudo/zudo-front-builder/issues/4123): native macOS arm64/x64 browser, V8 and production runtime verification.
- [#4124](https://github.com/Takazudo/zudo-front-builder/issues/4124): normal esbuild Darwin size/packaging contracts and separate release measurements after semantic acceptance.

No Mac execution or connection was requested in this session. Linux evidence does not satisfy those issues. SSR copied-glue URL imports remain a baseline limitation on both backends; adapter inner sourcemaps remain stripped by existing policy.
