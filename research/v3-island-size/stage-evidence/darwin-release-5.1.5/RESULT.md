# Darwin arm64 v5.1.5 release-bump stage and apply (2026-10-28)

This stage re-pins the Darwin contract from v5.1.4 to v5.1.5 with `--reason release-bump`, at main `8dbc5983`. It is stage 1 of 2 for #4580. A `dependency-graph` stage refuses a package-version move, so the Darwin package version and pnpm lock catch up first; the paired dependency-graph stage follows at the zudo-doc 9.1.0 head. Claude ran it on wrath-of-god, Takazudo's Mac, on Takazudo's instruction (standing rule 2026-10-21). The records are on #4580 ([1/5](https://github.com/Takazudo/zudo-front-builder/issues/4580#issuecomment-6464219616) to 5/5).

## Inputs

| Field | Value |
| --- | --- |
| Source | clean standalone checkout of `8dbc59834efc2eb86a6a93ee2620473408415712`, Cargo.lock `216d99d5…7810` (unchanged), pnpm-lock `adc1bda0…0e59`, package 5.1.5, `decision.json` `661c48dc…a95a` |
| Local receipt N | SHA-256 `e46bff1e40095270eb3d725749df6f198791407bc67c49fa216275e5664b36f7` (35 commands, bundle `bdc25b3e…48a1`) |
| Private original receipt O | SHA-256 `88b3349069ed2416046ee64f95e61f9ef3062772b69c300fc5ae8dbac6a9211d` |
| Tool checkout | clean `8dbc5983` checkout after `pnpm install --frozen-lockfile`; apply in a second clean `8dbc5983` checkout |
| Toolchain | wrath-of-god (Apple M4), rustc/cargo 1.99.0, Node v24.14.0 (zlib `1.3.1-e00f703`), pnpm 12.8.2, macOS 27.0.1 (26A434) |

All 16 raw and `gzipNormalized` totals equal the ceilings, with two identical full-dist inventories per case. The artifact's own `check-budget.mjs` exited 0.

The run deviated from the 5.1.4 run in two ways:

- No driver bundle from 2026-10-26 was retained on either Mac, so the driver was rebuilt from the ZFB-R004 neutralized text (#4336 comment 6327306336). Only the source, lock hash, paths and header changed, plus the rustup proxy directory on `PATH` (Homebrew's rust otherwise shadows `rustc`).
- `CARGO_PROFILE_DEV_DEBUG=0` came from the driver's own 20 GiB rule, because only 14.3 GiB was free.

## Result

The stage exited 0: `staged Darwin island-size contract to v5.1.5 (release-bump)`. The apply printed `applied 3 staged files (darwin-arm64 release-bump)` and exited 0. The applied bytes equal the staged hashes, and the patch (sha256 `2d9bbdf8…240a`, 21250 B) applied here unchanged:

| File | Live before | Staged and applied |
| --- | --- | --- |
| `research/v3-island-size/README.md` | `54e2fd5f…0744` | `7eabaabfa47e0ec25282cd6e05d01c9ae61e1d9baf62de49d8afcfd1f796b820` |
| `research/v3-island-size/decision.json` | `661c48dc…a95a` | `b7b5feab702a8a772b5a021fbb9d49ad68ab9773a0b0253ba6848594d4351914` |
| `scripts/__tests__/island-size-budget.test.mjs` | `c5aa9116…5600` | `8a3ccf5949d4303664a97752bc038bca97e74d934828841840410bc1b3b5b51b` |

- **Unchanged:** ceilings, allowance, the dependency fingerprint, `normalizerSha256` and `toolchain.cargoLockSha256`.
- **Moved:** `toolchain.packageVersion` (`5.1.4` → `5.1.5`), `toolchain.pnpmLockSha256` (`fa2cbec1…` → `adc1bda0…`), `outcome` (`release-version-stamp`), `baselineSourceSha`, `platformStatus` and `baselineEvidence`.
- **Actual gzip (evidence only):** moved on 10 rows. Workspace: model +4, scalar-signal +3, blog-theme, json-api and show-for +1, multi-island -1. Packed: blog-theme +5, json-api +1, model -1, show-for -3.

After the apply, the predecessor (`decision.json` at `8dbc5983`) was frozen as `scripts/__tests__/fixtures/island-size-darwin-v5.1.4-tokio.json`. The Darwin predecessor entry and the live-contract history test were hand-pinned to it. The repository formatter rewraps `decision.json`; the committed file differs from the applied bytes in whitespace only, and its parsed JSON is identical.
