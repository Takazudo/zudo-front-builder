# Darwin arm64 v5.1.4 release-bump stage and apply (2026-10-26)

This stage re-pins the Darwin contract from v4.7.0 to v5.1.4 with `--reason release-bump`, at main `c8a44a5d`. It is stage 1 of 2 for #4569. A `cargo-producer-lock` stage allows only `Cargo.lock` to move, so the Darwin package version and pnpm lock must catch up first; the `tokio-security-4569` stage follows at the tokio head. Claude ran it on Takazudo's Mac under POLICY-02 on the 4336 coordination issue, which lets agents run admission, apply and attestation steps.

## Inputs

| Field | Value |
| --- | --- |
| Source | clean standalone checkout of `c8a44a5d5a42b08bad67cf24ea8f04b5dbae4283`, Cargo.lock `e79303ad…893d` (unchanged), pnpm-lock `fa2cbec1…44d2`, package 5.1.4 |
| Local receipt N | SHA-256 `991d743972ed88c311764fa654559c39984602f5bc243114984e74a42c016565`; `validateLocalReceipt` passes (36 commands, bundle `a5e92c10…a0bd`) |
| Private original receipt O | SHA-256 `b693f80a417fff441e59716e65625a92a840855294313f3f7bb0e6ebb64cf818` |
| Tool checkout | clean detached `c8a44a5d` checkout after `pnpm install --frozen-lockfile`; apply in a second clean `c8a44a5d` checkout |
| Toolchain | rustc/cargo 1.99.0, Node v24.14.0 (zlib `1.3.1-e00f703`), pnpm 12.8.2, macOS 27.0.1 (26A434) |

All 16 raw and `gzipNormalized` totals equal the ceilings, with two identical full-dist inventories per case. The artifact's own `check-budget.mjs` exited 0: a schemaVersion 3 contract compares the dependency fingerprint, not the package version or the whole pnpm-lock hash.

The run deviated from the R004 driver in three ways:

- `CARGO_PROFILE_DEV_DEBUG=0` was set by operator instruction because the disk is shared, although 25.7 GiB was free (above the 20 GiB rule). The provenance line records that reason.
- The driver ran with the rustup proxy directory first on `PATH`, because Homebrew's rust 1.94.0 otherwise shadows `rustc`. A first attempt at the tokio head stopped at the toolchain assertion before any install or build; it is kept as `attempt1/` of that bundle.
- The cargo build passed the heavy guard without a queue wait (629 s).

## Result

The stage exited 0: `staged Darwin island-size contract to v5.1.4 (release-bump)`. The apply printed `applied 3 staged files (darwin-arm64 release-bump)` and exited 0. The applied bytes equal the staged hashes:

| File | Live before | Staged and applied |
| --- | --- | --- |
| `research/v3-island-size/README.md` | `277309fe…b234` | `431dd4f69f947881b7c091e0b1ef6cf5600afb5b6167d7a1321b7aa79cc9a4c6` |
| `research/v3-island-size/decision.json` | `70e44110…3a78` | `012eda6f013059c9b54a67bc2b95c7df584b5878679dde5308c884a4d808cfcf` |
| `scripts/__tests__/island-size-budget.test.mjs` | `64b7ed9d…7761` | `2a7006a0a2691e794635bc62532fbf7747c7a377f5b32296ca2a84075dfef9fb` |

- **Unchanged:** ceilings, allowance, the dependency fingerprint, `normalizerSha256` and `toolchain.cargoLockSha256`.
- **Moved:** `toolchain.packageVersion` (`4.7.0` → `5.1.4`), `toolchain.pnpmLockSha256` (`66b1209c…` → `fa2cbec1…`), `outcome` (`release-version-stamp`), `baselineSourceSha`, `platformStatus` and `baselineEvidence`.
- **Actual gzip (evidence only):** moved on 12 rows. Workspace: blog-theme +2, show-for +1, multi-island -1, json-api and scalar-signal -2, model -4. Packed: event-only +3, model and show-for +1, multi-island -1, blog-theme -2, scalar-signal -3.

After the apply, the previous contract (`decision.json` at `c8a44a5d`, the #4429 oxc edge) was frozen as `scripts/__tests__/fixtures/island-size-darwin-v4.7.0-oxc.json`. The Darwin predecessor entry and the live-contract history test were hand-pinned to it. The repository formatter rewraps `decision.json`; the committed file differs from the applied bytes in whitespace only, and its parsed JSON is identical.
