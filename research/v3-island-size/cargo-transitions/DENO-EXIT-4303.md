# Deno exit Cargo removal (#4303, #4395/#4396)

[`deno-exit-4303.json`](deno-exit-4303.json) is the reviewed `--reason cargo-removal`
declaration for the Gate B lock move. [`deno-exit-4303.old.lock`](deno-exit-4303.old.lock) is
`2abb23825ff5a880d761e9c1cdce57b062a2b839:Cargo.lock` verbatim.

| Lock | SHA-256 |
| --- | --- |
| old (main `2abb238`) | `c0d708895c54e312b0f5f9d90ee67b5d1037b9a667ece59d475791a5619e5b64` |
| new (PR #4412) | `f004a0680082afbfa29e215dc88bacff751da71f0aa80f9362a30c1248630216` |

- **Removed (21):** az, boxed_error, capacity_builder, capacity_builder_macros, cooked-waker,
  deno_core, deno_core_icudata, deno_error, deno_error_macro, deno_ops, deno_path_util,
  deno_unsync, inventory, serde_v8, stringcase, strum, strum_macros, syn-match, sys_traits,
  sys_traits_macros, wasm_dep_analyzer. No record is added.
- **Changed internal records:** `zfb-js-host` drops `deno_core`; `zfb-render` drops `deno_core`,
  `deno_error`, `getrandom 0.3.4` and `reqwest` (its embed_v8-only edges).
- **Pruned external records (`prunedExternal`):** `num-bigint@0.4.6` drops `rand 0.8.6`,
  `tokio@1.52.3` drops `parking_lot` and `url@2.5.8` drops `serde_derive`. Version, source and
  checksum are unchanged. Cargo.lock lists every optional edge any workspace feature
  activates, and only deno_core's feature requests activated these three. #4396 added this
  declared shape to the cargo-removal proof; any other external change is still refused.
- **Producer closures:** the default-feature `zfb` closure receipts in
  [`deno-exit-4303/`](deno-exit-4303/) are row-for-row identical at both locks for
  `x86_64-unknown-linux-gnu` (473 rows) and `aarch64-apple-darwin` (457 rows), so nothing that
  is built changes; `closure.removed` is empty. Receipts were recorded with cargo 1.97.0 on
  Linux (`cargo metadata --filter-platform` resolves the Darwin target without its toolchain).
- `proveCargoRemoval` proves the declaration for both targets.

The declaration binds the new lock hash. Any later `Cargo.lock` change before the paired
Linux/Darwin re-pin (S8, #4397) needs a re-declared transition and fresh receipts.
