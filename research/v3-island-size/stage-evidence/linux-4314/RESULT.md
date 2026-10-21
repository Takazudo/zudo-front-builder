# Linux x64 #4314 stage, attempt 1 (2026-10-19)

Stage only. Nothing was applied: the live contract, README, test pins and `Cargo.lock` are unchanged, and applying this stage needs separate owner approval. It followed [LINUX-SECURITY-4314.md](../../LINUX-SECURITY-4314.md) on Claude Code Cloud, as the integration lead decided ([#4314](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6276918040)).

## Inputs

| Field | Value |
| --- | --- |
| Checkout | clean worktree at `7e79458304878d51bafd29e24ec25e93b84893ca`, tree `883733d5a01a0b4cd579c8e204c3e7d904dbff6a` |
| Host | linux/x64, official Node `v24.14.0` (SHASUMS256 verified), zlib `1.3.1-e00f703`, pnpm `12.8.2` |
| Tools install | `pnpm install --frozen-lockfile --ignore-scripts`, exit 0, checkout still clean |
| Artifact | `island-size-linux-x64` `12213032790` from run `39302891207`, job `118232731946`, zip SHA-256 `369e9abfd79bbf08e610e4bf68eb5d99eb87d4f5cd7eb4994f0ec562b1e5f00b` (equals GitHub's digest) |
| Measured source / probe head | `e397161c609e780a79ec4273c907168355518aab` / `0355ad78a4f8d2c240844d2b48da7c2769f6de25`, both tree `883733d5…` |
| Original checker | the job's enforce step rejected only the three old-lock diagnostics and retained complete unchanged-total evidence |

No new measurement, Cargo build or Actions run was made.

## Result

The stager exited 0 on the first attempt. Its log is in [stage.log](stage.log).

| File | SHA-256 |
| --- | --- |
| `stage-manifest.json` | `9b2746b9e25adc79ec85622296657e4e1dd5ce76c23f36e64c0ac2b481d5ef52` |
| [staged.diff](staged.diff) (`diff -u` of each live file against its staged copy) | `fddf15d51bc5d7c4b2d99ad6bc66cbd0c120b9cbb8eae28bfc303269a5200a07` |
| staged `decision-linux-x64.json` | `65ea5dd69d0c3294cd24e128388b74353ba43ad767e714be2be908d3ab9191d7` (live `f5055202…`) |
| staged `README.md` | `69c46258dd57ed6f626a3d1b86fca43496f9098ef9ce83f79d6b5e5d6179eac4` (live `e4496a7b…`) |
| staged `island-size-budget.test.mjs` | `63f2072f…`, identical to live |

The manifest, verbatim:

```json
{
  "stageVersion": 1,
  "runs": [
    {
      "reason": "cargo-producer-lock",
      "platform": "linux-x64",
      "sourceSha": "e397161c609e780a79ec4273c907168355518aab",
      "headSha": "0355ad78a4f8d2c240844d2b48da7c2769f6de25",
      "runId": 39302891207,
      "artifactId": 12213032790
    }
  ],
  "destinations": {
    "research/v3-island-size/README.md": {
      "live": "e4496a7bfcd0be52fa4c59f6688b5c0b23a731e5e88f0f423e174c0b899b706a",
      "staged": "69c46258dd57ed6f626a3d1b86fca43496f9098ef9ce83f79d6b5e5d6179eac4"
    },
    "research/v3-island-size/decision-linux-x64.json": {
      "live": "f505520217649c5cd0a614fe68c83cd500f44f4aafd8c599082348a2d6e7cf3a",
      "staged": "65ea5dd69d0c3294cd24e128388b74353ba43ad767e714be2be908d3ab9191d7"
    },
    "scripts/__tests__/island-size-budget.test.mjs": {
      "live": "63f2072fa630274b675b984a6e27b23adcfe6399e8454e1e2b16707f50265d59",
      "staged": "63f2072fa630274b675b984a6e27b23adcfe6399e8454e1e2b16707f50265d59"
    }
  }
}
```

Comparing the live contract with the staged one:

- Ceilings are equal and the allowance stays raw 0 / gzipNormalized 0.
- Only `outcome`, `baselineSourceSha`, `toolchain.cargoLockSha256`, `platformStatus` and `baselineEvidence` change.
- `toolchain.cargoLockSha256` moves from `230f0acb…` to `563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd`.
- `cargoTransition` records review `rustls-security-4314` (manifest `fe689aca…04ff`) and reads the old lock from `21d6807e…:Cargo.lock`.
- `repinnedFrom` cites the live contract at `7e79458…` (outcome `cargo-internal-rewire-transition`, run `39008132372`).
- Actual gzip moved by −3 to +4 bytes in 11 rows. That is evidence only; the normalized totals are unchanged.

## Before applying

A disposable copy of `7e79458` with only the two staged files in place ran `scripts/__tests__/island-size-budget.test.mjs`:

| | Before | Staged Linux files in place |
| --- | --- | --- |
| Failures | 2: the Darwin and Linux ceilings tests (old lock pin) | 3: listed below |

- **Darwin ceilings:** expected, because the Darwin stage is not applied.
- **`decision-linux-x64.json carries a flat repinnedFrom with a verified contract reference`:** the predecessor table is pinned by hand to the previous re-pin.
- **`moves the live Linux contract from the released v4.5.3 bytes only by the paired cargo-internal-rewire`:** the live outcome becomes `cargo-producer-lock-transition`.

The Linux ceilings test passes once the lock moves. The rebaseline tools do not rewrite the hand-pinned predecessor and history tests, as with the v4.5.3 rewire. Applying this stage therefore needs a reviewed edit of those two tests in the same change. That edit is a gate-file change and needs fresh-context review. The stale `cargo-internal-rewire (#4258)` comment on the Darwin pin is corrected in the same change.
