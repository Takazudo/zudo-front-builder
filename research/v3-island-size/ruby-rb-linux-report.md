# Linux x64 island size after `rb` vocabulary support

The `rb` runtime and JSX vocabulary fix changes the emitted client module. The exact Linux x64 contract was refreshed from the full real build, with zero allowance.

## Provenance

- CI run: https://github.com/Takazudo/zudo-front-builder/actions/runs/37235606093, Health job `111533958718`.
- CI merge source SHA: `4abd29569138c97ae746aec967438b6ceac34961`.
- Saved artifact: `ruby-linux-island-size-evidence`, containing workspace and packed output inventories and reports.
- Both modes measured all eight fixtures twice with byte-identical dist inventories. Node `v24.14.0`, zlib `1.3.1-e00f703`, esbuild `0.25.12`, package `3.2.0`.
- pnpm lock SHA-256: `f34fbd2b26d452fb69c92d2b49c6e4e391b5016b947f917e170d8785dcdd12e2`; Cargo lock SHA-256: `695094eb4031b7e0578130ce339ab7cac6c66aedbb0bfb219640a0112d3cc75e`.

| Fixture | Workspace raw | Workspace gzip-9 | Packed raw | Packed gzip-9 |
| --- | ---: | ---: | ---: | ---: |
| no-island | 0 | 0 | 0 | 0 |
| event-only | 63,212 | 21,137 | 63,274 | 21,159 |
| scalar-signal | 63,220 | 21,136 | 63,282 | 21,160 |
| show-for | 63,366 | 21,216 | 63,428 | 21,246 |
| model | 63,211 | 21,142 | 63,273 | 21,173 |
| blog-theme | 63,561 | 21,279 | 63,623 | 21,299 |
| json-api | 63,366 | 21,230 | 63,428 | 21,253 |
| multi-island | 63,488 | 21,238 | 63,550 | 21,263 |

The temporary upload step is removed after preserving this evidence. The terminal sweep topic will remeasure the combined source after the remaining product changes, including the script rawHtml boundary fix.
