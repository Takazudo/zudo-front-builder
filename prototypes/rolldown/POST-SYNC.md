# Main synchronization verification — 2026-10-15

PR #4120 remains draft and open. This is Linux x86_64 debug-profile evidence,
not a default switch, native Darwin result, release or publication decision.

## Inputs and evidence

Main `fbb7921d8fb3f28d859bbd4cf5737759f547866a` was incorporated by
`557f65e3c5febc651e05bafc8974b8bbb91737bd`. The verification workflow was
added at `21863e45fa920292e9bf48746f667f00ac6980a1`; it changed no producer
source relative to that merge.

[Bounded native verification run 38624226907](https://github.com/Takazudo/zudo-front-builder/actions/runs/38624226907)
completed successfully. Source, Wind and native artifacts were downloaded and
the archive hashes checked before inspecting their reports:

| Artifact | ID | SHA-256 |
| --- | --- | --- |
| Source snapshot | 11923191324 | `c5c4f380d3482dedff4438274e0a4582f99fd34b5bbec0754b2c9639e4379be0` |
| Wind outputs | 11924370779 | `be1283f063892491f5b5b29f161bb57d7842247cb41793c92ebf0fe9f2fb0af4` |
| Native execution | 11922764182 | `c135598d410ffa9348d65ff3b963addd6e4d58b61db4588825ede107f43c24c6` |

Artifacts have seven-day retention; retain the concise findings here and the
original Actions logs. Source/lock identities are distinct from archive hashes:

- Cargo.lock: `63aa8f330710941b8dfc4e5ccad81b0f42e2baf7c6e740ad42802223bc574c43`.
- pnpm-lock.yaml: `ee4accdbbbe22851c41ac529d492fbb6c03b007061ed0042068b2551731b5817`.
- Rust 1.99.0, Node 24.14.0, pnpm 12.8.2, pinned esbuild 0.25.12 and
  Playwright 1.58.2. Exact native Rolldown revision remains `24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9`.

The source archive contained a machine-absolute skill-document symlink. It was
excluded from the local inspection copy rather than followed. Repository writes
use the original Git tree plus explicitly reviewed paths, not that local copy's
tree or its local bookkeeping commit.

## Native and default behavior

The locked opt-in CLI and existing prepared/browser/SSR/production examples
built. The workflow's native/default Rust selections, `gate.mjs`,
`client-scripts.mjs`, `verify.mjs` and `production.mjs` completed successfully.
The downloaded client reports independently confirm:

- native attribution: three production jobs and three dev-edit jobs;
- esbuild controls: zero native jobs in both phases;
- entry plus two declared workers served successfully, zero browser errors,
  watched reloads and executable refreshed assets;
- missing-import and worker-stage-escape cases rejected.

Both downloaded production reports execute the real scratch and adapter outputs:
SSR/catch-all responses agree, Wasm returns 29/43, CJS returns 31, MDX and CSS
modules render, ordinary glue module executes, second-pass determinism holds,
and the scratch throw maps to `pages/throw.tsx:3:9`. The unsupported SSR copied
resource URL remains a limitation, not a passed feature. Existing source-map
stripping at the adapter boundary remains unchanged.

These are fresh Linux results for the named source. Earlier WSL/Darwin results
remain historical. New plugin examples do not turn this into plugin acceptance.

## Reviewed generated-data repair

The Wind sources and root Cargo.toml were compared to incorporated main; Wind
itself and its compatibility catalog are unchanged, but shared dependencies are
not claimed unchanged. All 117 locked Wind library tests and 19 preview-pipeline
tests passed. Regenerating all 173 genuine previews changed only the manifest's
compiler source fingerprint, with no CSS bytes changed.

The 100-path Wind digest was independently recomputed as
`5a08ea9a15e45de6afd6d7adfc6887e217e04d8249290a5ce0a1974b39aaee4c`.
The compiled-preview fingerprint is
`3b25b7c131ea64514dcbff6cbe96798583f5227e67bcbdc2ea36187985f09aad`.
The reviewed support pin and catalog assertions are retained. Both EN/JA
reference freshness checks passed with the repaired provenance.

The prior default WASM job in [Health run 38621899886](https://github.com/Takazudo/zudo-front-builder/actions/runs/38621899886/job/116051335263)
reported changed measured rows after successful build/consumer checks. Its
measurements, not a local rebuild, supply this refresh:

| Artifact | Raw bytes | gzip-9 bytes | Unchanged gzip ceiling |
| --- | ---: | ---: | ---: |
| Root | 3389152 | 1522818 | 1600000 |
| Highlight | 1540963 | 821001 | 880000 |
| Render | 2204415 | 1097797 | 1100000 |
| Parse | 703502 | 285765 | 325000 |

Only measured rows and their eight generated documentation consumers change.
Version stamping, glue rows, ceilings and tolerances remain unchanged. The next
normal CI measurement must confirm these rows; a documentation check is not a
new WASM build. `post-sync-data.mjs` pins the exact eleven before/after file hashes
and refuses other inputs. Its optional object export creates unreferenced Git
blobs/tree only; the manager separately reviews and updates the prototype ref.
It is a one-time repair script, not a general policy-repin API.

Local checks on the actual generated changes: both reference generators,
md-WASM documentation consistency and 19 preview contract tests passed. The
repair script was replayed from the pristine snapshot and rerun idempotently.
No local Rust build or full local workspace test is claimed.

## Remaining integration gate

The original Health root tests failed on Wind provenance and two island-size
Cargo-provenance assertions. The latter reject the actual changed Cargo.lock;
these are not measured numerical overflows. The later broad workspace steps
were skipped. The new bounded native pass does not make that Health run green.

The island-size contracts remain untouched. The Rolldown lock delta is not an
additive-only or internal-rewire-only transition. Do not apply the pnpm-only
reason or change hashes/budgets to manufacture acceptance. A reviewed transition
and qualifying platform evidence remain required; no native Mac dispatch or
live paired re-pin is performed here. Keep that gate distinct from successful
Linux runtime parity and the generated-data repairs.

## Plugin continuation boundary

`plugin_prebundle.rs` and `plugin-prebundle.mjs` are an isolated differential
option probe. They do not modify `bundle_plugin_entry`, the prepared production
adapter, the Node plugin host, or timeout/staged-file ownership. The candidate
bare-external mapping is deliberately tested against tsconfig paths and installed
nested-package resolution; a failing row is a capability gap, not a test to drop.

Successful native `close()` after a build is not proof of active-build
cancellation or no late writes. The pinned `write`/`close` APIs both require a
mutable bundler, and the existing adapter's scoped thread joins synchronously.
No bounded-cancellation contract has been implemented or verified for plugins.
The live plugin caller must remain on esbuild until both semantic and lifecycle
gates are satisfied. No claim is made that upstream could never support it.
