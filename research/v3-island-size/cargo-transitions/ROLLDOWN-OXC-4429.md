# Rolldown oxc edge (#4429, PR #4440)

[`rolldown-oxc-4429.json`](rolldown-oxc-4429.json) is the reviewed `--reason cargo-internal-rewire`
declaration (schema 2, [existing external edge](../EXISTING-EXTERNAL-EDGES.md)) for the #4429 lock
move. Takazudo approved this edge-only transition on 2026-10-23.
[`rolldown-oxc-4429.old.lock`](rolldown-oxc-4429.old.lock) is
`7b062415c002151adf20b52aaf77019a90adfa0e:Cargo.lock` verbatim.

| Lock | SHA-256 |
| --- | --- |
| old (main `7b06241`, the Deno-exit new lock) | `f004a0680082afbfa29e215dc88bacff751da71f0aa80f9362a30c1248630216` |
| new (PR #4440) | `e79303ad33f123966d23efb32e309f4263afc00e587ea14bab3a2c872273893d` |

- **Only change:** the internal `zfb-rolldown-prototype@0.0.0` record gains the dependency entry
  `oxc`, resolving to the existing, byte-identical `oxc@0.153.0` registry record that Rolldown
  already locks. No record is added, removed or otherwise changed.
- **Producer closures:** the default-feature `zfb` closure receipts in
  [`rolldown-oxc-4429/`](rolldown-oxc-4429/) are row-for-row identical at both locks for
  `x86_64-unknown-linux-gnu` (580 rows) and `aarch64-apple-darwin` (564 rows). Both endpoints
  are already in them, and `oxc` keeps its feature set (`ast_visit` was already enabled), so
  nothing that is built changes; `closure.changed` is empty. Receipts were recorded on Linux
  (`cargo metadata --filter-platform` resolves the Darwin target without its toolchain).
- `proveCargoInternalRewire` proves the declaration for both targets
  (`scripts/__tests__/cargo-existing-external-edge.test.mjs`).

Application is the paired Linux/Darwin `cargo-internal-rewire` stage and apply: the Linux health
artifact for the PR plus owner-attested local Darwin evidence. Ceilings, allowances and the
dependency fingerprint do not move. The declaration binds the new lock hash, so any later
`Cargo.lock` change before the paired re-pin needs a re-declared transition and fresh receipts.
