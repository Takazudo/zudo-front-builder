# Linux x64 tokio-security-4569 stage and apply (2026-10-26)

This stage applies the reviewed `tokio-security-4569` `cargo-producer-lock` transition ([TOKIO-4569.md](../../cargo-transitions/TOKIO-4569.md)) to `decision-linux-x64.json` on PR #4571. It was run by Claude under POLICY-02 on the 4336 coordination issue, which lets agents run apply and admission steps; the transition replaces tokio 1.52.3 with 1.54.1 for RUSTSEC-2026-0386 (#4569).

## Inputs

| Field | Value |
| --- | --- |
| Measurement | health [run 40306538134](https://github.com/Takazudo/zudo-front-builder/actions/runs/40306538134), job 121516985404, on #4571. PR merge source `3bfa2e022a60a97f3f60ac733b970b51f671d5fd`, probe head `665413028815504af7c53a0b11ef58d43e2237e4`. The two have the same tree |
| Artifact | `island-size-linux-x64` 12642919423, zip SHA-256 `804b13ed857cbc778c6a5e0e179582e42692f3db0668ec3c8cb70bf0e0f6cd93`, equal to GitHub's digest, expires 2026-11-02T10:31:43Z |
| Tool checkout | clean detached worktree, HEAD `0cfe0d6f0afb728890d275bee6839a93e809de65` (the probe head plus the runtime-identity re-pin of the Linux contract, README and tests), Cargo.lock `216d99d5…`, pnpm-lock.yaml `fa2cbec1…` |
| Node | host `v24.14.0` darwin-arm64, zlib `1.3.1-e00f703`; pnpm 12.8.2. The script checks the artifact's recorded Linux x64 provenance, not the host |

The tool checkout differs from the measured source only in `decision-linux-x64.json`, `README.md`, `island-size-budget.test.mjs` and a new test fixture. None is a measured identity input; the artifact matched the checkout's locks, package version, fixtures, runners and dependency fingerprint.

The health job failed only where it was expected to:

- `pnpm test:workspace`: the two lock-admission tests.
- `Enforce Linux x64 island-size budgets`: the three Cargo.lock provenance lines, with "Complete unchanged-total evidence retained".

An earlier stage against the v5.1.4 contract was refused because that contract still pinned pnpm-lock.yaml `8ffa0f0b…`. The runtime-identity re-pin in the preceding commit (health run 40294886927 on main `c8a44a5d`) moved it to `fa2cbec1…` first.

## Result

The stage exited 0 ([stage.log](stage.log)) and reported no actual gzip change. The apply printed `applied 3 staged files (linux-x64 cargo-producer-lock)` and exited 0. It changed only `README.md` and `decision-linux-x64.json`; the staged test file was byte-identical to the live one.

| File | Live before | Staged and applied |
| --- | --- | --- |
| `stage-manifest.json` | | `fd7721d7521618bda39a0391be21430bce6d263a9014b4f6d66ab3e800e41d4d` |
| `research/v3-island-size/README.md` | `83a5e671…` | `32d6af9fd940114528429d4d62ce51ff75bf85316d37a9d8537c105cca52e280` |
| `research/v3-island-size/decision-linux-x64.json` | `0aeadcf4…` (v5.1.4 runtime-identity re-pin) | `4d88fa1bb666e47e90b161b6988d18888d049b4f13a2a204518c721c94dca3e8` |

- **Unchanged:** ceilings, allowance, the dependency fingerprint, `packageVersion` (`5.1.4`), `pnpmLockSha256` and every actual gzip total.
- **Moved:** `toolchain.cargoLockSha256` (`e79303ad…` → `216d99d5…`), and `outcome`, which becomes `cargo-producer-lock-transition`.

The runtime-identity predecessor is frozen as `scripts/__tests__/fixtures/island-size-linux-x64-v5.1.4-pnpm.json`. The Darwin arm64 contract is not changed here; it needs its own native measurement.
