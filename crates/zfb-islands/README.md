# `zfb-islands`

Islands runtime pipeline for `zfb`. Scans reachable SDK `Island` boundaries,
resolves their concrete client-eligible function targets, and bundles those
targets with zfb's in-process native Rolldown bundler into a shared entry
bundle plus code-split chunks, and emits the hydration markup + runtime glue.

The crate is fully built out. See `src/lib.rs` rustdoc for the
architecture overview: the `ClientBundler` trait, the production
`IslandsBundler` and its typed jobs (`src/bundle_job.rs`), the scanner,
hydration rewrite, manifest, and code-splitting. The `NativeRustBundler` placeholder lives in
`src/future_rust_native.rs`.

## Boundary discovery implementation contract

The [boundary discovery contract](BOUNDARY-DISCOVERY.md) describes the
implemented #3506–#3508 behavior: actual SDK boundary targets, binding identity,
finite wrapper summaries, actionable diagnostics, and public marker
compatibility. The real production and packed-consumer acceptance lives in
`crates/zfb/tests/scanner_boundary_acceptance.rs`; its separate Chromium check
is under `tests/scanner-boundary-acceptance/`.
