# Darwin arm64 webpki-security-4314 stage and apply (2026-10-21)

This stage applies the reviewed `webpki-security-4314` `cargo-producer-lock` transition ([WEBPKI-4314.md](../../cargo-transitions/WEBPKI-4314.md)) to `decision.json` on PR #4375. It ran on the owner's Mac under POLICY-01/02 on the 4336 coordination issue: receipt N is recorded as attested on Takazudo's instruction, and agents may run apply and admission steps. The Mac committed and pushed nothing; its patch was applied and verified in the PR checkout.

## Inputs

| Field | Value |
| --- | --- |
| Source | clean standalone checkout of `5b5f21523818634f8982e0dd3d7fc14885141f2c`, Cargo.lock `d51febae…766d`, package 4.5.4 |
| Local receipt N | SHA-256 `fdffd9ed251d0ecbb72e38fc7ce6bbcb216f6824bd37cc3d7404299af6950547`; `validateLocalReceipt` passes (36 commands, bundle `6152d347…423e`) |
| Patch | SHA-256 `7a2f9b570e34ebe00b0d2a044f2a1f9de7f3aa87e1cfd59c7fa7a53316981969` |

All 16 raw and `gzipNormalized` totals equal the ceilings, with two identical full-dist inventories per case. The artifact's own `check-budget.mjs` run against the previous pin exited 1 only on the three Cargo.lock provenance lines.

## Result

The stage and apply exited 0 and changed `README.md`, `decision.json` and the Darwin pin in `island-size-budget.test.mjs`.

- **Unchanged:** ceilings, allowance, the dependency fingerprint, `packageVersion` (`4.5.4`), `pnpmLockSha256` and every actual gzip total.
- **Moved:** `toolchain.cargoLockSha256` (`0cd091b7…` → `d51febae…`), `outcome` (`cargo-producer-lock-transition`), `baselineSourceSha`, `platformStatus` and `baselineEvidence`.

The applied `decision.json` was `8035b94a…5461`. The repository formatter rewraps it on commit; the committed file differs in whitespace only, and its parsed JSON is identical.
