# #318 findings — 2026-10-10

**Continue narrowly.** Direct Rust consumption is viable at this exact pin for the selected prepared jobs. Executed browser and SSR evidence supports further ownership evaluation; native execution was slower on this tiny fixture. Do not switch defaults, claim general parity, or infer a full-build improvement.

## Revision and scope

- Initial-phase verified implementation commit: `648ba4621a74b1a46358f6a6e9d94c626a978be3` (initial implementation `c0ae55f70fab1b5477b5818576ee572ff92105b5`). The final source follow-up replaces two immediate option-field assignments with an equivalent struct initializer to satisfy native-feature Clippy; later initial-phase evidence/provenance changes do not alter runtime code. The production follow-up adds a real CLI fixture, isolates private compatibility code, and corrects private copy IDs leaking into output graph imports; see [PRODUCTION.md](PRODUCTION.md) and the PR for its exact verified final SHA.
- Repository base: `4c53014c04982687e3960e9f1f8aa6ae2d64d845` (refreshed main).
- Rolldown Git revision: `24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9`, crate version `1.2.13`, including `rolldown_common` from the same revision. `Cargo.lock` pins the complete family; no fork.
- Local tools: Linux x86_64, rustc 1.99.0, Node 24.19.0, esbuild 0.25.12, Playwright 1.58.2 / Chromium 145.0.7632.6, Hono 4.12.8. Development profile (Rust opt-level 0), debug information disabled, four Cargo jobs. The esbuild executable is an optimized prebuilt binary; this comparison is not a release-engine benchmark.
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
| SSR selected pipeline | 130212, 76122, 92001 | 257977, 199172, 204069 |
| Browser selected bundle calls | 346024, 16414, 16012 | 449440, 124009, 123687 |
| Browser production publication | 6303, 5970, 5812 | 5735, 4806, 4634 |
| Native SSR bundle / metadata | n/a | 191898/2860, 164189/2649, 169049/2666 |

For the third SSR iteration, normal timers reported esbuild preparation/invocation/postprocessing at 27/28/35 ms; native at 26/173/4 ms. Native browser main/worker timing is separately printed for every job (third iteration: 114647/5372 us bundling and 1377/91 us metadata). Compilation and browser startup are excluded from these measurements. Three samples do not support a statistical performance conclusion; first-call cache/warm-up overhead is visible. No peak RSS comparison or whole-site/end-to-end speedup claim.

## Ownership costs and constraints

[Upstream Rust policy](https://rolldown.rs/apis/rust-crates) says these crates have no semver guarantee, documentation, or support for Rust-only issues; Rust API contributions are welcome. This is an ownership/maintenance cost, not a feasibility blocker. The next update must be an explicit pin upgrade with this fixture matrix; the bounded historical adjacent-revision rehearsal is documented in [PRODUCTION.md](PRODUCTION.md).

Concrete coupled APIs include `BundlerOptions`, `BundlerTransformOptions`, `ResolveOptions`, native `Plugin` hooks, `ModuleInfo` and `Output::{Chunk,Asset}`. The plugin captures actual `imported_ids`/`dynamically_imported_ids`; chunks expose `module_ids`, imports, dynamic imports and facade ID; assets expose original filenames. The compatibility representation is sorted for deterministic consumers.

- Copy-loader graph edges currently use upstream's private `__ROLLDOWN_COPY_MODULE__#` prefix (`crates/rolldown_plugin_copy_module/src/lib.rs` at the pin), `ctx.get_file_name`, and `OutputAsset.original_file_names`. The real SSR Wasm fixture exercises it. This coupling needs owning or replacing with a small upstream API contribution before long-term adoption.
- File loaders use a small native plugin to preserve compound `.zfb-resource.mjs` and Wasm opaque-byte behavior. Asset paths come from actual emitted references/provenance; sourcemaps are deliberately exempt from source-asset provenance requirements.
- Metadata is the subset existing consumers need, not complete esbuild metadata. Input byte counts are transformed source lengths; output input contributions omit `bytesInOutput`. Static/dynamic edges do not preserve esbuild's finer `require-call` kinds. Selected graph assertions concern ESM.
- Unknown prepared flags fail closed. External wildcard support is limited to suffix `*`; other patterns fail. NODE_PATH handling uses the explicitly prepared command environment. This is a bounded decoder, not blanket policy parity for arbitrary flags/loaders/resolution plugins.
- Rolldown unresolved imports can otherwise become warning/externalization. The adapter treats `UNRESOLVED_IMPORT` as failure, retaining rendered diagnostics. Other warnings remain visible (including `use client` directive and explicit JSX-vs-tsconfig warning).
- Initial lock resolution required targeted minimum-version updates (`rustc-hash`, `oxc_resolver`, `json-escape-simd`, `regex`, `uuid`, `unicode-segmentation` and their closure); shared OXC/resolver users see the lock closure changes even when native is off. Lock package entries grow from 594 to 689 (63 newly named packages). `swc_core` stays at 74.0.2; OXC parsers 0.95.0 and 0.153.0 coexist. This is dependency/AST duplication evidence, not a binary-size measurement. No SWC removal, deno_core replacement or host replacement. Feature-off compilation is checked separately.
- Initial Git source fetch also fetched Rollup/test262 submodules. Dependency resolution failed after 174 seconds before compilation; subsequent 75-second compile exposed two adapter type mistakes, fixed locally. Final warm build completed in about 3 seconds. One 33-second feature-union rebuild was inadvertently unguarded after assuming it was warm; other heavy builds and real-browser runs used the shared guard. These are compilation/setup costs, not bundler timings.

## Verification command ledger

All commands ran from the repository root; the installed Cargo home/tool paths were `/workspace/.cargo`, `/workspace/.rustup` and `/workspace/zfb-prototype-tools`. `CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0` and the README's explicit esbuild path were set. The commands below abbreviate only those environment assignments. Runtime execution at the implementation SHA above completed with guard `verdict=PASS exit=0 secs=5`.

| Command | Observed outcome |
| --- | --- |
| Guarded `cargo build -p zfb-build -p zfb-islands --features rolldown-prototype --example rolldown_ssr --example rolldown_browser` | PASS, final warm compile 2.90 s |
| Guarded `cargo build --locked -p zfb-rolldown-prototype --features native --example prepared` | PASS, 25.47 s with smaller feature union |
| `node prototypes/rolldown/prepare.mjs`; guarded `node prototypes/rolldown/verify.mjs` | PASS on committed source: direct graph/API gate, both SSR hosts, browser producers, fresh-process client stages, Chromium; assertion excerpts in [EVIDENCE.txt](EVIDENCE.txt) |
| `cargo test -p zfb-build --features rolldown-prototype --test bundler_css_modules --test bundler_workspace_pkg_alias --test bundler_main_fields` | PASS 6/6 native and 6/6 baseline; none skipped |
| Native `cargo test -p zfb-build --features rolldown-prototype --test bundler_root_workspace_stage_escape_audit_armed_regression -- --ignored` | PASS 1/1; initial command without `--ignored` skipped and was not counted |
| Native `cargo test -p zfb-build --features rolldown-prototype --test bundler_exact_match_resolution plugin_alias_matches_exact_specifier` and `plugin_alias_does_not_match_prefix_with_slash` | PASS 1/1 each |
| Guarded `cargo check -p zfb-build -p zfb-islands --no-default-features` | PASS, 17.45 s; native feature absent |
| Native `cargo clippy --locked -p zfb-rolldown-prototype --features native --lib -- -D warnings` | PASS after equivalent options-initializer cleanup |
| `cargo fmt --all --check`; Oxfmt 0.70.0 (repository fmt settings) check on prototype JS/TS; mdx-formatter 1.2.1 check on prototype MD/MDX | PASS |
| `compilerSourceDigest(REPO_ROOT)` from `docs/scripts/wind-preview-assets.mjs` | Refreshed and verified existing wind manifest's source-only digest (covers Cargo and all crates); no CSS or release artifacts regenerated |

## Initial PR CI diagnosis

On `335cdc36`, the docs check failed at generated English utility references: `Wind source differs from reviewed support pin 478bbf83e137fbdbe2f386df834839c08e884b9e`. That separate reviewed Wind closure hashes root `Cargo.lock` as well as unchanged Wind sources. After **117/117** `cargo test --locked -p zudo-wind --lib` tests passed (guard PASS, 22 s), its digest was refreshed with the existing 100-path count, fixed support pin and assertion logic unchanged. English and Japanese generated-reference checks now pass locally. CI's workspace Wind preview asset job also passed on the initial head, checking generated CSS against the changed dependency closure.

On `b40a06d3`, four existing island-size-budget tests still rejected the new Cargo lock hash. Both complete normal-esbuild size matrices were then measured on pinned Linux and native Darwin; all raw/gzip totals exactly match the existing ceilings. Only lock provenance and explanatory status were refreshed, preserving ceilings, zero allowances, assertions and original baseline evidence. The 17 budget tests plus 22 Wind-reference tests pass. See [native measurement and artifact replay report](../../research/v3-island-size/rolldown-lock-provenance-report.md). The linked-package probe failures on earlier CI heads were a cascade: the skipped CLI build left `target/debug/zfb` absent.

The repository automatically started docs/showcase preview workflows when the draft opened. They were canceled to honor this task's explicit no-publish/no-deploy boundary. The docs upload job and showcase preview job executed no steps; the associated binary/smoke jobs were canceled along with their preview-producing workflow. Those cancellations are not passing validation, nor compiler/test failures. Normal nondeployment CI is observed separately on the PR.

### Terminal-CI follow-up

CI for `de58861d` used synthetic merge `f3a6806904e71d412af12254ec2b72e9d5837f7a` with newer main `11623738455758588cb0ad811778c711c8638b6d` (the 4.3.0 release metadata). That main was synchronized locally before the follow-up. The original source-only Wind digest matched a clean archive of the prototype head, but the merged package/measurement JSON changed the digest. The actual Wind generator and freshness checker were rerun on the combined tree: all 173 samples pass, with only the manifest digest changing, no CSS changes.

The default Wasm job exposed a separate **real shared-lock cost**. [Baseline main job 114130159406](https://github.com/Takazudo/zudo-front-builder/actions/runs/38023765871/job/114130159406) passed with rustc 1.99.0 (`b940084d7`); [prototype merge job 114131518358](https://github.com/Takazudo/zudo-front-builder/actions/runs/38024216690/job/114131518358) used the same compiler and passed all 237 consumer tests plus generated-export checks. Its four artifacts remained below every existing ceiling, but differed from the documented measurements. The actual budget step's file measurements, not expected values, supply the refreshed measured rows below. Generated size documentation was updated using `assert-md-wasm-size-docs.mjs --fix`; ceilings, gzip tolerance, build flags and checker logic are unchanged.

| Artifact | CI final Wasm bytes | CI gzip-9 bytes | Existing gzip ceiling |
| --- | --- | --- | --- |
| default | 3385636 | 1520643 | 1600000 |
| highlight | 1540775 | 820953 | 880000 |
| render | 2201047 | 1095897 | 1100000 |
| parse | 700678 | 284215 | 325000 |

Nine updated packages are present in `cargo tree --locked -p zfb-md-wasm --target wasm32-unknown-unknown --edges normal`: bitflags 2.13.2, memchr 2.8.3, regex 1.13.1, regex-automata 0.4.18, regex-syntax 0.8.11, rustc-hash 2.1.3, serde_json 1.0.151, smallvec 1.16.3 and uuid 1.28.0. No per-package causal attribution is claimed.

A bounded local four-artifact build used the same compiler, wasm-bindgen 0.2.121 and pinned Binaryen. The first invocation omitted CI's version stamp; the corrected `ZFB_RELEASE_VERSION=4.3.0` invocation passed under the guard in 58 seconds. Local raw sizes still differ slightly from hosted CI (for example default 3385169 versus 3385636); those local numbers were **not** substituted into the manifest. Existing policy treats CI as the authority, so the unchanged next CI budget check must remeasure and validate the refreshed rows. No environment-matching experiment or tolerance relaxation was attempted. Final terminal workflow outcomes are recorded in the draft PR.

## Failures retained as evidence

The first browser run timed out because the test server selected an old hashed entry from a reused output directory, producing a build-token mismatch. The fix uses the actual production pipeline receipt; both engines then passed the same browser assertions. Earlier fixture setup failures included hardcoded worker URL, incomplete SDK package copies, and `className` in the owned JSX dialect. The first native route dependency assertion found absolute graph keys incompatible with the existing cwd-relative schema; the adapter now changes key representation consistently without rediscovering edges. None of these was classified as an upstream capability gap; none was fixed by weakening the asserted contract.

## Remaining callers and deliberate limits

| Existing family | Prototype coverage |
| --- | --- |
| `zfb-build/src/bundler.rs::run_esbuild` SSR/SSG/runtime prepared bundles | Selected SSR job executed; not a complete SSG/runtime production route matrix |
| `zfb/src/commands/build.rs` runtime-only deployment rebundle (`worker_only_routes`, `bundle-runtime.mjs`) | Executed through actual CLI and local adapter for the bounded mixed SSG/SSR catch-all fixture on both engines; scratch and adapter inner output execute in existing V8; normal path stays esbuild |
| `zfb-islands/src/esbuild.rs::bundle_one_entry` shared islands | Selected production entry, splitting, resources and its companion workers executed |
| `zfb-islands/src/esbuild.rs` inherent client-script bundling | Remains esbuild; not routed through this experiment |
| `zfb-config-loader/src/loader.rs` embedded-V8 and Node config evaluation prebundle | Remains esbuild; both hosts unchanged |
| `zfb-build/src/plugin_bundler.rs` TypeScript plugin prebundle | Remains esbuild |
| Version probes and direct esbuild-specific regression helpers | Remain esbuild |

No public backend selection API, `NativeRustBundler` implementation, staging redesign, default switch, deployment, release, broad local CI-equivalent run, local full end-to-end suite, Windows/macOS prototype-runtime validation (the separate Darwin size-provenance measurement runs the unchanged esbuild baseline), source-map-enabled browser production rename test, or package-wide compatibility claim. The harness's node_modules link is Unix-specific. No broad repeated workspace builds or b4push were run. Change-induced lock provenance failures additionally required one targeted CLI build and the existing 32-build size matrix on each of Linux and native Darwin; these are the normal esbuild path, separate from the prototype runtime matrix. Keep #318 and this draft PR open.

Next bounded work: seek a typed upstream copy-edge API, decide the baseline SSR copied-resource contract separately, and broaden selected CJS/external/alias cases only as real callers require. The production second pass and one historical adjacent transition are now exercised; private coupling is isolated but remains owned. A subsequent bounded release-profile comparison is documented in [RELEASE-MEASUREMENT.md](RELEASE-MEASUREMENT.md); config/plugin/client-script migration remains separate work. See [PRODUCTION.md](PRODUCTION.md).

## Terminal CI reconciliation before the production follow-up

Head `8e029dd2b067c52fbc6fc7f603d077b6199eba42` reached terminal CI. Docs, PR checks, Wind computed styles, no-V8 and all seven Wasm legs passed; default Wasm job 114134347041 remeasured the exact refreshed bytes above and passed 237 consumer tests. Main health passed compilation, Clippy and 6730 tests, then failed the Linux island gzip gate. Its five failing rows exactly match baseline main 11623738455758588cb0ad811778c711c8638b6d at job 114130135066. All 16 measurement rows and all 92 saved emitted files are byte-identical between baseline artifact 11660048737 and PR artifact 11660256168. Their ZIP hashes are respectively `f928c7b1e66fbed30ed3a7ba4a52286700227c727674bc4a5ad07cd396d7766d` and `400c6bb101b30c4447d726635369740d9551e0a00d65302fe9decaa9a7e7ab0f`. This is a confirmed baseline failure, not a native/lock regression; ceilings remain unchanged. The dependent Wasm browser leg skipped. Preview-producing workflows were intentionally canceled before upload steps. Final follow-up-head terminal results are recorded in the PR without an evidence-only push loop.

## Release measurement and boundary hardening follow-up

See [RELEASE-MEASUREMENT.md](RELEASE-MEASUREMENT.md) for the unchanged optimized profile, bounded equivalent-input protocol, exact invocation, copy-provenance regression contract, and limitations. Mac runtime and normal-path size/packaging work remain explicitly deferred in [#4123](https://github.com/Takazudo/zudo-front-builder/issues/4123) and [#4124](https://github.com/Takazudo/zudo-front-builder/issues/4124). No dependency pin or Cargo.lock change is needed for this follow-up.
