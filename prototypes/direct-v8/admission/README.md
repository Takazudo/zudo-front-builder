# Proposed Cargo.lock admission for #4307 (stage preparation only)

Status: **proposal, not approved, not staged, not applied.** Admission is a live
contract change that needs owner approval and the native Darwin evidence owned by
the security/Darwin evidence lane. Nothing here edits `research/v3-island-size/`
contracts, ceilings, allowances or transition records.

## The delta

| | Whole-lock SHA-256 | Records |
| --- | --- | ---: |
| Approved (main `21ef7c4`) | `230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138` | 697 |
| #4307 | `07fa645050cc0db69ad75f2bc5b824a7cd558dc101f3619089e83d397003e4f9` | 697 |

One record changes: unpublished `zfb-js-host@0.0.0` gains the existing external
edge `deno_core` → `deno_core@0.399.0` (record bytes unchanged). No record is
added or removed and no external version, source or checksum moves.

## Producer closure receipts

`receipts/` holds `scripts/record-cargo-producer-closure.mjs --producer zfb`
output (default features, `cargo metadata --locked --filter-platform`) for both
targets: `old-*` at `21ef7c4`, `new-*` at the commit that adds this directory.
The Darwin receipt is metadata resolution on Linux, not native Mac evidence.

| Target | Rows old/new | Only change |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | 498 / 498 | `zfb-js-host` features gain `deno-platform`, `v8-platform` |
| `aarch64-apple-darwin` | 482 / 482 | same |

Before this commit, the #4307 head (`bd4feaf`) also changed the external `v8`
row's features (`+default`), because `zfb-js-host` requested v8 with default
features while `deno_core` does not. The #4310 validator refuses any external
feature drift, so that head could not be admitted even with the new capability.
`zfb-js-host` now asks for `v8` with `default-features = false` and
`use_custom_libcxx` (v8's only default), which leaves the external row identical
and the compiled feature set unchanged.

## Dry proof

`prove-rewire.mjs` runs the existing `proveCargoInternalRewire` over the real
locks, `rewire-declaration.json` (schema 2) and the receipts, for both targets.
The artifact identity is substituted with the expected lock, so it proves the
structural rule only, never measured sizes.

```sh
node prototypes/direct-v8/admission/prove-rewire.mjs \
  --old-ref 21ef7c4678adba115a3607801ededeeaf6e5814e \
  --receipts prototypes/direct-v8/admission/receipts
```

Both targets pass at the commit that adds this directory. With the `bd4feaf`
receipts the same command fails on the `v8` feature drift (negative control).

## Remaining before stage/apply

1. Darwin arm64 native workspace/packed measurements at the candidate SHA, plus
   a reviewed local-provenance ingestion route (see
   `research/v3-island-size/EXISTING-EXTERNAL-EDGES.md`). Owned by L1 and the owner.
2. The Linux `island-size-linux-x64` health artifact for the same candidate SHA
   (the ordinary health job retains it with its expected lock rejection).
3. Owner approval for the paired `--stage`, review, then `--apply`.
