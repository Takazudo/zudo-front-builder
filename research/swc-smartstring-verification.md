# SWC 74 migration verification (#3636)

## Scope and provenance

- Baseline: `70699acdf807b85cc19d050ae30f9adf0d904f57`.
- Inherited migration head: `c8a64c1136e5073be7bc92d8b82ca1dd35938aed`.
- Migration commit: `2064e8875b0ff87afe63b14c7a23579af28b5a5f` (`swc_core` 74; remove the `smartstring` lock entry and advisory exception).
- Verification host: macOS arm64; `rustc 1.94.0`, `cargo 1.94.0`, `cargo-deny 0.19.9`, `cargo-nextest 0.9.140`. `rust-toolchain.toml` selects stable. No Rust compilation ran in this worker.
- Current `Cargo.lock` SHA-256: `695094eb4031b7e0578130ce339ab7cac6c66aedbb0bfb219640a0112d3cc75e`.

## Graph and policy results

- `cargo metadata --format-version 1 --no-deps`: passed; 21 workspace packages. The `compile` feature on `zfb-md-wasm` activates `render` and optional `zfb-render`.
- `cargo tree --workspace --target all -e all --prefix none`, plus lockfile inspection: no `smartstring` package or reverse path. The lockfile contains 20 SWC crate names and no crate has multiple resolved versions; manifests depend on the workspace `swc_core` umbrella, with no direct `swc_ecma_*` dependencies.
- `cargo deny check`: passed on the five configured Linux, macOS, and Windows targets (`advisories ok, bans ok, licenses ok, sources ok`). Duplicate-version and wildcard findings remain warnings under the current policy.
- `bash scripts/assert-zfb-md-wasm-graph.sh`: passed all seven isolated wasm graphs after adding the compile-only surface, including the compiler-off `zfb-content` and default-features-off `zfb-render` boundary checks.
- `bash -n scripts/assert-zfb-md-wasm-graph.sh` and `git diff --check`: passed. `actionlint` is unavailable on this host.

## CI coverage change

The existing wasm matrix covered default, no features, highlight, render, parse, and pipeline. Pipeline includes `compile`, but it does not exercise `--no-default-features --features compile` alone. The matrix and `scripts/assert-zfb-md-wasm-graph.sh` now include that compile-only configuration, with assertions for the SWC parser/codegen graph and native-only dependencies absent. The matrix's cargo check, clippy, and native test steps will exercise the feature on wasm32 and the host. The existing changed-files filter includes `Cargo.toml`, `Cargo.lock`, `deny.toml`, `.github/workflows/health.yml`, and `crates/**`; the required Health workflow itself remains unfiltered, and its wasm browser consumer is selected for these changes.

## Integration and platform evidence

- The manager's guarded `B4PUSH_FULL` log reports the JS suite at 532 passed and four failed: three assertions see the expected `Cargo.lock` provenance change, and one test hit its 5-second timeout under load. Full native/workspace, doctest, `zfb-md-extras --features test-utils`, no-default-feature, and esbuild integration results remain with the manager.
- The Linux x64 measurement is saved in [Health run 37226690489](https://github.com/Takazudo/zudo-front-builder/actions/runs/37226690489), job `111507684157`, artifact `swc-linux-island-size-evidence`. Its PR merge source SHA was `23d6583f2f66516f9d1172174d69888e93a354a3`, with Cargo.lock SHA-256 `695094eb4031b7e0578130ce339ab7cac6c66aedbb0bfb219640a0112d3cc75e`, Node `v24.14.0`, zlib `1.3.1-e00f703`, and esbuild `0.25.12`. All eight fixtures in both workspace and packed modes produced byte-identical inventories across two passes. Raw totals stayed fixed; selected gzip totals decreased by 1–4 bytes. `decision-linux-x64.json` now records those exact totals with zero allowance. The temporary diagnostic workflow wiring is removed before the final CI run.
- The local full gate hit the heavy guard's 1800-second maximum twice during workspace clippy; [deferred verification #3644](https://github.com/Takazudo/zudo-front-builder/issues/3644) tracks the exact CI owed result. The Linux contract assertion passed locally when selected alone.
- A fresh Darwin arm64 native build passed on the second guarded run. Both full real-consumer matrices and both modeled attribution sidecars passed. The [Darwin measurement report](v3-island-size/swc-74-darwin-report.md) records the source, tarballs, toolchain, exact workspace and packed totals, and ignored saved outputs. `decision.json` was updated from those real bytes with zero allowance; `check-budget.mjs` passed on the measurement SHA, and all 16 focused budget assertions passed locally. The terminal batch epic still must remeasure after its later product changes under #3594.
- The newly added compile-only wasm CI leg, the applicable browser gate, and required Health CI results are pending GitHub execution. No SWC fixture correction or size-contract change was made without demonstrated regression evidence.

## Visual checks

Not applicable; this change updates CI configuration, a graph assertion script, and verification notes.
