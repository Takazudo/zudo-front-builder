# Linux x64 noyalib-4268 stage and apply (2026-10-21)

This stage applies the reviewed `noyalib-4268` `cargo-producer-lock` transition ([NOYALIB-4268.md](../../cargo-transitions/NOYALIB-4268.md)) to `decision-linux-x64.json` on PR #4385. It was run by Claude under POLICY-02 on the 4336 coordination issue, which lets agents run apply and admission steps; Takazudo chose to adopt noyalib 0.0.58 on 2026-10-21.

## Inputs

| Field | Value |
| --- | --- |
| Measurement | health [run 39661748139](https://github.com/Takazudo/zudo-front-builder/actions/runs/39661748139), job 119436334916, on #4385. PR merge source `ad295e2beec481e9ad1eb8047b1632c9380e06be`, probe head `cf92bfa545410583215a9829bc5c71781463e14b`. The two have the same tree |
| Artifact | `island-size-linux-x64` 12376366743, zip SHA-256 `99f32aea4084981af40bf483964b67028ecb4cae835a9be527cfd475f9b1a1f1`, equal to GitHub's digest, expires 2026-10-28T19:45:13Z |
| Tool checkout | clean detached worktree, HEAD `cf92bfa545410583215a9829bc5c71781463e14b`, Cargo.lock `c0d70889…` |
| Node | official `v24.14.0` linux-x64 (SHASUMS256 verified), zlib `1.3.1-e00f703`, process-local; pnpm 12.8.2 |

The health job failed only where it was expected to:

- `pnpm test:workspace`: the two lock-pin tests.
- `Enforce Linux x64 island-size budgets`: the three Cargo.lock provenance lines, with "Complete unchanged-total evidence retained".

## Result

The stage exited 0 ([stage.log](stage.log)) and reported no actual gzip change. The apply printed `applied 3 staged files (linux-x64 cargo-producer-lock)` and exited 0. It changed only `README.md` and `decision-linux-x64.json`; the staged test file was byte-identical to the live one.

| File | Live before | Staged and applied |
| --- | --- | --- |
| `stage-manifest.json` | | `f805e791ef523caa742d03cea441c153fea4e7ff86833ca075d43eb16bf182d4` |
| `research/v3-island-size/README.md` | `e24ef67b…` | `dad0681f4ec1ebb1a81daea66709dc42082e203ef092493a1ec78cde19d4df43` |
| `research/v3-island-size/decision-linux-x64.json` | `f2c54dc1…` (v4.6.0 release bump) | `a680178ccdc798166157d852dbacf179abd178258446c8ef8079e3f1241ffdb6` |

- **Unchanged:** ceilings, allowance, the dependency fingerprint, `packageVersion` (`4.6.0`), `pnpmLockSha256` and every actual gzip total.
- **Moved:** `toolchain.cargoLockSha256` (`d51febae…` → `c0d70889…`), and `outcome`, which becomes `cargo-producer-lock-transition`.

The repository formatter rewraps `decision-linux-x64.json` on commit. The committed file differs from the applied bytes in whitespace only, and its parsed JSON is identical. The v4.6.0 predecessor is frozen as `scripts/__tests__/fixtures/island-size-linux-x64-v4.6.0.json`.
