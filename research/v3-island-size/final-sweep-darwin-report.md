# Final sweep Darwin arm64 island sizes

Source: `35bee65b022645286aac6d9778c434c99a6ad21a` (PR #3695 CI merge; head `5364b6019ede31cecf2e8f15c0b4eb6b481837f2`).

[Measurement run 37259456004](https://github.com/Takazudo/zudo-front-builder/actions/runs/37259456004) built the combined SWC, Ruby rb, script boundary, Wind and package-graph fixes on macos-15 arm64. Artifact: `bounded-fixes-darwin-island-size-evidence`.

Both modes include all eight fixtures and two byte-identical full-dist inventories each. The original job completed every measurement and failed only when the older SWC-only contract rejected the new totals. The refreshed contract uses these actual Darwin observations with zero allowance; Linux values were not substituted.

Toolchain: package 3.2.0, Node v24.14.0, zlib 1.3.1-e00f703, esbuild 0.25.12, Hono 4.12.25, pnpm 12.8.2, Rust 1.99.0. Native build: Mach-O arm64, one Cargo job, incremental off, default debug info; pre-build free disk 41.3 GiB.

| Fixture | Workspace all shipped raw | Workspace all shipped gzip-9 | Packed all shipped raw | Packed all shipped gzip-9 |
| --- | ---: | ---: | ---: | ---: |
| no-island | 0 | 0 | 0 | 0 |
| event-only | 64,661 | 21,548 | 64,723 | 21,571 |
| scalar-signal | 64,669 | 21,546 | 64,731 | 21,573 |
| show-for | 64,815 | 21,625 | 64,877 | 21,660 |
| model | 64,660 | 21,548 | 64,722 | 21,583 |
| blog-theme | 65,010 | 21,689 | 65,072 | 21,711 |
| json-api | 64,815 | 21,637 | 64,877 | 21,665 |
| multi-island | 64,937 | 21,650 | 64,999 | 21,673 |

For every nonempty fixture, initial raw/gzip equals the all-shipped totals in the table and later raw/gzip is 0/0. No-island ships no JS. Every nonempty fixture adds 1,452 raw bytes over the intermediate SWC-only Darwin baseline: 3 bytes from rb support and 1,449 from script boundary validation. These totals independently match the current Linux matrix; platform provenance remains separate.

The real workspace report is `worktrees/_manager/final-darwin-size-evidence/real-workspace/report.md`; the packed report is `worktrees/_manager/final-darwin-size-evidence/real-packed/report.md`. Modeled workspace and packed sidecars were not regenerated for this final source, so there are no current sidecar report paths or attribution claims. The `measure.mjs` output is supplementary diagnostics and does not set the shipped-size contract.

## Provenance

- Native binary SHA-256: `9817fcb76bc70552f0b5ff0049e7df695c10e011b89f9e24e71acc0d1c7f1556`.
- esbuild binary SHA-256: `3e030ee2aa86ad3c33e5e95ae0e53bb03de40e0da35c9b1180a67de4a497cae5`.
- SDK tarball SHA-256: `f2c27baf8a2ecef255b92059add89f1d7d8c94dffe5f7d50452b7cc6c2eb9184`.
- Runtime tarball SHA-256: `efe1fa9040d38417c1839dcac2b96a6d0c62c485c30c2cda177fb2dda0bf420f`.
- Hono 4.12.25 package manifest SHA-256: `272ab6b5a27aec8f30fac242dfec9b87dcf69015411affcca7e1c193eab11993`.
- pnpm lock SHA-256: `f34fbd2b26d452fb69c92d2b49c6e4e391b5016b947f917e170d8785dcdd12e2`.
- Cargo lock SHA-256: `695094eb4031b7e0578130ce339ab7cac6c66aedbb0bfb219640a0112d3cc75e`.

## Verification

The unchanged strict checker passed for all eight fixtures in both modes against a portable copy of the saved matrix in a detached checkout at the measured source SHA, with the refreshed contract. Only absolute package/tarball paths are relocated to local files with matching recorded hashes; source SHA, platform, hashes, sizes, and inventories remain unchanged. Full packs/dist artifacts remain outside git. The temporary CI executor is removed.
