# Darwin arm64 noyalib-4268 stage and apply (2026-10-21)

This stage applies the reviewed `noyalib-4268` `cargo-producer-lock` transition ([NOYALIB-4268.md](../../cargo-transitions/NOYALIB-4268.md)) to `decision.json` on PR #4385. Claude ran it on Takazudo's Mac under POLICY-02 on the 4336 coordination issue, which lets agents run apply and admission steps. Takazudo chose to adopt noyalib 0.0.58 on 2026-10-21.

## Inputs

| Field | Value |
| --- | --- |
| Measurement | owner-attested local native Darwin arm64 evidence at source `f65dd70d81da0e407fc3261ef57a99c54de83fdd`, receipt N SHA-256 `ab63a1eee7f0654acea6bcbabae41eb3c4df22d1e89a9e22cc32b63b62394605`, posted as ZFB-R004 on #4336 ([1/5](https://github.com/Takazudo/zudo-front-builder/issues/4336#issuecomment-6327305969) to [5/5](https://github.com/Takazudo/zudo-front-builder/issues/4336#issuecomment-6327307514)) |
| Tool checkout | clean detached `f65dd70d` checkout after `pnpm install --frozen-lockfile`; apply in a second clean `f65dd70d` checkout |
| Toolchain | rustc/cargo 1.99.0, Node v24.14.0 (zlib `1.3.1-e00f703`), pnpm 12.8.2, macOS 26.6.2 |

The measurement matched all 16 Darwin ceilings. The artifact's own `check-budget.mjs` exited 1, and it rejected only the three Cargo.lock provenance lines.

The run deviated from the R002/R003 recipe in four ways:

- N has 35 commands rather than 36 or 37, because the extractor skipped three interleaved tarball `shasum` xtrace lines.
- `CARGO_PROFILE_DEV_DEBUG=0` was set because only 16.2 GiB of disk was free.
- The cargo build waited about 70 minutes in the heavy-guard queue.
- The private operator record O was regenerated (O SHA-256 `7ab5145e…cfe0`) because the first copy carried R003's operator text. N is byte-identical either way.

## Result

The stage exited 0 and reported no actual gzip change. The apply printed `applied 3 staged files (darwin-arm64 cargo-producer-lock)` and exited 0. The posted patch (SHA-256 `ce11d3dc61d7c58eca7079f534580543b1532fba59f96af05f3ca071c341dfeb`, 20513 bytes) applies cleanly to `f65dd70d`, and the resulting files match the applied hashes below.

| File | Live before | Staged and applied |
| --- | --- | --- |
| `research/v3-island-size/README.md` | `2fe79e38…` | `7cb618a6d3386eac48bcbc623f8088562f27749b35a42a6b9c38335b4c15a5f4` |
| `research/v3-island-size/decision.json` | `287e84e5…` (v4.6.0 release bump) | `35647deadd1d215387eeb3d251a6bbc3c7903781e55b2f99b30844c6bf514d04` |
| `scripts/__tests__/island-size-budget.test.mjs` | | `8c0a1c4f4d51cc43fc69761ef1636a8fe90dcf35de90e92ec9047c6099423035` |

- **Unchanged:** ceilings, allowance, the dependency fingerprint, `normalizerSha256`, `packageVersion` (`4.6.0`), `pnpmLockSha256` and every actual gzip total.
- **Moved:** `toolchain.cargoLockSha256` (`d51febae…` → `c0d70889…`), `outcome` (now `cargo-producer-lock-transition`), `baselineSourceSha`, `platformStatus` and `baselineEvidence`.

After the apply, the predecessor table and the Darwin transition test in `island-size-budget.test.mjs` were hand-pinned to the v4.6.0 contract. That contract is frozen as `scripts/__tests__/fixtures/island-size-darwin-v4.6.0.json`.

The repository formatter rewraps `decision.json` on commit. The committed file differs from the applied bytes in whitespace only, and its parsed JSON is identical.
