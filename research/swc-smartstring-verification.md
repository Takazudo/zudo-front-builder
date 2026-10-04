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

## Pending evidence

- The manager's guarded `B4PUSH_FULL` log reports the JS suite at 532 passed and four failed: three assertions see the expected `Cargo.lock` provenance change, and one test hit its 5-second timeout under load. Full native/workspace, doctest, `zfb-md-extras --features test-utils`, no-default-feature, and esbuild integration results remain with the manager.
- The Linux x64 island contract still records Cargo lock SHA-256 `62cb06933c5048c43f20ed696f53dfc5ba57b444876396e3822b860b2bc0b21d`; the current lock hashes to `695094eb4031b7e0578130ce339ab7cac6c66aedbb0bfb219640a0112d3cc75e`. Its exact ceilings and zero allowance are unchanged pending measurement.
- A saved Linux x64 measurement is required for the complete eight-fixture workspace and packed matrices before changing `research/v3-island-size/decision-linux-x64.json`. The normal Health workflow's `pnpm test:workspace` runs before its island-size measurement, so the current provenance assertion failures skip that later measurement step. The current workflow does not upload the measurement directories. The manager will arrange a temporary Linux CI run that preserves the `real-workspace` and `real-packed` evidence, then update exact provenance/totals only from those saved artifacts. Darwin size evidence remains outstanding under #3594.
- The newly added compile-only wasm CI leg, the applicable browser gate, and required Health CI results are pending GitHub execution. No SWC fixture correction or size-contract change was made without demonstrated regression evidence.

## Visual checks

Not applicable; this change updates CI configuration, a graph assertion script, and verification notes.
