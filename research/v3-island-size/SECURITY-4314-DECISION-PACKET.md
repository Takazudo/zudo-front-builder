# #4314 rustls transition: evidence inventory and decision packet

This is an inventory and a set of decisions for the owner. It authorizes nothing: no live apply, admission waiver, merge to `main`, #4317 merge or new Mac work. The plan it answers is the [reconciliation plan](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6277405812). Batch BATCH-ZFB-01 item C ([#4336](https://github.com/Takazudo/zudo-front-builder/issues/4336#issuecomment-6288828584)) asked for it.

**Evidence policy.**

- Original Darwin evidence (receipt, bundle and stage) stays private and unchanged.
- No local absolute path or operator string is published here.
- A neutral receipt would be a new identity. It never replaces the original and never carries the original's hash.

## Identities (checked 2026-10-20)

| Role | Value |
| --- | --- |
| Darwin measured source | `bda633b3589e201737b02cf2060f7b24d41a45a7` |
| Reviewed tools / `base/4314-security-darwin-evidence` head | `7e79458304878d51bafd29e24ec25e93b84893ca` |
| `main` head | `013b0ab8fe5059e64ac0f7592a6eb938ac476e29` |
| Cargo.lock: `main` / base | `230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138` / `563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd` |
| Reviewed transition | `rustls-security-4314`, manifest `fe689aca3a29b012ca7fb96a3aad3dc9fc507f117a18be501e379dae2b3f04ff`, old lock from `21d6807e442bbed61e578c165a91d64c4a7dc141` |

## Saved Linux artifact: present and verified

| Check | Result |
| --- | --- |
| GitHub artifact `12213032790` (`island-size-linux-x64`, run `39302891207`, job `118232731946`) | `expired: false`, expires `2026-10-26T18:46:58Z`, 666,751 bytes, digest `sha256:369e9abfd79bbf08e610e4bf68eb5d99eb87d4f5cd7eb4994f0ec562b1e5f00b`, head `0355ad78a4f8d2c240844d2b48da7c2769f6de25` |
| Retained copy in the project's private folder | SHA-256 `369e9abf…f00b`, equal to GitHub's digest. `unzip -t` reports no errors; 98 files, 1,879,730 bytes uncompressed. |

The retained copy stays valid evidence after GitHub's expiry, because its digest is recorded here and on #4314. Expiry alone is no reason to remeasure. A Linux re-stage can read the retained copy. Downloading again is unnecessary.

## Hash inventory

Each kind of evidence is listed separately. A hash is never reused across rows.

### Darwin, original (private; Mac collection, stage attempt 2)

Values are as the collecting operator reported them. Cloud cannot recompute them, because the originals are not here.

| Item | SHA-256 |
| --- | --- |
| receipt | `cfae5404e8e6a0f61c045eb024369cf28321669c9160bb3a3532108d0131e50c` |
| bundle (inventory digest) | `136395016503fb8062e8a902cfc5e11191461325952e0216f09f6ffe0a03c83e` |
| stage-manifest.json | `d7587589b07e0efa4531584655e614824ffd865272745d6113a1af8078e20d20` |
| staged `decision.json` | `70eff8ca8e0d91e986140f70bc8ea32d309fbac8eb1f0f06fbea8f8cb1b00ad0` |
| staged test file | `f2a99be508d4b198aa43b748d5cb1fc428b17cb022b926c646d72c2c8a430114` |
| staged.diff | `942fd70b68fdebc86583cd324f49776617da3112da936017f348a83d5f4fbf16` |
| measured zfb binary | `41af55ff18943b8d72bc0bdcfab55b43b5fe736ab660ec524dfb06e17cdbb871` |

Stage attempt 2 is **not applicable**, for two reasons:

- Its staged `decision.json` embeds the receipt, so applying it would publish private paths.
- Its README `live` hash is the measured source's, `1d76d181…`, which is neither base's nor `main`'s.

### Darwin, derived (neutral receipt N)

None exists. N, its stage (attempt 3) and its manifest are **not created**. They wait for D1.

### Linux, stage-01 (Cloud, from the artifact above; never applied)

| Item | SHA-256 |
| --- | --- |
| stage-manifest.json | `9b2746b9e25adc79ec85622296657e4e1dd5ce76c23f36e64c0ac2b481d5ef52` |
| staged.diff | `fddf15d51bc5d7c4b2d99ad6bc66cbd0c120b9cbb8eae28bfc303269a5200a07` |
| staged `decision-linux-x64.json` | `65ea5dd69d0c3294cd24e128388b74353ba43ad767e714be2be908d3ab9191d7` |
| staged README | `69c46258dd57ed6f626a3d1b86fca43496f9098ef9ce83f79d6b5e5d6179eac4` |
| staged test file | `63f2072f…`, identical to live |

The project's retained copy passes `sha256sum -c` for every file. The public copy is in [stage-evidence/linux-4314](stage-evidence/linux-4314/RESULT.md).

### Destinations (live bytes)

| File | `bda633b` | base `7e79458` | `main` `013b0ab8` |
| --- | --- | --- | --- |
| `research/v3-island-size/README.md` | `1d76d181…` | `e4496a7b…` | `e4496a7b…` |
| `research/v3-island-size/decision.json` | `cc32aa6d…` | `cc32aa6d…` | `cc32aa6d…` |
| `research/v3-island-size/decision-linux-x64.json` | — | `f5055202…` | `f5055202…` |
| `scripts/__tests__/island-size-budget.test.mjs` | `63f2072f…` | `63f2072f…` | `63f2072f…` |

## Where each piece of evidence applies

- **Darwin.** A local-native stage applies only from a clean checkout whose HEAD is the measured source. `--apply` re-verifies this, together with the receipt, the bundle and the tools checkout (`scripts/island-size-local-evidence.mjs:205-256`). Any Darwin stage, attempt 2 or a future attempt 3, therefore lands from `bda633b` and reaches base by a regular merge. It never lands on base or `main` directly.
- **Linux.** Stage-01's `live` hashes equal base `7e79458`. The stager does not bind the checkout tree, so a re-stage at a later head reads the same artifact, but only with that head's live files. Stage-01 stops being applicable as soon as Darwin changes README or the test file.
- **Kinds of evidence are not interchangeable:**
  - Stage documents (recipes, manifests, diffs) describe a candidate; they are not measurements.
  - A replay of saved artifact bytes is not a native measurement.
  - Darwin native evidence is the Mac collection; Linux native evidence is the hosted-runner artifact.

  None of these substitutes for another.
- **Locks are separate.** `rustls-security-4314` admits exactly `+rustls@0.23.46 / -rustls@0.23.45`. A direct-V8 or Rolldown lock change needs its own reviewed transition, and this one does not cover it. #4310 changed no lock and admits no new lock.

## Decision packet (one request, three decisions)

### D1: owner attestation of a neutral receipt N

**Question.** May a new receipt N be made from the unchanged private bundle and attested by the owner as a new receipt? N would carry the same source, platform, `collectedAt` and bundle digest. Absolute paths in `commands` and `bundle.location` would be replaced by a fixed placeholder list.

**Why it is needed.** The stager writes "re-pinned from owner-attested local native evidence <receipt SHA-256>" into the published `platformStatus` and README (`scripts/rebaseline-island-size-darwin.mjs:928-929`, `1000-1002`). For N, that text is true only if the owner attests N.

**If yes:**

1. On the Mac: create N without touching the original, check it, and stage attempt 3 into a new directory. No build or measurement.
2. Review attempt 3 independently.
3. Apply it from a clean `bda633b` worktree.

Each step needs the batched approval below.

**If no:** the Darwin transition cannot be applied publicly with the current tooling. The alternatives are:

- **(i)** A new Darwin collection with neutral paths from the start. This is new Mac work: a cold build and a new identity.
- **(ii)** A reviewed tooling change so that a published contract carries only the receipt digest. This is a change to admission tooling, outside the current approvals.
- **(iii)** Keep `base/4314` red on the Darwin lock pin and hold.

**Stop conditions:**

- the original receipt's hash changes
- N differs outside the listed fields
- a path or identity scan finds anything
- attempt 3 moves a ceiling, the allowance, or anything other than the lock transition and the receipt

### D2: Linux history test, the actual-gzip assertion

**Question.** May the test "moves the live Linux contract … only by the paired cargo-internal-rewire" be renamed and changed? It would assert that `actualGzip` equals the predecessor contract's values plus exactly these 11 deltas, and nothing else.

**Facts, verified 2026-10-20 by comparing live base `decision-linux-x64.json` with staged stage-01:**

- 16 keys, the same key set before and after.
- Exactly these 11 change:

  | Mode | Fixture | Live → staged | Δ bytes |
  | --- | --- | --- | --- |
  | workspace | blog-theme | 22355 → 22352 | −3 |
  | workspace | json-api | 22299 → 22296 | −3 |
  | workspace | model | 22217 → 22215 | −2 |
  | workspace | scalar-signal | 22229 → 22226 | −3 |
  | workspace | show-for | 22285 → 22288 | +3 |
  | packed | blog-theme | 22366 → 22368 | +2 |
  | packed | event-only | 22214 → 22213 | −1 |
  | packed | json-api | 22310 → 22314 | +4 |
  | packed | model | 22223 → 22224 | +1 |
  | packed | multi-island | 22306 → 22309 | +3 |
  | packed | scalar-signal | 22217 → 22216 | −1 |

- Everything outside the movable keys (`outcome`, `baselineSourceSha`, `platformStatus`, `baselineEvidence`, `toolchain.cargoLockSha256`) is byte-equal. That includes every ceiling and normalized total. The allowance stays raw 0 / gzipNormalized 0.
- Actual gzip is evidence only and is never compared with a ceiling.
- The Darwin stage reported the same 11 deltas. That comes from the shared diff text and was not recomputed here.

**If yes:** the assertion stays exact, with the expected values updated by the recorded deltas. Nothing is dropped.

**If no:** keep the test as it is. The Linux apply then cannot land, because that test fails.

**Review:** fresh-context review before any merge request.

### D3: may the operator string be published?

**Question.** May the receipt's `operator` field be published? Its value is not repeated here. It is a display name with a short description of the environment, and holds no path, username, email or credential.

**If no:** N also replaces `operator` with a neutral value, which is listed in the derived report. The original is unchanged.

## What the single approval would cover, and what it would not

**Covers, each step stopping on its own conditions:**

1. Mac stage-only: create N and stage attempt 3.
2. Mac `--apply` of attempt 3 from a clean `bda633b` worktree. No push from the Mac.
3. Cloud:
   - commit those bytes plus the derived report on a new branch from `bda633b`
   - merge base into it with a regular merge
   - re-stage Linux from the retained artifact, then `--apply` it
   - make the reviewed test edits, run the fresh-context review and the focused checks
   - open a draft PR to `base/4314`
4. A regular merge of that draft PR into `base/4314`.

**Does not cover:**

- `base/4314` → `main`
- #4317
- an admission waiver, or any ceiling, allowance or gate change
- release or publication

**Deadline.** Nothing in D1–D3 depends on GitHub keeping the artifact, because the retained copy is verified. Re-staging before 2026-10-26 still avoids relying on the copy alone.
