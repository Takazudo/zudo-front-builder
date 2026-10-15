# #318 optional Rolldown lock provenance revalidation

The optional prototype changed root `Cargo.lock`, invalidating the reviewed size contracts' input provenance. This follow-up measures the **normal esbuild path**; the separate prototype timing experiment is not the shipped-size authority. No ceiling, allowance, fixture, runner, or budget-check assertion is changed.

Measured source: `b40a06d3671ca308fe807625a6e64b0df45454bc`. Cargo lock SHA-256: `8c2862b6e27faedcbe37b4f71bd05d83c845acd65cae7ba286a396438eb264d6`. pnpm lock remains `113d2ec03d688fe05d573b1c4a56b97a55647ccae86b1416b84e8c97e04d9b8d`.

## Linux x64

One targeted `cargo build --locked -p zfb` reused the manager's compiled dependencies. The first attempt failed downloading esbuild; a retry with the already installed `ZFB_ESBUILD_BIN` 0.25.12 passed in 11.20 seconds. SDK/runtime/slugify were built and packed from the unchanged pnpm lock. No package was published.

Following [the existing procedure](README.md), `measure-real.mjs` ran both complete eight-case matrices, with two saved full-dist passes per case, using Node **24.14.0**, zlib **1.3.1-e00f703**, pnpm **12.8.2**, Hono **4.12.25**, esbuild **0.25.12**, and SDK/runtime/slugify **4.2.1**. The shared heavy guard reported **PASS, 32 seconds**. Every repeated full-dist inventory matched.

The original budget check reported only these three errors:

```text
local Cargo.lock: SHA-256 differs from the reviewed contract
workspace provenance: Cargo lockfile differs from the contract
packed provenance: Cargo lockfile differs from the contract
```

After refreshing only the Linux contract's Cargo lock digest and explanatory status, the unchanged strict checker reported:

```text
Island size budget passed for all eight fixtures in workspace and packed modes.
```

All raw and gzip totals exactly equal the existing ceilings. Original baseline source/evidence and zero allowances remain intact.

Local retained artifacts are under ignored `target/island-size/`: `real-workspace/` and `real-packed/` contain `measurement.json`, `report.md` and all 32 saved dist trees; `pack/` contains the three package tarballs. The tarballs and binaries are intentionally not committed.

| Artifact | SHA-256 |
| --- | --- |
| CLI | `72986683bc2e79217066ca91c262668c080f9bb8ff2cfe545970693b881862eb` |
| esbuild binary | `bab29b2ca7a9e89b67cf720b77b2d743f9f31f5cf0d5bd74ee8c8de30ced7014` |
| zfb tarball | `09cced822d27cac4e67b00773ca1ab5f6a10cb203704a569f2779da5ea2724f1` |
| runtime tarball | `a7469b7464b35b585c25ae53f24484faae5cd6b9f68c8c7e73f86d4a35124c83` |
| slugify tarball | `be41c9e7c819405a50bb3d6106e569149aad63fa1359031d03c34aab9020e2f6` |
| workspace measurement JSON | `6b000779dcf7af9231c410d071fc400f2b9e7834924bef13ac67939787c5f9ec` |
| packed measurement JSON | `082be96113842a27c44b1bf757447d78ad465adcbf48008aaf9e366839b61817` |
| Hono package manifest | `272ab6b5a27aec8f30fac242dfec9b87dcf69015411affcca7e1c193eab11993` |

## Darwin arm64

The existing native measurement workflow was dispatched with exact `ref=b40a06d3671ca308fe807625a6e64b0df45454bc` and `enforce=false`: [run 38023402745](https://github.com/Takazudo/zudo-front-builder/actions/runs/38023402745). It performs the same complete matrices on the pinned macos-15 / Node 24.14.0 toolchain and retains the native artifacts. Linux values are not substituted for Darwin measurements.

The native build, package builds and both repeated matrices completed. The original native budget check rejected **only the three Cargo-lock provenance mismatches quoted above**; it found no size or saved-byte mismatch. The workflow conclusion is success because `enforce=false`; that is not described as an original budget-check pass.

Artifact **11659876342**, `island-size-darwin-arm64-b40a06d3671ca308fe807625a6e64b0df45454bc`, is retained for 30 days by that run. Its downloaded ZIP SHA-256 `f7f4541f6aaa818ebe80a939b8c4c66cc210373a97ae6b6185b3a3c2dda6f7be` matches GitHub's artifact digest. Local extraction is under `target/island-size/darwin-evidence/`. The native runner recorded macOS 15.7.9 arm64, Rust/Cargo 1.99.0, one Cargo job, default development debug information and the same pinned Node/zlib/pnpm/Hono versions as Linux.

An independent artifact replay on Linux, using pinned Node 24.14.0, recomputed SHA-256 for all **92 saved full-dist files**, rehashed/recompressed all **28 saved JS chunks**, verified all three package tarballs, and compared all **16 mode/case totals** against the unchanged Darwin contract. All passed and every raw/gzip total exactly equaled its existing ceiling. This is byte/artifact validation of native measurements, not a claim that a Linux process ran natively on Darwin.

Only `toolchain.cargoLockSha256` and explanatory `platformStatus` change in each platform contract. The original baseline source/evidence, every ceiling, zero allowance, fixture and assertion remain unchanged. The existing 17 size-budget tests and 22 Wind-reference tests pass after these metadata refreshes (**39/39**). The strict Linux saved-artifact checker also passed after the hash refresh; the original Darwin check's failed status is preserved above.

Darwin CLI SHA-256: `75e4055937fc9648455ee6d7378aa84b92c4ae03c4ed929749579fb4c598a435`; esbuild binary: `3e030ee2aa86ad3c33e5e95ae0e53bb03de40e0da35c9b1180a67de4a497cae5`. All three package tarball digests match the Linux digests in the table. No native prototype backend was enabled in either platform's size run.

## Linux observed totals

| Mode | Case | Raw | Gzip |
| --- | --- | --- | --- |
| workspace | no-island | 0 | 0 |
| workspace | event-only | 66792 | 22249 |
| workspace | scalar-signal | 66800 | 22256 |
| workspace | show-for | 66949 | 22322 |
| workspace | model | 66791 | 22249 |
| workspace | blog-theme | 67144 | 22374 |
| workspace | json-api | 66950 | 22335 |
| workspace | multi-island | 67072 | 22341 |
| packed | no-island | 0 | 0 |
| packed | event-only | 66854 | 22267 |
| packed | scalar-signal | 66862 | 22284 |
| packed | show-for | 67011 | 22342 |
| packed | model | 66853 | 22272 |
| packed | blog-theme | 67206 | 22410 |
| packed | json-api | 67012 | 22360 |
| packed | multi-island | 67134 | 22365 |
