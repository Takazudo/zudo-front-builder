# Linux x64 #4314 stage attempt 3 and apply, on top of v4.5.4 (2026-10-21)

Attempt 2 ([ATTEMPT-02.md](ATTEMPT-02.md)) re-pinned the Linux contract from the v4.5.3 cargo-internal-rewire contract. Main then released v4.5.4, and that release re-pinned the Linux contract (`release-version-stamp`, `Cargo.lock` still `230f0acb…`). Merging main into `base/4314-security-darwin-evidence` kept main's v4.5.4 contract. Attempt 2's result therefore no longer described the merged tree.

This attempt re-stages the same reviewed transition from a fresh measurement of the merged tree, with main's v4.5.4 contract as its predecessor. The owner authorized the main integration on 2026-10-21. Release is excluded.

## Inputs

| Field | Value |
| --- | --- |
| Measurement | health [run 39570410628](https://github.com/Takazudo/zudo-front-builder/actions/runs/39570410628) (job 119122855750) on #4369. PR merge source `60d80bcd5e557a141e3fe021336e1054a34b92b4`, probe head `949f4cccbdd48ed6d8c8208ec51e93c1dee37b08` |
| Artifact | `island-size-linux-x64` 12332223203, zip SHA-256 `1a1283671037de4c52757147f0a9b4705e7a14e7d58d99a7aad7f64cece194c4`, equal to GitHub's digest |
| Tool checkout | clean, HEAD `be106cb261fd8bbdd0fbe83d64f64cc1649919e6`, Cargo.lock `56319425…` |
| Node | official `v24.14.0` linux-x64 (SHASUMS256 verified), zlib `1.3.1-e00f703`, process-local |
| Stager arguments | as in attempt 2, with the new run, job, artifact, head and source |

## Result

The stage exited 0 ([stage-03.log](stage-03.log)) and reported no actual gzip change. The apply printed `applied 3 staged files (linux-x64 cargo-producer-lock)` and exited 0. It changed only `README.md` and `decision-linux-x64.json`. The staged test file was byte-identical to the live one.

| File | Live before | Staged and applied |
| --- | --- | --- |
| `stage-manifest.json` | | `fa9265e9ed748c8251555b88edcf3d7273729aab0926ab821fa7a4585e589e4c` |
| `research/v3-island-size/README.md` | `9e2a76ea…` | `d2e0cfc1c07c38977df30666e0bb96c79aac59f872e2d3644331642c31093d10` |
| `research/v3-island-size/decision-linux-x64.json` | `9f8cfcad…` (v4.5.4) | `cd351992f2770250c8c46fc645b792e94369f7ba9169df53dbaa94623692b303` |

- **Unchanged:** ceilings, allowance, the dependency fingerprint, `packageVersion` (`4.5.4`) and `pnpmLockSha256`.
- **Moved:** `toolchain.cargoLockSha256` (`230f0acb…` → `56319425…`), and `outcome`, which becomes `cargo-producer-lock-transition`.
- **No change:** no actual gzip total moved relative to v4.5.4.

The repository formatter rewraps `decision-linux-x64.json` on commit. The committed file differs from the applied bytes in whitespace only, and its parsed JSON is identical.
