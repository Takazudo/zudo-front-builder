# Research Notes Index

This directory keeps research notes only while they remain useful to the
shipped code or docs. Keepers need a concrete `shipped-in` or
`referenced-from` anchor; one-shot probes should be deleted once their
findings are no longer load-bearing.

## Code-referenced keepers

| File | Status | Shipped-in / referenced-from |
|---|---|---|
| `1229-dev-staging-decision.md` | `shipped-in` | Injected package-route dev staging; referenced from `crates/zfb/src/commands/package_routes.rs`. |
| `1284-dev-dep-invalidation.md` | `shipped-in` | Dev dependency-invalidation fixes and tests; referenced from `crates/zfb-*` dev invalidation tests and implementation comments. |
| `344-v8-feature-gate.md` | `referenced-from` | V8 feature-gate rationale; referenced from `crates/zfb/src/config.rs` and `crates/zfb/src/commands/build.rs`. |
| `346-embed-as-library-api.md` | `shipped-in` | `zfb-server` embed API and middleware shape; referenced from `crates/zfb-server/README.md`, `src/embed.rs`, and `src/middleware.rs`. |

## Purgeable one-shots

Deleted during issue #1469:

- `347-routes-json-manifest.md`
- `348-recipes-catalog.md`
- `349-ccresdoc-probe.md`
