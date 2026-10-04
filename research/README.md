# Research Notes Index

This directory keeps research notes only while they remain useful to the
shipped code or docs. Keepers need a concrete `shipped-in` or
`referenced-from` anchor; one-shot probes should be deleted once their
findings are no longer load-bearing.

## Code-referenced keepers

| File                                 | Status            | Shipped-in / referenced-from                                                                                                         |
| ------------------------------------ | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| `1229-dev-staging-decision.md`       | `shipped-in`      | Injected package-route dev staging; referenced from `crates/zfb/src/commands/package_routes.rs`.                                     |
| `1284-dev-dep-invalidation.md`       | `shipped-in`      | Dev dependency-invalidation fixes and tests; referenced from `crates/zfb-*` dev invalidation tests and implementation comments.      |
| `344-v8-feature-gate.md`             | `referenced-from` | V8 feature-gate rationale; referenced from `crates/zfb/src/config.rs` and `crates/zfb/src/commands/build.rs`.                        |
| `346-embed-as-library-api.md`        | `shipped-in`      | `zfb-server` embed API and middleware shape; referenced from `crates/zfb-server/README.md`, `src/embed.rs`, and `src/middleware.rs`. |
| `1638-resource-delivery-decision.md` | `referenced-from` | Browser resource-delivery contract for md-wasm; implementation issues #1639, #1640, and #1641.                                       |
| `1898-parse-to-ast-interoperability-contract.md` | `referenced-from` | Raw `parseToAst` tier contract; implementation issues #1902, #1904, #1906, #1907, and #1908.                             |
| `2013-request-time-capability-contract.md` | `referenced-from` | Request-time `fetch` + Web Crypto contract for the embedded V8 host; locks epic #2012's sub-issues #2014-#2020. |
| `2036-styled-404-detection-diagnosis.md` | `referenced-from` | Why the styled 404 page is never substituted (the `content-length` conjunct in `assetHasStyled404Body`); locks epic #2035's sub-issues #2037 and #2038. |
| `3242-owned-engines-ledger.md` | `referenced-from` | Epic #3242 decisions, measured inventory and test dispositions; referenced by epic #3242 and sub-issue #3244. |
| `3242-owned-engines-baseline.md` | `referenced-from` | Owned-engine baseline and migration inputs; referenced by epic #3242 and sub-issue #3245. |
| `3242-zudo-wind-v1-spec.md` | `referenced-from` | zudo-wind v1 language contract; referenced by epic #3242 and sub-issue #3246. |
| `3242-zudo-react-v1-contract.md` | `referenced-from` | zudo-react v1 runtime contract; referenced by epic #3242 and sub-issue #3247. |
| `3242-v3-release-notes-material.md` | `referenced-from` | Breaking-change and migration material; referenced by epic #3242 and sub-issue #3305. |
| `3242-owned-engines-completion-report.md` | `referenced-from` | Owned-engine completion record; referenced by epic #3242 and sub-issue #3306. |
| `3318-scratch-dir-design.md` | `referenced-from` | Scratch-dir + per-invocation `--define` contract; locks epic #3339's sub-issues #3341-#3348. |
| `swc-smartstring-verification.md` | `referenced-from` | SWC 74 migration verification evidence for issue #3636. |

## Purgeable one-shots

Deleted during issue #1469:

- `347-routes-json-manifest.md`
- `348-recipes-catalog.md`
- `349-ccresdoc-probe.md`
