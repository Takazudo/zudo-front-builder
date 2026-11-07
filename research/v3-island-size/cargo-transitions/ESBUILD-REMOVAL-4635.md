# Exact esbuild-removal lock move (#4635)

This is the proposed reviewed `Cargo.lock` move of the esbuild-removal milestone
([#4628](https://github.com/Takazudo/zudo-front-builder/issues/4628), tasks #4635/#4636). **It is pending
owner approval.** Registering it in `scripts/security-lock-transitions.mjs` makes the shared
reviewed-transition entry point recognize exactly this lock pair; that registration is the provenance
decision under review, not an approval recorded elsewhere. It authorizes only the pair in
`esbuild-removal-4635.json`. Caller-written manifests cannot authorize other moves.

| Lock | SHA-256 |
| --- | --- |
| Original, main `0e6f9ca36c31b3961b69cd31837431e4dad3768b` (the #4569 tokio replacement's new lock) | `216d99d595688e68db223517fe52048f3618d73d36abbb71ea652c25fec97810` |
| Milestone lock, `base/esbuild-removal` `346efa92734632e17497ca9eab5de7d7ff0a1dfc` | `b4e8f2413e0c8df9df8cd47fb8e555189560ad01bae65d3e9d637d6a51810634` |

The old lock is preserved verbatim in [esbuild-removal-4635.old.lock](esbuild-removal-4635.old.lock) and
the new lock in [esbuild-removal-4635.new.lock](esbuild-removal-4635.new.lock), so the policy tests do not
depend on checkout history. The old lock has 676 package records and the new lock 674. The header, every
non-package section and every external record that remains are byte-identical.

## Record inventory

`esbuild-removal-4635.json` lists each record with its before/after SHA-256 under `inventory`;
`scripts/__tests__/esbuild-removal-transition.test.mjs` recomputes that inventory from the frozen bytes.

| Record | Change | Dependency edges |
| --- | --- | --- |
| `zfb-bundler@0.0.0` | added (internal) | `anyhow`, `oxc`, `rolldown`, `rolldown_common`, `rolldown_resolver`, `serde`, `serde_json`, `tokio` |
| `zfb-compiler-worker@0.0.0` | added (internal) | `hex`, `libc`, `serde`, `serde_json`, `sha2`, `tokio`, `zfb-bundler` |
| `zfb@0.0.0` | changed (internal) | drops `flate2`, `hex`, `tar`, `zfb-binfetch`; adds `zfb-bundler`, `zfb-compiler-worker` |
| `zfb-build@0.0.0` | changed (internal) | drops `zfb-rolldown-prototype`, `zfb-toolchain-pins`; adds `zfb-bundler`, `zfb-compiler-worker` |
| `zfb-config-loader@0.0.0` | changed (internal) | drops `zfb-build`; adds `zfb-bundler`, `zfb-compiler-worker` |
| `zfb-islands@0.0.0` | changed (internal) | drops `zfb-rolldown-prototype`, `zfb-test-utils`, `zfb-toolchain-pins`; adds `zfb-bundler` |
| `zfb-server@0.0.0` | changed (internal) | adds `zfb-compiler-worker` |
| `zfb-rolldown-prototype@0.0.0` | removed (internal; renamed `zfb-bundler`) | |
| `zfb-binfetch@0.0.0` | removed (internal; the esbuild fetcher) | |
| `tar@0.4.45` | removed (external) | |
| `xattr@1.6.1` | removed (external, reached only through `tar`) | |

`zfb-bundler` keeps the `zfb-rolldown-prototype` edge list and gains `serde`, which resolves to the
existing, unchanged `serde` record. No external version, source or checksum changes, and no external
record is added.

None of the existing shape-specific reasons covers this move. `cargo-internal-rewire` refuses removed
records and `cargo-removal` refuses added records, and an intermediate lock between the two would be
hand-assembled rather than reproduced. The exact-byte decision is therefore the same mechanism as the
#4268, #4314 and #4569 pairs. No Cargo producer closure receipts were recorded here, because Cargo was not
run for this record. The native measurements must still prove every raw and `gzipNormalized` total
unchanged.

## Application

Each platform still needs its own native evidence. `research/v3-island-size/README.md` records each
applied platform transition.

- **Linux:** the Actions `health` artifact, through `--reason bundler-backend` (#4635). The esbuild-free
  measurement also moves the runner digest and the bundler identity, so the Cargo-only
  `cargo-producer-lock` reason refuses it.
- **Darwin:** this needs owner-native Mac evidence and a schemaVersion 4 Darwin re-pin path, and neither
  exists yet. The Darwin contract stays pinned to its reviewed schemaVersion 3 identity.

Pass these to `--reason bundler-backend`:

- `--reviewed-cargo-transition research/v3-island-size/cargo-transitions/esbuild-removal-4635.json`
- `--added-packages zfb-bundler@0.0.0,zfb-compiler-worker@0.0.0`
- `--old-cargo-ref 0e6f9ca36c31b3961b69cd31837431e4dad3768b:Cargo.lock`
- the two lock hashes above

Ceilings, allowances and the dependency fingerprint do not move.
