# Exact noyalib 0.0.58 adoption

This is the reviewed `Cargo.lock` replacement for [#4268](https://github.com/Takazudo/zudo-front-builder/issues/4268). On 2026-10-21 Takazudo chose to adopt the published noyalib / noyalib-serde-yaml 0.0.58 pair, accepting its WASM size growth and frontmatter slowdown (see the `noyalib 0.0.58` section of `DEPENDENCIES.md`). It authorizes only the lock pair in `noyalib-4268.json`: noyalib 0.0.55 → 0.0.58 and noyalib-serde-yaml 0.0.55 → 0.0.58, version and checksum only. `scripts/security-lock-transitions.mjs` pins the decision bytes and both complete lock hashes next to the #4314 security replacements, and the shared reviewed-transition entry point dispatches it from there. Caller-written manifests cannot authorize other replacements.

| Lock | SHA-256 |
| --- | --- |
| Original, main `84dc4a842bee059af334a02bfd453d2ce2c74871` | `d51febae86251fc5b354d81b9505aea21841661150105476029fcc3e6327766d` |
| Approved minimal replacement | `c0d708895c54e312b0f5f9d90ee67b5d1037b9a667ece59d475791a5619e5b64` |

Both locks contain 697 package records. Neither package changes its dependency list.

| Crate | New checksum |
| --- | --- |
| noyalib 0.0.58 | `3fad8f1a552070f6ba5f60f25e880f7b6daeda8fe138f589345141d31a21a3bb` |
| noyalib-serde-yaml 0.0.58 | `c7ef9e69591faecd546d11575c1f65c4c5f6725692cd82c6280eae4c1a923696` |

Cargo 1.97 verified both archives against these checksums while downloading for `cargo metadata --locked`.

`cargo update -p noyalib-serde-yaml --precise 0.0.58` also reselected Windows edges, as it did for the #4327 probe. Those incidental edges are **not authorized**. The minimal version/checksum edit passes `cargo metadata --locked` without rewriting the lock.

## Application

Each platform still needs its own native evidence:

- **Linux:** the Actions `health` artifact.
- **Darwin:** owner-attested local evidence through the `cargo-producer-lock` local profile, which accepts exactly the approved decisions in `security-lock-transitions.mjs`.

Select this decision with `--reason cargo-producer-lock`. Pass:

- `--reviewed-cargo-transition research/v3-island-size/cargo-transitions/noyalib-4268.json`
- `--added-packages noyalib-serde-yaml@0.0.58,noyalib@0.0.58`
- `--old-cargo-ref 84dc4a842bee059af334a02bfd453d2ce2c74871:Cargo.lock`
- the two lock hashes above

Ceilings, allowances and the dependency fingerprint do not move.
