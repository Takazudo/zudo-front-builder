# #4172 result: noyalib 0.0.55 is MIGRATE-ELIGIBLE

The exact `noyalib` / `noyalib-serde-yaml` 0.0.55 pair passes every compatibility, resource and API gate that the shipping 0.0.53 pair passes. It stays under every unchanged hard artifact ceiling on CI. The owner's candidate-specific exception (#4161, 2026-10-12) covers the discretionary growth. Production application belongs to #4173.

## Evaluated state

- Source: `7e1558d64e274954af19f528a9c4f40210da7dcc` (`base/sweep-261012-reliability-yaml` with this harness).
- Host: macOS 26.6.1 arm64. Native lanes ran Homebrew rustc/cargo 1.94.0; WASM ran rustup stable 1.96.0, wasm-bindgen 0.2.121, wasm-opt 130, Node v24.14.0, pnpm 12.8.2. Both lanes used the same toolchain.
- Evidence dir: `$HOME/.cache/zfb-4172-yaml-evidence/20261012T164259Z` (local; `result.json` is copied next to this report).
- Run: `compare.sh` on 2026-10-12, 16:42–18:02 UTC. Summarized with `dispositions.json` and the CI candidate log below.

## Pair provenance

See `provenance.json`. Both pairs are non-yanked, and their checksums match crates.io and the GitHub Release SHA-256s. MSRV (1.86.0), license, features, normal dependencies and `build.rs` are unchanged. The only manifest addition is a dev-only `jsonschema`. Resolving the 0.0.55 pin changes exactly the two pair entries in `Cargo.lock` (594 packages).

## Gates (both lanes)

| Gate | Control 0.0.53 | Candidate 0.0.55 |
| --- | --- | --- |
| Immutable 18-observation YAML differential harness | pass | pass |
| Frontmatter resource budgets | pass | pass |
| `zfb-content` error messages and full content tests | pass | pass |
| md-wasm `api` (33) / `parse_to_ast` (25) | pass (rerun) | pass (rerun) |
| WASM build, npm test, exports, pack, packed, release | pass | pass |
| Strict md-wasm budget against the CI-measured manifest | exit 1 (expected: Mac bytes are not CI bytes) | exit 1 (same) |

**The `md-api-ast` rerun.** The first run failed `version_falls_back_to_cargo_pkg_version_in_dev_builds` in **both** lanes. This was a harness defect: `compare.sh` exported `ZFB_RELEASE_VERSION` into native tests, and that test asserts the unstamped dev fallback. The step was rerun per lane at the same source SHA with saved manifests and `env -u ZFB_RELEASE_VERSION`. The original logs are kept as `md-api-ast.leaked-env.*` and `result.pre-rerun.json`. `compare.sh` now scopes the variable away from that step.

## Frontmatter probe diffs

All diffs are documented 0.0.55 changes. None changes the value of a previously accepted document; each is a refusal, a new acceptance, or different error text. Dispositions are in `dispositions.json`.

| Case | Change | Disposition |
| --- | --- | --- |
| `verbatim-int` (`v: !<int> 1`) | newly refused (YAML 1.2.2: verbatim names the URI exactly) | accepted, but must be called out in the zfb and zfb-md-wasm changelog lanes |
| `verbatim-core-uri-int` / `-str` (`!<tag:yaml.org,2002:int\|str> 7`) | newly accepted | accepted |
| `alias-depth-over-max` | newly refused (alias deeper than `max_depth` 128) | accepted (resource-safety tightening) |
| `flow-seq-key-1025-chars`, `flow-seq-key-30000-chars`, `flow-pair-in-seq-1050` | error text only (both pins refuse) | none needed |

No frontmatter in `docs/`, `crates/zfb/templates/` or `tests/` uses verbatim tags.

## Long-flow-line evidence

The long-flow-line case runs through `zfb_content::frontmatter::extract`, the real `serde_yaml::from_str::<serde_json::Value>` path, under an unchanged parser configuration. The improvement is demonstrated:

- **Mapping-value shape:** 0.0.53 grows roughly quadratically (log-log slope 1.82–2.00). 0.0.55 is near-linear (1.03 / 0.96).
- **At 240,000 rows:** 4.23 s → 0.074 s, about 57× faster.
- **Refusal above `max_nodes`:** 400k and 800k scalars now take 49–52 ms, against 9.3–23.9 s on 0.0.53.
- **Document-start shape:** the same pattern (slope 1.91 → 1.01).

Timing is evidence only and is not a CI threshold.

## Sizes

These are CI shipping-relevant numbers from the never-merged candidate branch `scratch/4172-noyalib-055-ci-probe`, head `560f7bee`, [health run 38279381185](https://github.com/Takazudo/zudo-front-builder/actions/runs/38279381185), wasm-md (default) job 114912721041.

| Artifact | 0.0.53 documented gzip-9 | 0.0.55 CI gzip-9 | Δ | Hard ceiling |
| --- | --- | --- | --- | --- |
| render-only | 1,094,209 | 1,094,951 | +742 | 1,100,000 |
| default | 1,519,893 | 1,521,735 | +1,842 | 1,600,000 |
| parse-only | 284,532 | 285,917 | +1,385 | 325,000 |
| highlight-only | unchanged | 817,815 | 0 | 880,000 |

- **Same-toolchain local deltas (Mac):** render +1,043, default +1,245, highlight −7. The local tarball is 3,818,260 B against a 3,900,000 B ceiling (+3,911 B).
- **CI tarball:** not measured by that run. #4173 must obtain it from the authoritative CI lane.
- **Discretionary rules:** default and parse growth exceeds the 1,024 B threshold, and render grows. The owner waived both for this exact pair. That is not a ceiling change.

## Decision and follow-up

**MIGRATE-ELIGIBLE.** #4173 applies the pin. Its obligations:

- The exact `=0.0.55` pin and the two-entry lock delta.
- Re-pin the Wind source digest and both island-size contracts' `cargoLockSha256`, each backed by its own platform measurement. Linux comes from CI; Darwin needs a native enforced run.
- CI-derived `shipped-sizes.json` and the synchronized doc tables.
- A DEPENDENCIES.md record.
- A changelog call-out of the `!<int>`-style verbatim-tag refusal.

0.0.56 and 0.0.57 stay out of scope.
