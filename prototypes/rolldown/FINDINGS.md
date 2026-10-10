# #318 findings — 2026-10-10

**Continue narrowly.** Direct Rust consumption is viable at this exact pin for the selected prepared jobs. Executed browser and SSR evidence supports further ownership evaluation; native execution was slower on this tiny fixture. Do not switch defaults, claim general parity, or infer a full-build improvement.

## Revision and scope

- Verified implementation commit: `c0ae55f70fab1b5477b5818576ee572ff92105b5`; final evidence-only follow-up changes this report and source-digest metadata.
- Repository base: `4c53014c04982687e3960e9f1f8aa6ae2d64d845` (refreshed main).
- Rolldown Git revision: `24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9`, crate version `1.2.13`, including `rolldown_common` from the same revision. `Cargo.lock` pins the complete family; no fork.
- Local tools: Linux x86_64, rustc 1.99.0, Node 24.19.0, esbuild 0.25.12, Playwright 1.58.2 / Chromium 145.0.7632.6, Hono 4.12.8. Development profile, debug information disabled, four Cargo jobs. No release benchmark.
- This follows the accepted 2026-10-10 brief and four historical comments on [issue #318](https://github.com/Takazudo/zudo-front-builder/issues/318). Browser, SSR, and review evidence is bounded to the checked-in fixture and selected existing tests.

`zfb-rolldown-prototype` owns a strict decoder of ZFB's **already prepared esbuild command contract**, plus compatibility metadata. This is not a generalized neutral compiler API. It runs `Bundler::with_plugins(...).write()` in process, on an isolated thread/Tokio runtime. Only `std::process::Command` and `anyhow::Result` cross the boundary; no upstream types are exposed. No Node binding or Rolldown CLI is invoked.

The SSR seam is after final plugin aliases/virtual modules and synthetic tsconfig preparation in `run_esbuild`. The islands seam is after real registration entry generation and policy assembly; its module workers use the same native adapter. Existing MDX compilation, CSS Module processing, worker URL preparation, original-source maps, resource readback, dependency consumers, exclusion and stage-escape audits remain active. Resolver choices come from actual bundler graph data; there is no replacement speculative resolver or authored import scan.

## Runtime and contract evidence

| Gate | Result | Executed assertion |
| --- | --- | --- |
| Direct native API | PASS | `gate.mjs`: actual static/dynamic graph, split ESM imported by Node, 42/43 exports, linked map, actionable missing import |
| Browser owned JSX | PASS, both backends | Real Chromium hydrates V8-rendered `Counter`, with matching build identity and preserved component function name; mounted marker then counter click changes 0 to 1 |
| Browser companions | PASS, both | Click executes dynamic chunk, module worker (40+2), fetched Wasm export returning 42, copied `.zfb-resource.mjs` export returning 42; existing output reader verifies provenance |
| Production rename | PASS, both | Actual `ProductionAssetPipeline::apply` hashes entry; browser executes the returned URL with stable companion references |
| Embedded SSR | PASS, both | Actual `EmbeddedV8RenderHost` executes alternate ESM and serves 200 HTML with owned SDK island, MDX bold content, actual preprocessed CSS class, tsconfig/workspace alias |
| Throw/source map | PASS, both | Linked `bundle.mjs.map`, existing production mapper reprojects actual V8 throw to `pages/throw.tsx:2:9` |
| Graph/dependencies | PASS, both | Existing route dependency collector includes original `Hero.mdx`, transitive `message-dep.ts`, `answer.wasm`, and glue; SSR Wasm deployment manifest contains one asset |
| Negative exclusion/import | PASS, both | Excluded transitive file fails; unresolved `absent-probe` fails and names specifier |
| Stage escape | PASS, native | Existing real root-workspace symlink escape fixture, explicitly run with `--ignored`, rejects via existing audit |
| Reused fixtures | PASS | Six existing CSS Modules, main-fields/external, workspace-package alias tests on both backends; exact plugin alias and prefix-negative tests native |
| Determinism | PASS | Three same-backend repetitions compare SSR bytes and browser entry plus sorted companions; separate fresh client-stage invocation also checked |

Browser source contains an authored `new URL('./probe.ts', import.meta.url)` and uses the existing stage rewrite helper. It does not hardcode a synthetic emitted worker URL. SSR fixture's `class` follows the owned JSX dialect. Function-name compatibility is exercised by actual named registration/hydration, not just checking a printed identifier.

## Timings

Prepared-binary samples are emitted by each harness; Rust compilation is separate. `selected_pipeline_us` includes SSR staging, native/esbuild invocation and normal postprocessing. `selected_bundle_call_us` includes islands entry preparation, main plus worker bundling and output readback; `production_publish_us` separately measures the actual production writer. Native `bundle` covers graph/build/generate/write; `metadata` covers the owned graph/provenance adaptation and serialization. Existing SSR timing messages still label the selected invocation `esbuild` because that is the existing seam name.

Uncontended prepared run (microseconds, first process, three iterations):

| Measurement | esbuild | native Rolldown |
| --- | --- | --- |
| SSR selected pipeline | 116617, 76356, 71795 | 316691, 236671, 215262 |
| Browser selected bundle calls | 345291, 17982, 16316 | 456887, 128196, 177428 |
| Browser production publication | 8325, 6410, 6276 | 5431, 4802, 6589 |
| Native SSR bundle / metadata | n/a | 248681/2768, 189634/3061, 168155/2811 |

For the third SSR iteration, normal timers reported esbuild preparation/invocation/postprocessing at 26/17/27 ms; native at 33/175/5 ms. Native browser main/worker timing is separately printed for every job (third iteration: 166645/6435 us bundling and 1558/129 us metadata). Compilation and browser startup are excluded from these measurements. Three samples do not support a statistical performance conclusion; first-call cache/warm-up overhead is visible. No peak RSS comparison or whole-site/end-to-end speedup claim.

## Ownership costs and constraints

[Upstream Rust policy](https://rolldown.rs/apis/rust-crates) says these crates have no semver guarantee, documentation, or support for Rust-only issues; Rust API contributions are welcome. This is an ownership/maintenance cost, not a feasibility blocker. The next update must be an explicit pin upgrade with this fixture matrix; an adjacent-revision upgrade exercise was not attempted.

Concrete coupled APIs include `BundlerOptions`, `BundlerTransformOptions`, `ResolveOptions`, native `Plugin` hooks, `ModuleInfo` and `Output::{Chunk,Asset}`. The plugin captures actual `imported_ids`/`dynamically_imported_ids`; chunks expose `module_ids`, imports, dynamic imports and facade ID; assets expose original filenames. The compatibility representation is sorted for deterministic consumers.

- Copy-loader graph edges currently use upstream's private `__ROLLDOWN_COPY_MODULE__#` prefix (`crates/rolldown_plugin_copy_module/src/lib.rs` at the pin), `ctx.get_file_name`, and `OutputAsset.original_file_names`. The real SSR Wasm fixture exercises it. This coupling needs owning or replacing with a small upstream API contribution before long-term adoption.
- File loaders use a small native plugin to preserve compound `.zfb-resource.mjs` and Wasm opaque-byte behavior. Asset paths come from actual emitted references/provenance; sourcemaps are deliberately exempt from source-asset provenance requirements.
- Metadata is the subset existing consumers need, not complete esbuild metadata. Input byte counts are transformed source lengths; output input contributions omit `bytesInOutput`. Static/dynamic edges do not preserve esbuild's finer `require-call` kinds. Selected graph assertions concern ESM.
- Unknown prepared flags fail closed. External wildcard support is limited to suffix `*`; other patterns fail. NODE_PATH handling uses the explicitly prepared command environment. This is a bounded decoder, not blanket policy parity for arbitrary flags/loaders/resolution plugins.
- Rolldown unresolved imports can otherwise become warning/externalization. The adapter treats `UNRESOLVED_IMPORT` as failure, retaining rendered diagnostics. Other warnings remain visible (including `use client` directive and explicit JSX-vs-tsconfig warning).
- Initial lock resolution required targeted minimum-version updates (`rustc-hash`, `oxc_resolver`, `json-escape-simd`, `regex`, `uuid`, `unicode-segmentation` and their closure); shared OXC/resolver users see the lock closure changes even when native is off. Lock package entries grow from 594 to 689 (63 newly named packages). `swc_core` stays at 74.0.2; OXC parsers 0.95.0 and 0.153.0 coexist. This is dependency/AST duplication evidence, not a binary-size measurement. No SWC removal, deno_core replacement or host replacement. Feature-off compilation is checked separately.
- Initial Git source fetch also fetched Rollup/test262 submodules. Dependency resolution failed after 174 seconds before compilation; subsequent 75-second compile exposed two adapter type mistakes, fixed locally. Final warm build completed in about 3 seconds. One 33-second feature-union rebuild was inadvertently unguarded after assuming it was warm; other heavy builds and real-browser runs used the shared guard. These are compilation/setup costs, not bundler timings.

## Verification command ledger

All commands ran from the repository root; the installed Cargo home/tool paths were `/workspace/.cargo`, `/workspace/.rustup` and `/workspace/zfb-prototype-tools`. `CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0` and the README's explicit esbuild path were set. The commands below abbreviate only those environment assignments. Runtime execution at the implementation SHA above completed with guard `verdict=PASS exit=0 secs=6`.

| Command | Observed outcome |
| --- | --- |
| Guarded `cargo build -p zfb-build -p zfb-islands --features rolldown-prototype --example rolldown_ssr --example rolldown_browser` | PASS, final warm compile 2.90 s |
| Guarded `cargo build --locked -p zfb-rolldown-prototype --features native --example prepared` | PASS, 25.47 s with smaller feature union |
| `node prototypes/rolldown/prepare.mjs`; guarded `node prototypes/rolldown/verify.mjs` | PASS on committed source: direct graph/API gate, both SSR hosts, browser producers, fresh-process client stages, Chromium; assertion excerpts in [EVIDENCE.txt](EVIDENCE.txt) |
| `cargo test -p zfb-build --features rolldown-prototype --test bundler_css_modules --test bundler_workspace_pkg_alias --test bundler_main_fields` | PASS 6/6 native and 6/6 baseline; none skipped |
| Native `cargo test -p zfb-build --features rolldown-prototype --test bundler_root_workspace_stage_escape_audit_armed_regression -- --ignored` | PASS 1/1; initial command without `--ignored` skipped and was not counted |
| Native `cargo test -p zfb-build --features rolldown-prototype --test bundler_exact_match_resolution plugin_alias_matches_exact_specifier` and `plugin_alias_does_not_match_prefix_with_slash` | PASS 1/1 each |
| Guarded `cargo check -p zfb-build -p zfb-islands --no-default-features` | PASS, 17.45 s; native feature absent |
| `cargo fmt --all --check`; Oxfmt 0.70.0 (repository fmt settings) check on prototype JS/TS; mdx-formatter 1.2.1 check on prototype MD/MDX | PASS |
| `compilerSourceDigest(REPO_ROOT)` from `docs/scripts/wind-preview-assets.mjs` | Refreshed and verified existing wind manifest's source-only digest (covers Cargo and all crates); no CSS or release artifacts regenerated |

## Failures retained as evidence

The first browser run timed out because the test server selected an old hashed entry from a reused output directory, producing a build-token mismatch. The fix uses the actual production pipeline receipt; both engines then passed the same browser assertions. Earlier fixture setup failures included hardcoded worker URL, incomplete SDK package copies, and `className` in the owned JSX dialect. The first native route dependency assertion found absolute graph keys incompatible with the existing cwd-relative schema; the adapter now changes key representation consistently without rediscovering edges. None of these was classified as an upstream capability gap; none was fixed by weakening the asserted contract.

## Remaining callers and deliberate limits

| Existing family | Prototype coverage |
| --- | --- |
| `zfb-build/src/bundler.rs::run_esbuild` SSR/SSG/runtime prepared bundles | Selected SSR job executed; not a complete SSG/runtime production route matrix |
| `zfb/src/commands/build.rs` runtime-only deployment rebundle (`worker_only_routes`, `bundle-runtime.mjs`) | Calls the same `run_esbuild` seam and could select native under the feature/env opt-in; this second production pass was not executed here; normal path stays esbuild |
| `zfb-islands/src/esbuild.rs::bundle_one_entry` shared islands | Selected production entry, splitting, resources and its companion workers executed |
| `zfb-islands/src/esbuild.rs` inherent client-script bundling | Remains esbuild; not routed through this experiment |
| `zfb-config-loader/src/loader.rs` embedded-V8 and Node config evaluation prebundle | Remains esbuild; both hosts unchanged |
| `zfb-build/src/plugin_bundler.rs` TypeScript plugin prebundle | Remains esbuild |
| Version probes and direct esbuild-specific regression helpers | Remain esbuild |

No public backend selection API, `NativeRustBundler` implementation, staging redesign, default switch, deployment, release, CI-wide run, full end-to-end suite, Windows/macOS validation, source-map-enabled browser production rename test, or package-wide compatibility claim. The harness's node_modules link is Unix-specific. No broad repeated builds or b4push were run. Keep #318 and this draft PR open.

Next bounded work: replace/own the argv coupling and copy-loader private prefix; evaluate required CommonJS and external/alias option semantics against real consumer jobs; exercise the alternate production SSR pass and broader source-map/diagnostic cases; run a controlled release-profile benchmark on representative prepared inputs; attempt one adjacent upstream pin update and measure patch/lock churn. Config/plugin/client-script migration requires separate caller-specific evidence.
