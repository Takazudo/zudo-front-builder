# Linux island size after script boundary validation

The shared script rawHtml tokenizer guard ships in the client runtime. The exact Linux x64 contract
is refreshed from complete real workspace and packed builds, with zero allowance.

- Source: `2ffc69d647bf0518c0d449e9a0abd7a473c1fe51` (CI merge ref).
- Evidence: [Health run 37254863769](https://github.com/Takazudo/zudo-front-builder/actions/runs/37254863769), job `111589659768`, artifact `bounded-fixes-linux-island-size-evidence`.
- All eight fixtures in both modes completed two byte-identical dist inventories.
- Toolchain: package 3.2.0, Node v24.14.0, zlib 1.3.1-e00f703, esbuild 0.25.12, Hono 4.12.25.
- pnpm lock SHA-256: `f34fbd2b26d452fb69c92d2b49c6e4e391b5016b947f917e170d8785dcdd12e2`; Cargo lock SHA-256: `695094eb4031b7e0578130ce339ab7cac6c66aedbb0bfb219640a0112d3cc75e`.
- Native binary SHA-256: `27e16d19c53e3ee6b192473351fe3112efd3e17b495470cb1fb7d11b415e87e2`.
- SDK/runtime tarball SHA-256: `f2c27baf8a2ecef255b92059add89f1d7d8c94dffe5f7d50452b7cc6c2eb9184` / `efe1fa9040d38417c1839dcac2b96a6d0c62c485c30c2cda177fb2dda0bf420f`.

| Fixture | Workspace raw | Workspace gzip-9 | Packed raw | Packed gzip-9 |
| --- | ---: | ---: | ---: | ---: |
| no-island | 0 | 0 | 0 | 0 |
| event-only | 64,661 | 21,548 | 64,723 | 21,571 |
| scalar-signal | 64,669 | 21,546 | 64,731 | 21,573 |
| show-for | 64,815 | 21,625 | 64,877 | 21,660 |
| model | 64,660 | 21,548 | 64,722 | 21,583 |
| blog-theme | 65,010 | 21,689 | 65,072 | 21,711 |
| json-api | 64,815 | 21,637 | 64,877 | 21,665 |
| multi-island | 64,937 | 21,650 | 64,999 | 21,673 |

Every island fixture adds 1,449 raw bytes over the rb-only measurement. The no-island fixture still
ships no JavaScript. This is the measured cost of the shared boundary validation; no allowance was
added. The temporary artifact step is removed after capture. Darwin arm64 has an independent final
measurement gate, and the final inherited source remains subject to the regular Linux CI check.
