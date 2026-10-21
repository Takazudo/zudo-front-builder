# Linux x64 webpki-security-4314 stage and apply (2026-10-21)

This stage applies the reviewed `webpki-security-4314` `cargo-producer-lock` transition ([WEBPKI-4314.md](../../cargo-transitions/WEBPKI-4314.md)) to `decision-linux-x64.json` on PR #4375. It was run under POLICY-02 on the 4336 coordination issue, which lets agents run apply and admission steps.

## Inputs

| Field | Value |
| --- | --- |
| Measurement | health [run 39609635178](https://github.com/Takazudo/zudo-front-builder/actions/runs/39609635178), job 119254853191, on #4375. PR merge source `dbf93a6ce7a74d927e0c0cbd9f86a503bc8bb05c`, probe head `9a31670040818ab5019dc0fdb2f7f5d1cc45313e`. The two have the same tree |
| Artifact | `island-size-linux-x64` 12350627061, zip SHA-256 `1468ae7b642df12ff339d584f0093dc995dafeab943792835d3f5ea86f9da49a`, equal to GitHub's digest, expires 2026-10-28T13:53:14Z |
| Tool checkout | clean, HEAD `9a31670040818ab5019dc0fdb2f7f5d1cc45313e`, Cargo.lock `d51febae…` |
| Node | official `v24.14.0` linux-x64 (SHASUMS256 verified), zlib `1.3.1-e00f703`, process-local; pnpm 12.8.2 |

The health job failed only where it was expected to:

- `pnpm test:workspace`: the two lock-pin tests.
- `Enforce Linux x64 island-size budgets`: the three Cargo.lock provenance lines, with "Complete unchanged-total evidence retained".

## Result

The stage exited 0 ([stage.log](stage.log)) and reported no actual gzip change. The apply printed `applied 3 staged files (linux-x64 cargo-producer-lock)` and exited 0. It changed only `README.md` and `decision-linux-x64.json`; the staged test file was byte-identical to the live one.

| File | Live before | Staged and applied |
| --- | --- | --- |
| `stage-manifest.json` | | `474315318676df9b0d4568453c0555086fcdd563bed7a995ce3d6dd0a3ee767b` |
| `research/v3-island-size/README.md` | `75967c64…` | `efd911e9504b60c960b4d6c8e155784ea693266ef3b7c8270aa66441d69bd7d1` |
| `research/v3-island-size/decision-linux-x64.json` | `2625ccc8…` (direct-V8 rewire) | `7b25f6b07244be1d9c800e4a4328f2f6ca67aef1043034a96d0f4c25974c71f6` |

- **Unchanged:** ceilings, allowance, the dependency fingerprint, `packageVersion` (`4.5.4`), `pnpmLockSha256` and every actual gzip total.
- **Moved:** `toolchain.cargoLockSha256` (`0cd091b7…` → `d51febae…`), and `outcome`, which becomes `cargo-producer-lock-transition`.

The repository formatter rewraps `decision-linux-x64.json` on commit. The committed file differs from the applied bytes in whitespace only, and its parsed JSON is identical.
