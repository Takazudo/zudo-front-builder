# Darwin arm64 island size after SWC 74

This is an intermediate measurement for the SWC smartstring epic. The terminal sweep epic will measure again after the remaining client and compiler changes.

## Provenance

- Source SHA: `69030f99a4105957bdda0ffdb3a3a26381d739ec`.
- Host: Darwin arm64; Node `v24.14.0`, zlib `1.3.1-e00f703`, esbuild `0.25.12`, Hono `4.12.25`, package `3.2.0`.
- Lockfiles: pnpm `f34fbd2b26d452fb69c92d2b49c6e4e391b5016b947f917e170d8785dcdd12e2`; Cargo `695094eb4031b7e0578130ce339ab7cac6c66aedbb0bfb219640a0112d3cc75e`.
- Fresh native binary SHA-256: `a2cd08b9ab977fcbfa9421c9088dc21feb428f3557c348c8ebc40c4aeffa9a47`; `cargo build -p zfb` passed under the heavy guard with the repository's single-job settings on the second run. The first guarded build reached its maximum runtime during compilation without a source error.
- Packed tarball SHA-256: zfb `5430c4d5aecb79af81b9efc8db80e0448cdc7a152f3c7896f83aea820611be60`; zfb-runtime `d94b3876f917b56edca41f20af5722dc652724118d84c4c1d342450edf5611f5`.
- Saved local outputs, ignored by Git: `worktrees/_manager/darwin-size-69030f99/real-workspace/`, `real-packed/`, `modeled-workspace/`, and `modeled-packed/`. Each contains a report and measurement JSON; real outputs also contain both complete `dist` inventories for every fixture. The modeled outputs are attribution sidecars, not shipped-size evidence.

## Real shipped JavaScript bytes

| Fixture | Workspace raw | Workspace gzip-9 | Packed raw | Packed gzip-9 |
| --- | ---: | ---: | ---: | ---: |
| no-island | 0 | 0 | 0 | 0 |
| event-only | 63,209 | 21,133 | 63,271 | 21,156 |
| scalar-signal | 63,217 | 21,133 | 63,279 | 21,160 |
| show-for | 63,363 | 21,213 | 63,425 | 21,244 |
| model | 63,208 | 21,138 | 63,270 | 21,168 |
| blog-theme | 63,558 | 21,278 | 63,620 | 21,297 |
| json-api | 63,363 | 21,227 | 63,425 | 21,253 |
| multi-island | 63,485 | 21,239 | 63,547 | 21,258 |

Each workspace and packed fixture produced byte-identical inventories across two passes. Both full real matrices passed the heavy guard. With `decision.json` set to these exact totals and zero allowance, `check-budget.mjs` passed against the saved workspace and packed outputs on the measurement SHA. The focused budget test passed all 16 assertions locally.

The prior Darwin contract came from an older 3.1.0 source SHA and was materially stale; these totals describe the current 3.2.0/SWC 74 checkout. No portion of the increase from that older baseline is attributed solely to SWC. Linux x64 has its own independent measurement and contract. The next sweep epic must remeasure the combined final source rather than carrying these interim totals forward without evidence.
