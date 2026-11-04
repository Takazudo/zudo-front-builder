# Darwin arm64 v5.2.0 release-bump stage and apply (2026-11-04)

This stage re-pins the Darwin contract from v5.1.5 to v5.2.0 with `--reason release-bump`, at main `54054042`. The Darwin contract still recorded package 5.1.5 at `d04ef461` while main and the Linux contract were at 5.2.0; this is a provenance catch-up, not a CI failure. Claude Code ran it on the owner's MacBook Neo, and the owner authorized this exact application on 2026-11-04 in the same session. The records are on #4576 and #4276.

## Inputs

| Field | Value |
| --- | --- |
| Source | the clean repository checkout at `54054042e5706cfa0d8c365913a8372fb7e864a1`, Cargo.lock `216d99d5…7810` (unchanged), pnpm-lock `5cce7d76…277d`, package 5.2.0, `decision.json` `ed3ffdcd…ecb7` |
| Local receipt | SHA-256 `a0dd8e36ce96713614da90d84367f1224c90d2f5cc208d750c02bc50c0fca934` (38 commands; operator: the agent, owner attestation given by the application approval) |
| Bundle | `localBundleDigest` `de4ebd44a76e42f8239f7c4e9d75f4240d29eb0dc16428c5d0f828c83edc86f7`; evidence `SHA256SUMS` `d193ae45…eba4` |
| Stage manifest | SHA-256 `f820807c03b1f6f45853058ecfaec9b8615f6bdaf84186d872ff154f39a9580f` |
| Tool checkout | the same clean `54054042` checkout |
| Toolchain | MacBook Neo (`Mac17,5`, Apple A18 Pro, 8 GB), macOS 27.2 (26B123), native arm64; rustc/cargo 1.99.0; Node v24.14.0 (zlib `1.3.1-e00f703`); pnpm 12.8.2; esbuild 0.25.12; Hono 4.12.25 |
| Build | one guarded `cargo build --locked -p zfb` with `CARGO_BUILD_JOBS=1`, `CARGO_INCREMENTAL=0` and `CARGO_PROFILE_DEV_DEBUG=0` (the 20 GiB rule: 13.5 GiB free), 11 min; `target/debug/zfb` SHA-256 `4ca75863…3813` |

All 16 raw and `gzipNormalized` totals equal the ceilings, with two identical full-dist inventories per case. The bundle's own `check-budget.mjs` exited 0 on the first attempt.

The driver is the reviewed 2026-10-26 release-bump driver with only the source, lock hash, paths and header changed. The receipt's command arrays come from the same mechanical extractor, plus one rule: a `key=` line written by `tee` can land in front of the next xtrace record, so a `key=+ cmd` line is accepted only when the previous record was `printf key=`. That rule recovered one real `shasum` record here. Re-extracting the 2026-10-26 bundle with it adds two real `shasum` records that its receipt omitted, and nothing else. That historical receipt is unchanged.

The receipt covers stages 01 to 09 and their first-level xtrace records, as the 2026-10-26 receipt did. It does not list stage `00-assert-env` (HEAD equals the source, `git status` is clean outside `target`/`node_modules`, Cargo.lock hash, `Darwin:arm64`, `proc_translated=0`, rustc/cargo 1.99.0, pnpm 12.8.2, Node/zlib), the `++` command substitutions such as `esbuild --version`, or the post-run `git status` (`git-status-after.txt`, empty). Those records are in the retained evidence directory, and their bytes are in `SHA256SUMS` `d193ae45…eba4`. The dependency fingerprint is not re-derived from a recorded command: the staging validators checked it against the measurement at staging and apply time.

The generated `platformStatus`, `note` and README text call the evidence "owner-attested", while the receipt's operator says owner attestation was pending at collection time. The owner's approval of this application is that attestation. The generated `note` also says the normalizer digest was re-pinned; it was not (`normalizerSha256` is unchanged). Both are renderer wording shared with earlier stages. They are kept byte-for-byte here because the applied bytes are the reviewed stage, and the renderer fix is tracked in [#4593](https://github.com/Takazudo/zudo-front-builder/issues/4593).

## Result

The stage exited 0: `staged Darwin island-size contract to v5.2.0 (release-bump)`. The apply printed `applied 3 staged files (darwin-arm64 release-bump)` and exited 0. The applied bytes equal the staged hashes:

| File | Live before | Staged and applied |
| --- | --- | --- |
| `research/v3-island-size/README.md` | `9027baf5…a189` | `3c7217ddc08483c42032df995e920cd69974ea755468e983a5e3ba3f58bf368b` |
| `research/v3-island-size/decision.json` | `ed3ffdcd…ecb7` | `87c58614425848e9b4ba125de8b962e6c0dcbda6f5c23ed98e397745547a55bd` |
| `scripts/__tests__/island-size-budget.test.mjs` | `1366360e…537b` | `db465587c16cf676a00ce72215e54541d7943e1729156bfdd939d08f98b6fc76` |

- **Unchanged:** ceilings, allowance, the dependency fingerprint, `fixtureSha256`, `runnerSha256`, `normalizerSha256` and `toolchain.cargoLockSha256`.
- **Moved:** `toolchain.packageVersion` (`5.1.5` → `5.2.0`), `toolchain.pnpmLockSha256` (`51cee362…` → `5cce7d76…`), `outcome` (`release-version-stamp`), `baselineSourceSha`, `platformStatus` and `baselineEvidence` (which drops `fingerprintTransition`).
- **Actual gzip (evidence only):** moved on 10 rows. Workspace: event-only and multi-island +2, json-api and scalar-signal -2. Packed: show-for +3, scalar-signal +1, json-api and model -1, blog-theme and event-only -3.

An independent review checked the receipt, bundle digest, the checker re-run, the contract invariants and the command extraction. It found that the applied test file alone left two history tests pinned to the superseded live contract. Following `f3c74e5e2`, the predecessor (`decision.json` at `54054042`) is frozen as `scripts/__tests__/fixtures/island-size-darwin-v5.1.5-doc911.json`. The Darwin `repinnedFromContracts` row cites it, the doc 9.1.1 test reads that fixture, and a new test checks that the live contract moved from it only by this release bump. The focused budget suite passes 77/77. The repository formatter rewraps `decision.json`; the committed file differs from the applied bytes in whitespace only, and its parsed JSON is identical.

This is current-source Darwin evidence. It does not show that the published 5.2.0 binary (built from `0f17c8e9`) contains later main changes.
