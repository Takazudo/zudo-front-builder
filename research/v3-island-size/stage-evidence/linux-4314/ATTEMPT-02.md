# Linux x64 #4314 stage attempt 2 and apply (2026-10-21)

Attempt 1 ([RESULT.md](RESULT.md)) was staged at `7e79458`. It could not be applied after the Darwin apply ([darwin-4314](../darwin-4314/RESULT.md)), because that apply changed `README.md`. This attempt re-stages the same retained artifact at the merge head that carries the Darwin apply, `83f5517e5d589d2f818a992625baf0032f2b77b2`, and applies it there. That is what [LINUX-SECURITY-4314.md](../../LINUX-SECURITY-4314.md) "After the Darwin apply" describes. The recipe's two tree-equality checks were dropped, as that section says. Every stager argument is unchanged from attempt 1.

## Inputs

| Field | Value |
| --- | --- |
| Checkout | clean, HEAD `83f5517e…`, Cargo.lock `56319425…` |
| Node | official `v24.14.0` linux-x64 (SHASUMS256 verified), zlib `1.3.1-e00f703`, process-local. Dependencies were already installed by pnpm `12.8.2`. |
| Artifact | retained copy of `12213032790` (run `39302891207`, job `118232731946`), zip SHA-256 `369e9abfd79bbf08e610e4bf68eb5d99eb87d4f5cd7eb4994f0ec562b1e5f00b`, equal to GitHub's digest |

## Result

The stage exited 0; its log is [stage-02.log](stage-02.log). The apply printed `applied 3 staged files (linux-x64 cargo-producer-lock)` and exited 0. It changed exactly `README.md` and `decision-linux-x64.json`.

| File | Live before | Staged and applied |
| --- | --- | --- |
| `stage-manifest.json` | | `1229ce472d849143fc3cdab5f85e0eee122e0b5f06dc7d3fde0763b369938f76` |
| `research/v3-island-size/README.md` | `1a92489b…` | `daba719d02d5aa88be7c356eea9408508102a0c1cf6d9acfa75b7caed1280034` |
| `research/v3-island-size/decision-linux-x64.json` | `f5055202…` | `f3522c4594b6c8010ae1450d51b9ffb2310bf62f746d89b2644433c2357093f1` |
| `scripts/__tests__/island-size-budget.test.mjs` | `08064f4d…` | unchanged |

The staged Linux contract differs from attempt 1's staged contract in one field only: `baselineEvidence.repinnedFrom.contract.commit`. That field names the commit that stores the predecessor bytes `f5055202…`, which was `7e79458…` then and is `83f5517e…` now. Ceilings and allowance are unchanged. Actual gzip moves by the 11 deltas recorded for decision D2.

The repository formatter rewraps `decision-linux-x64.json` on commit. The committed file differs from the applied bytes in whitespace only, and its parsed JSON is identical. An earlier re-stage at a local merge head that was rebuilt before any push was discarded unapplied.
