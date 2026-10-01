# V3 island size baseline (#3468)

Measured product source: `897f288c013e24ff4fde01483a55d8a1945fc964` on `base/v3-decisions`; package versions `@takazudo/zfb@3.0.0` and `@takazudo/zfb-runtime@3.0.0`. This is a controlled **current-source** baseline, not a reproduction of the historical #3383 v2/v3 recipes. The source includes the prerequisite server-rendering fix for duplicated Show/For module instances discovered by the real consumer run. No size optimization has been applied in this report.

## Shipped JS from real `zfb build`

| Case | Workspace raw / gzip | Packed raw / gzip | Packed initial | Packed later |
| --- | ---: | ---: | ---: | ---: |
| no-island | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| event-only | 49,830 / 17,004 | 49,892 / 17,027 | 49,892 / 17,027 | 0 / 0 |
| scalar-signal | 49,838 / 17,009 | 49,900 / 17,037 | 49,900 / 17,037 | 0 / 0 |
| show-for | 49,985 / 17,082 | 50,047 / 17,111 | 50,047 / 17,111 | 0 / 0 |
| model | 49,829 / 17,012 | 49,891 / 17,045 | 49,891 / 17,045 | 0 / 0 |
| blog-theme | 50,180 / 17,138 | 50,242 / 17,166 | 50,242 / 17,166 | 0 / 0 |
| json-api | 49,984 / 17,096 | 50,046 / 17,127 | 50,046 / 17,127 | 0 / 0 |
| multi-island | 50,106 / 17,103 | 50,168 / 17,132 | 50,168 / 17,132 | 0 / 0 |

Each of the eight workspace and packed consumer builds was run twice. The runner compared SHA-256 inventories of **every file** in `dist`, including HTML, and failed on any difference. Both paths passed. No-island emitted no islands JS. Every island case emitted one entry and no shared or lazy chunks, so initial equals all shipped and later is zero. The reported gzip size sums per-file `node:zlib` `gzipSync` at level 9, `mtime: 0`; actual HTTP compression may vary.

| Case | Packed real entry | Raw | Gzip | SHA-256 |
| --- | --- | ---: | ---: | --- |
| event-only | `assets/islands-ee166548.js` | 49,892 | 17,027 | `ee166548706a55bc6bfd24679271c55196cef612fa292968f9260736f0a15cd1` |
| scalar-signal | `assets/islands-9865d925.js` | 49,900 | 17,037 | `9865d9250672d851c5a96be799268782b14b85c1281db0c8594211392e6d1792` |
| show-for | `assets/islands-26995749.js` | 50,047 | 17,111 | `269957496c715fd41a9fb176f431a1eb91ebd1f12b69993ed878826d2d2eb083` |
| model | `assets/islands-83d45382.js` | 49,891 | 17,045 | `83d45382ceb9b00dcab9d79c7705af60a9ce2429b0fbb8d8a1f857aa5e2533de` |
| blog-theme | `assets/islands-f0cad77c.js` | 50,242 | 17,166 | `f0cad77c76bb57ef53e345a8bb3dce53f15d6367b156159d68d5cbf0592441d9` |
| json-api | `assets/islands-6e8dbc5b.js` | 50,046 | 17,127 | `6e8dbc5bc2dda6441c57402d7f25f838aed78ba3d4994599b75bdbb4ef02659e` |
| multi-island | `assets/islands-af50e300.js` | 50,168 | 17,132 | `af50e300d1bcf09d2f2b7743c3f6afc256e9a48189ae5ef5cbe296ce21eaf4e4` |

The two-island scalar-plus-theme fixture ships **268 raw / 95 gzip bytes** more than scalar alone in packed mode (workspace: 268 / 94). The single entry deduplicates the shared runtime. Packed builds are consistently 62 raw bytes above workspace builds for each nonempty case; gzip differences are 23–33 bytes. Their module resolution paths differ, so use packed results for a published-consumer gate.

## Provenance and replay

- `@takazudo/zfb` tarball SHA-256: `85a92e2958d538d5831348e58094f1319740dbfcd826a3ee58c6a35ef6c8f93d`; `@takazudo/zfb-runtime` tarball SHA-256: `5d8cb3fec9990118a6d69a32f2c38a6719fc83c40318f31f7e674776848a3acd`. Packed consumers linked their extracted packages, including `dist/zudo-react/client.js` and `dist/server.js`.
- Lockfile-installed `hono@4.12.25` was linked into both consumer modes: package manifest SHA-256 `272ab6b5a27aec8f30fac242dfec9b87dcf69015411affcca7e1c193eab11993`. The packed runtime declares `^4.12.25`.
- Rust `zfb` CLI binary SHA-256: `87e048cc0925adc9718b44803a7a524a933e5c4d3e71e29b952cbbb9c3a21e93`. Its fresh guarded `cargo build -p zfb` passed after one environment timeout and retry. Source TS packages were rebuilt and repacked at the recorded source. The runner checks the SHA syntax and records binary/package digests; it cannot cryptographically prove build provenance from the SHA alone.
- esbuild `0.25.12` binary SHA-256: `3e030ee2aa86ad3c33e5e95ae0e53bb03de40e0da35c9b1180a67de4a497cae5`. Node `v24.14.0`, zlib `1.3.1-e00f703`. `pnpm-lock.yaml` SHA-256 `69f78b69412b6aaff5dfffa53c37fa15fe54e24da713f583f0ecdd0dabdddbfc`; `Cargo.lock` SHA-256 `43326bc0848833aef50bc2df736f502a1052798bca5e199bc5758e7aaef5b60a`.
- Exact install/build/pack/replay commands and fixture definitions are in [README.md](./README.md). Ignored local outputs: `results/real-workspace/`, `results/real-packed/`, `results/modeled-workspace/`, and `results/modeled-packed/`, each with `measurement.json` and `report.md`. Real pass directories preserve emitted HTML and JS; modeled pass directories preserve esbuild `metafile.json`. Re-run the README commands to regenerate them. Fixture SHA-256 values are recorded in both modeled JSON reports.

## Reachability diagnostics from esbuild sidecar

The sidecar reproduces production minification, browser platform, automatic JSX, production/dev defines, splitting, `--keep-names`, and registration semantics, but its entry path and build identity differ from the real CLI. Raw bytes match the real packed bundle for each case; gzip can differ by a few bytes. Use the real CLI table above for the baseline. esbuild `bytesInOutput` is attribution within a bundled output, **not** independent savings from removing a module.

For the packed event-only sidecar, major retained inputs in its `islands.js` output are:

| Input | Retained bytes in output |
| --- | ---: |
| hydrate (`zudo-react/hydrate.js`) | 19,403 |
| forms (`zudo-react/forms.js`) | 7,402 |
| island runtime (`dist/runtime.js`) | 4,955 |
| vocabulary (`zudo-react/vocabulary.js`) | 4,463 |
| structure (`zudo-react/structure.js`) | 939 |

The event-only, scalar, Show/For, and model outputs all retain nearly the same runtime/forms/structure input set. Their packed raw totals vary by only 0–155 bytes from event-only even though the fixture code differs. This shows that the current hydration import graph retains structural and form code for the simplest island; it does **not** establish that these features can be separated safely. The scalar sidecar without global `--keep-names` was 2,815 raw / 1,157 gzip bytes smaller, but that flag affects identity and error behavior across the entire graph and is not an approved optimization.

The measured sidecar metafiles are generated without changing the bundler's private resource metafile, resource read-back, or stage-escape audit. The next decision task (#3469) will rank bounded options, set numeric acceptance, and update #3470/#3471 before implementation.
