# Darwin arm64 tokio-security-4569 stage and apply (2026-10-26)

This stage applies the reviewed `tokio-security-4569` `cargo-producer-lock` transition ([TOKIO-4569.md](../../cargo-transitions/TOKIO-4569.md)) to `decision.json` on the #4569 branch. It is stage 2 of 2: the Darwin v5.1.4 release bump ([darwin-release-5.1.4](../darwin-release-5.1.4/RESULT.md)) moved the package version and pnpm lock first, because `cargo-producer-lock` allows only `Cargo.lock` to move. Claude ran it on Takazudo's Mac under POLICY-02 on the 4336 coordination issue, which lets agents run admission, apply and attestation steps.

## Inputs

| Field | Value |
| --- | --- |
| Source | clean standalone checkout of `4ecf8db40745124bf072c37a5fa9f25aa078421e`, Cargo.lock `216d99d5…7810`, pnpm-lock `fa2cbec1…44d2`, package 5.1.4, `decision.json` `f23b4969…4113` (the v5.1.4 release contract) |
| Local receipt N | SHA-256 `29c704ca40f2932d5545d20aae142526fb9c812f9f49ce027d0dd2b4e6693ae6`; `validateLocalReceipt` passes (35 commands, bundle `b495bb45…1106`) |
| Private original receipt O | SHA-256 `322b9318c11ebaeaeebac296fd0dfb0762b952f928a246eb730f65c97a0e306b` |
| Tool checkout | clean detached `4ecf8db4` checkout after `pnpm install --frozen-lockfile`; apply in a second clean `4ecf8db4` checkout |
| Toolchain | rustc/cargo 1.99.0, Node v24.14.0 (zlib `1.3.1-e00f703`), pnpm 12.8.2, macOS 27.0.1 (26A434) |

All 16 raw and `gzipNormalized` totals equal the ceilings, with two identical full-dist inventories per case. The artifact's own `check-budget.mjs` exited 1, and it rejected only the three Cargo.lock provenance lines: `local Cargo.lock`, and `workspace`/`packed provenance`.

The run deviated from the R004 driver in three ways:

- `CARGO_PROFILE_DEV_DEBUG=0` was set by operator instruction because the disk is shared, although 23.6 GiB was free (above the 20 GiB rule). The provenance line records that reason.
- The driver ran with the rustup proxy directory first on `PATH`, because Homebrew's rust 1.94.0 otherwise shadows `rustc`.
- An earlier collection at `665413028` matched the same ceilings but could not be staged, because its `decision.json` still pinned 4.7.0. It is retained as superseded evidence.

## Result

The stage exited 0: `staged Darwin island-size contract to v5.1.4 (cargo-producer-lock): no actual gzip change (actual gzip, evidence only)`. The apply printed `applied 3 staged files (darwin-arm64 cargo-producer-lock)` and exited 0. The applied bytes equal the staged hashes:

| File | Live before | Staged and applied |
| --- | --- | --- |
| `research/v3-island-size/README.md` | `32524521…eddd` | `fd51a822ba8c53a7aaaef207c30547a9db8edf60b9efd8de792d64aaf318f1e9` |
| `research/v3-island-size/decision.json` | `f23b4969…4113` | `5b565e5a1cc79072e61bbdc93ceda74e9cb9c90c5630dcb2463014bd3fec3b8b` |
| `scripts/__tests__/island-size-budget.test.mjs` | `8edfe7c3…1b8e` | `cd4064c4df7eaa838236a4c6a4bc1efbfed7556bbcabb4fa75971dc05b340f89` |

- **Unchanged:** ceilings, allowance, the dependency fingerprint, `normalizerSha256`, `packageVersion` (`5.1.4`), `pnpmLockSha256` and every actual gzip total.
- **Moved:** `toolchain.cargoLockSha256` (`e79303ad…` → `216d99d5…`), `outcome` (`cargo-producer-lock-transition`), `baselineSourceSha`, `platformStatus` and `baselineEvidence`. The transition record adds `tokio@1.54.1` and removes `tokio@1.52.3`, proved against `c8a44a5d:Cargo.lock`, and equals the Linux record.

After the apply, the predecessor (`decision.json` at `4ecf8db4`) was frozen as `scripts/__tests__/fixtures/island-size-darwin-v5.1.4-release.json`. The Darwin predecessor entry and the live-contract history test were hand-pinned to it. The repository formatter rewraps `decision.json`; the committed file differs from the applied bytes in whitespace only, and its parsed JSON is identical.
