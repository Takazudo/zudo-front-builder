# Paired dependency-graph re-pin for zudo-doc 9.1.0 (2026-10-28)

This is the first real activation of `--reason dependency-graph` (LOCK-FINGERPRINT-POLICY.md section 11), for #4580. The docs host's move to zudo-doc 9.1.0 moves the island-size dependency fingerprint from `e0c6472838fa1f0f7028393c3717a2fa4942fb51ae7cf691146bc535d95f8e95` to `346c6ad14b7dcde05778dfc1198ba8ac6e1db85235f176e101ec56944a5aed0f`; the only canonical-input difference is the `tsx@4.23.1` snapshot gaining `optional: true`. Linux comes from the `health` artifact and Darwin from owner-attested local evidence through the new `dependency-graph` profile (owner decision 2026-10-28). Claude ran the stages and the apply on wrath-of-god on Takazudo's instruction (standing rule 2026-10-21). The records are on #4580 ([1/5](https://github.com/Takazudo/zudo-front-builder/issues/4580#issuecomment-6464913366) to 5/5).

## Original enforced runs

| Probe head | Run | Result |
| --- | --- | --- |
| `e2dd2e47` | [health 40656919008](https://github.com/Takazudo/zudo-front-builder/actions/runs/40656919008), job 122710778839 | Rejected only `local dependency fingerprint: sha256 differs from the reviewed contract`; 16/16 totals equal. No artifact: `island-size-evidence.mjs` retained only an old-`Cargo.lock` rejection, fixed in `665e32ac` |
| `665e32ac` | [health 40659132094](https://github.com/Takazudo/zudo-front-builder/actions/runs/40659132094), job 122717498346, merge source `fa3e0f284657d63fdd52e700d66f1f092182f5fa` | Same single rejection; `Complete unchanged-total evidence retained`. Artifact `island-size-linux-x64` 12803537943, zip SHA-256 `c782c2fa…9c3d` (equal to GitHub's digest) |

Both runs' `pnpm test:workspace` failed only on the four fingerprint-pin tests this apply rewrites.

## Darwin evidence

| Field | Value |
| --- | --- |
| Source | clean standalone checkout of `665e32ac53ab3b54ad415923ffae2bd131cbfd3b`, Cargo.lock `216d99d5…7810`, pnpm-lock `9e27e5e3…e213`, package 5.1.5, `decision.json` `68d92342…bae8` |
| Local receipt N | SHA-256 `e24848ed05bebce22f92dd37ae1b8dfbf593e46ae000e2c9490e6435ede8a317` (35 commands) |
| Native check-budget | exit 1; the only rejection is the fingerprint line, and both provenance fingerprints are `346c6ad1…` |
| Toolchain | wrath-of-god (Apple M4), rustc/cargo 1.99.0, Node v24.14.0, pnpm 12.8.2, macOS 27.0.1 (26A434); driver rebuilt from the ZFB-R004 neutralized text |

A first Darwin collection at `e2dd2e47` (N `9b80489d…`) matched the same ceilings but is superseded, because the Darwin source must equal the Linux probe head.

## Result

Both halves staged into one directory and applied together: `applied 5 staged files (darwin-arm64 dependency-graph, linux-x64 dependency-graph)`. The patch (SHA-256 `af5156ed7784a66c29c2981d846d625918ca26d380cab9f40f37b49df2c7db02`, 26567 B) applied here unchanged.

- **Moved, both contracts:** `outcome` (`dependency-graph-repin`), `baselineSourceSha` (`665e32ac`), `dependencyFingerprint.sha256`, `toolchain.pnpmLockSha256` (`9e27e5e3…`), `platformStatus` and `baselineEvidence` (with `fingerprintTransition` `e0c64728…` → `346c6ad1…`, policy 1).
- **Unchanged:** ceilings, allowance, package version 5.1.5, `Cargo.lock`, normalizer, fixtures, runners and every actual gzip total.
- `lock-fingerprint-inventory.json` was regenerated from `--explain`.

The predecessors (both contracts at `665e32ac`) are frozen as `scripts/__tests__/fixtures/island-size-{darwin,linux-x64}-v5.1.5-release.json`, and the predecessor entries and history tests were hand-pinned to them. The repository formatter rewraps the contracts; the committed files differ from the applied bytes in whitespace only.
