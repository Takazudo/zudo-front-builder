# Proposed Cargo.lock admission for #4301 on main's rustls lock (stage preparation only)

Status: **proposal, not approved, not staged, not applied.** Admission is a live
contract change that needs owner approval and the native Darwin evidence owned by
the security/Darwin evidence lane. Nothing here edits `research/v3-island-size/`
contracts, ceilings, allowances or transition records.

## The delta

| | Whole-lock SHA-256 | Records |
| --- | --- | ---: |
| Approved (main `f9dd1f5`, #4369 rustls) | `563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd` | 697 |
| #4301 integration (`base/4301-direct-v8` + main) | `0cd091b7adcc7776895d9de119ac28ac8e8a931cdf28e46c676d5460627cc6f2` | 697 |

The original #4307 proposal (`230f0acb…` → `07fa6450…` against main `21ef7c4`) was
superseded when #4314's rustls transition landed on main first. The declared delta
is the same single record, re-declared against main's new approved lock.

One record changes: unpublished `zfb-js-host@0.0.0` gains the existing external
edge `deno_core` → `deno_core@0.399.0` (record bytes unchanged). No record is
added or removed and no external version, source or checksum moves.

## Producer closure receipts

`receipts/` holds `scripts/record-cargo-producer-closure.mjs --producer zfb`
output (default features, `cargo metadata --locked --filter-platform`) for both
targets: `old-*` at `f9dd1f5`, `new-*` at the integration head that carries this README.
The Darwin receipt is metadata resolution on Linux, not native Mac evidence.
Files use the `.receipt` extension so the repository formatter keeps the recorder's
original bytes.

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
  --old-ref f9dd1f5f9dad47555cd8ff41b9e65dc0a2e04410 \
  --receipts prototypes/direct-v8/admission/receipts
```

Both targets pass on the integration head (declaration SHA-256
`ce15e6cb5f9eff48902b859b2d35060e617bee639d8500440deafb0e751f1dba`). With the `bd4feaf`
receipts the same command fails on the `v8` feature drift (negative control).

## Remaining before stage/apply

1. Darwin arm64 native workspace/packed measurements at the candidate SHA, plus
   a reviewed local-provenance ingestion route (see
   `research/v3-island-size/EXISTING-EXTERNAL-EDGES.md`). Run on the owner's Mac.
2. The Linux `island-size-linux-x64` health artifact for the same candidate SHA
   (the ordinary health job retains it with its expected lock rejection).
3. Owner approval for the paired `--stage`, review, then `--apply`.
