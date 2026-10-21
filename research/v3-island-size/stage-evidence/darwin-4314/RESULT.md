# Darwin arm64 #4314 stage attempt 3 and apply (2026-10-21)

Owner decisions D1 yes, D2 yes and D3 no are recorded in [the decision packet](../../SECURITY-4314-DECISION-PACKET.md). The Mac steps followed [MAC-ATTEMPT3-4314.md](../../MAC-ATTEMPT3-4314.md) on the Mac that holds collection-01. No build, measurement or push happened there.

This directory holds **derived** files only. The original receipt, the bundle and stage attempt 2 stay private and unchanged.

## Identities

| Item | SHA-256 |
| --- | --- |
| Original receipt (private, unchanged) | `cfae5404e8e6a0f61c045eb024369cf28321669c9160bb3a3532108d0131e50c` |
| Neutral receipt N, embedded verbatim in `decision.json` | `52cfabf929b2b9f26c70633daef30d24f135aa8eeef61d6da3e5d65fcd168431` |
| Retained bundle (inventory digest) | `136395016503fb8062e8a902cfc5e11191461325952e0216f09f6ffe0a03c83e` |
| `stage-manifest.json` (private; it holds absolute paths) | `f910ab221b01f412c9a046486ab9c66f8dd1f2afcc45d59ab7b097730163d9b0` |
| [stage-manifest.derived.json](stage-manifest.derived.json) (absolute paths replaced by `<NEW>`, `<TOOLS>`, `<RETAIN>`) | `9962c9e60a233d17efb1a13ab2d6e3542e5d865761b1aeb380c8ba6b7d585b7f` |
| [neutral-report.json](neutral-report.json) | digests and placeholder counts only |

Owner attestation of N: [#4314 comment 6310764872](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6310764872). Claude posted it on the owner's explicit instruction.

## Neutralization

- The first run refused, and wrote nothing. Two fragments remained: the local user name inside the pnpm-pack tarball names. The user name equals the public npm scope.
- One map prefix was added, mapping the absolute pack path and tarball stem to `<PACKED_ZFB_TARBALL_STEM>`. The second run passed.
- N changes 39 command arguments, `bundle.location` and `operator`. `operator` is now `zfb owner, local Darwin arm64 collection`. Every other field equals the original.

## Stage and apply

| Step | Result |
| --- | --- |
| Stage, from tools `7e79458`, source `bda633b` | exit 0, output `staged Darwin island-size contract to v4.5.3 (cargo-producer-lock)` |
| Apply, from a new detached `bda633b` worktree | exit 0, output `applied 3 staged files (darwin-arm64 cargo-producer-lock)`. `git status` showed exactly the three destinations. |
| Original receipt and attempt 2 after the apply | unchanged |

Applied bytes, equal to the staged values:

| File | Live at `bda633b` | Applied |
| --- | --- | --- |
| `research/v3-island-size/README.md` | `1d76d181…` | `a5f540f395f127b668232a6d92ea6ae3e59557738b5e4a60e762aef43406e355` |
| `research/v3-island-size/decision.json` | `cc32aa6d…` | `01ddb9d6b911d28432df4ad8101d09198c83706107be7fb06b8d9314b4cac2db` |
| `scripts/__tests__/island-size-budget.test.mjs` | `63f2072f…` | `08064f4da0f4f829a12207f356c35bb9a06113b52aed4ab87b14339f23c970f7` |

The repository formatter rewraps `decision.json` on commit (the same as for the P2a apply). The committed file therefore differs from the applied bytes in whitespace only, and its parsed JSON is identical. `README.md` and the test pins are committed byte for byte.

## Independent review (L1, from the returned files)

- **Receipt:** the embedded receipt hashes to N, and it passes `validateLocalReceipt` at `7e79458`.
- **Unchanged contract fields:** `ceilings`, `allowance` and `dependencyFingerprint`.
- **Changed contract fields:**
  - `toolchain.cargoLockSha256` moves `230f0acb…` → `56319425…`.
  - `outcome` becomes `cargo-producer-lock-transition`.
  - `actualGzip` moves by exactly the 11 deltas recorded for D2.
- **Private paths:** none of the three files gains a private-path fragment relative to `bda633b`.

The README's two example exports with a home path have been in the file since `027a4120` (2026-10-01). Whether to remove them is a separate decision.

## Binding scope

At apply time, the tooling re-verified the receipt digest, the bundle digest, the clean tools checkout and the clean `bda633b` checkout. Nobody can re-run those checks from this repository, because the originals are private. This record and the owner's attestation are the public evidence.
