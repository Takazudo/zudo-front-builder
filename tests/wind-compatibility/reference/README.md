# First bounded reference admission

The exact 4.3.2 to 4.3.3 manual upstream-following procedure and frozen
target identity are in [`manual-following.md`](./manual-following.md) and
[`manual-following-target.v1.json`](./manual-following-target.v1.json). That
round keeps the admitted 4.3.2 state authoritative while it records complete
4.3.3 assessment and three-way comparison evidence.

## Live state and history

The current admission state is recorded in [`accepted.json`](./accepted.json)
and [`reviewed-through.json`](./reviewed-through.json). Treat those records as
authoritative; this runbook describes the procedure and dated execution
history. The generated EN/JA inventory records source inspection and does not
assert browser compatibility.

### 2026-10-06 — run 37409669895

This manager-owned attempt passed the full comparison, production build, four
Rust build fixtures, and dist proof, then failed shipping: `css-initial` warm
CSS differed from the clean copy. Diagnosis found that the clean-copy filter
removed every directory named `dist`, including the shipping fixture's actual
`node_modules/.../dist/component.js` package asset. This run cannot admit the
reference. Any affected preview assets and shipping behavior must be rebuilt
and the complete evidence rerun before promotion.

### 2026-10-06 — run 37420945247

The manual 4.3.2 to 4.3.3 round ran on tested source
`6ae4aed60b363e8594a7b342f5944fa796a52069` in the
[manager-owned Linux workflow](https://github.com/Takazudo/zudo-front-builder/actions/runs/37420945247).
Plan `sha256:bcb252b1b2fc7ab35382adc9bf79a11e081a7ba85ad246de837a02ae5219a2d0`,
assessment `sha256:c196629e568f74f83b3b6868e9b0e6ae8eaaa95301277aca60d7d2d6bc2b66c7`,
comparison `sha256:babf4cbb5687cad87655001e8a378ba0a9b9d7de7b0bfc0669bd92548fca4b39`,
classification `sha256:738253fa1b6f08056aa3b4cfc92d38497f0d5e2fb75d4301e7abdfeab79684a4`,
and tested-input digest
`sha256:4932df0cd46b0700ea763fccb56c2b38791a6dd2e05e9bf0f1987b03b41944e0`
bind the recorded results to that source. The workflow completed successfully
as an evidence-capture rehearsal; the candidate comparison itself failed.
This profile 1.5 comparison used one Wind build and retained all six accepted
and six candidate engine reports:

| Report | Accepted 4.3.2 | Candidate 4.3.3 |
| --- | --- | --- |
| Pilot Chromium | `sha256:19d2997e3fcff7e42216bcb62a6dae25a055aac8b22cfb2279c197aecd309a08` | `sha256:4241f3cfb177d428840bf19e7a106437a95441847c3476a5ca907993569484eb` |
| Pilot Firefox | `sha256:cdb90b5e675d56cde47f19db4443a17d39b142e7d704f70e3a23475b841ba3eb` | `sha256:5caad9b4da6f01b83794f3dd4e9f3a115224df5dc8755011886384b44d4a9e7b` |
| Pilot WebKit | `sha256:10c1c1dbc0ebc69ccd83918f9c7c10e52847b0c305bb22b3749a47a14bd98f52` | `sha256:ede77c6529e5a29c01f462cd64796cad19340c1d93446866c883583d17fb3d8e` |
| Corpus Chromium | `sha256:be1b658c75143a662f8637b8b69af59db56eccaed59cca65bc95b2654224ff94` | `sha256:c9c3713a3cb75c7c0f795ecb0d64ecd0fc714afb7a4df2be6ce3371d061a59b4` |
| Corpus Firefox | `sha256:3a77646bd5f36546aa169f162c9a4f7a0e7fb5d9db1452faddfeae35aa1b95e6` | `sha256:3c175038402fa3f84d6591c8baea680a12c051d022c96673d33fe5aecc1f186a` |
| Corpus WebKit | `sha256:6e9b7ea88a7b34c635ff437bf7f10c405a7d222d5defc82f7e0b7d9feaade031` | `sha256:1bfc9c02f312622a02a1b0d5749e4bc37ba588e782a213667974162ed1a94af3` |

Assessment captured GitHub release 355054384 and its 15 listed fixes, a
34-file npm artifact with 14 changed paths, a source archive with 541 files /
652 tree items and 77 changed paths, and 23 changed test files. The candidate
artifact is pinned by SRI
`sha512-gOhV3P7ufE62QDGg1zVaTgCR+EtPv92k2nIhVcVKcLmxT1sUBsQGhnZj175j+MqRt4zLF7ic+sCYjfhxMxj7YQ==`
and tarball SHA-256
`f1493c5bcb29a80310da63d19282d233ba259479f36561a27146cd4747d1faef`.
The observed `v4.3.3` source tag points to
`c2b24dd15fed1c59dd521bd86082f520c9f5ad0d`; `packageGitSha` is null and the
tag-to-npm-artifact link remains unverified.

All six accepted reports passed. Candidate evidence retained all planned
pilot (15 cases and 11 controls per engine) and corpus memberships (66
Chromium, 27 Firefox, and 27 WebKit cases); all six candidate reports failed
the profile comparison. The ten failing case IDs in each engine were
`hover-block`, `breakpoint-block`, `hover-flex`, `focus-flex`,
`group-focus-flex`, `peer-focus-flex`, `dark-flex`, `pseudo-before`,
`space-hidden-children`, and `native-sr-reversal`. These are structure
contract mismatches from nested variant selector/media flattening. Across the
358 browser rows, 115 CSS-tree/CSS-rule observations changed, while sampled
value, number, and box observations were unchanged. The 189 Wind CSS rows had
no changes. Of 112 changed upstream CSS files, 79 were version-banner-only;
the 33 substantive changes comprise 30 variant-flattening rows and three
native reset files. Nine of 33 control rows changed. The reset changes include
the font fallback stack moving from `ui-sans-serif`/`system-ui` to a platform
list and Firefox `:-moz-focusring` excluding iframes. Those upstream changes
explain observed native-reset control deltas, but this sample does not establish
global reset equivalence or review their broader semantics. Inventory refresh
reported three dynamic-source-site line shifts in `variants.ts`, with no
utility membership change.

The checked disposition is `review-only`: keep accepted 4.3.2 authoritative
and record 4.3.3 as reviewed-through only. The unresolved profile transition
requires per-utility review of the structure mismatches and reset semantics;
unchanged sampled computed values do not establish profile parity. The
machine-generated `unexplained-drift` labels conservatively mean that no
candidate change was adopted without that review; they do not mean every
version-banner-only row has an unknown cause. This documentation commit was
written after the tested source and is not part of the compared input.

## Evidence required

On a clean checkout of the exact integration source, run the required Wind
policy/inventory/accounting/mutation and empty-token checks, `ci-run.sh full`,
and the generated EN/JA plus preview freshness checks together. The optional
Chromium-only `wind-computed-style` workflow does not qualify. The retained
full-run bundle must contain a passing three-engine comparison and
authenticated production shipping evidence from that Linux x64 run.

Review exact membership and identity, not a percentage:

- `wind-preset-free` profile 1.5, Wind language spec 1.14, and catalog 219;
- the full corpus of 120 cases, with the reviewed 66/27/27 membership and
  outcomes intact for Chromium, Firefox, and WebKit;
- all 15 pilot cases, 11 controls, and 3 extraction contracts for each required
  engine, with no missing or unexpected case IDs;
- exact approved-difference membership, mutation controls, browser and
  toolchain identities, candidate artifact integrity, and tested-input digest;
- successful empty-token, inventory/accounting, and gate-mutation contract
  checks from the required health lane;
- fresh generated English and Japanese compatibility data for the same source;
- passing production build, Rust real-build fixture, dist proof, shipping
  consumer matrix, and existing browser consumer checks.

An expected difference is acceptable only when its exact case and difference
IDs are reviewed. A missing row, changed membership, unexplained mismatch,
failed or skipped required job, stale report, unavailable browser, or changed
input blocks admission. Retain original CI reports and logs outside Git; never
rebuild an expected result from Wind output or reconstruct shipping evidence on
a different runner.

## Ephemeral checked-transition artifact

The permanent required `health` job only produces and verifies evidence; it
does not advance reference state. The temporary #3840 admission probe invokes
the helper below after the full health contracts and `ci-run.sh full` pass, and
before its evidence artifact is uploaded. Do not add this invocation to the
permanent health workflow.

The helper accepts only the first bootstrap: the plan and live state must have
no accepted reference, must target stable `tailwindcss@4.3.2`, and must bind the
passing full comparison to the current source and tested-input digest. It also
checks the exact full report memberships (112 upstream, 358 browser, 33
control, and 189 Wind delta rows), requires `candidateInventory` to remain the
bootstrap-pinned null case, and rejects any non-null change flag or any
`localChanged` flag other than null. These null deltas mean there is no prior
accepted baseline to compare; they do not replace the candidate-versus-oracle
case assertions and shipping checks. Any unexpected row, flag, identity,
profile, or state fails closed and requires renewed review.

Only after those checks does the helper create a fixed empty-change
classification bound to the exact plan and comparison report IDs. It runs
`reference-cli.mjs promote` first as a dry run and then with `--apply yes`; the
existing CLI replays the comparison, validates the shipping evidence, and
writes the two records as a recoverable checked pair. The helper compares the
two transition results and tested-input identities, preserves original report
bytes, copies the paired state records and command outputs under
`wind-gate/admission/`, and restores the checkout's original state bytes in a
`finally` path. The helper and CLI never commit, push, or publish the temporary
records.

```sh
EVIDENCE="${RUNNER_TEMP:?}/wind-gate"
CACHE="${RUNNER_TEMP:?}/wind-reference-cache"
node scripts/wind-compatibility/bootstrap-admission.mjs bootstrap \
  --evidence "$EVIDENCE" \
  --cache "$CACHE"
```

The root-owned temporary admission probe runs this only after its required
health contracts and full evidence step succeed, and before it uploads the
`wind-checked-admission-evidence` artifact. The helper writes the
classification, dry-run/apply outputs, summary, and state pair beneath
`wind-gate/admission/`; downloading a completed ordinary health artifact alone
cannot perform the executable-bound shipping replay.

Review the downloaded full comparison, shipping reports, bootstrap summary,
CLI dry-run/apply outputs, and exact case/difference membership before
committing anything. Verify that the paired records under
`wind-gate/admission/state/` equal both CLI `next` values and carry the exact
source, report, reference, profile, and tested-input identities. Commit only
`accepted.json` and `reviewed-through.json` after independent evidence review;
do not hand-edit either record. If any required evidence is red, missing,
deferred, or fails the bootstrap policy, leave both authoritative records
unchanged.

Only `accepted.json` and `reviewed-through.json` are excluded from the schema 2
tested-input digest, to avoid a hash-of-itself cycle. Every other tracked or
unignored file, including Markdown/MDX, fixtures, helpers, lockfiles, generated
compatibility data, and workflow contracts, is part of the digest. After the
successful run, change none of those inputs before dry-run and apply. The
helper verifies the pair against both CLI `next` values before copying it to
the artifact. The source commit used as `finalVerificationSha` is recorded by
the CLI; the later metadata commit is not part of its own tested preimage.

Admission remains incomplete while any required result is missing, red,
cancelled, skipped, or deferred. Source-inspected inventory rows stay at
`source-inspected`; this bounded baseline does not claim universal Tailwind or
all-platform compatibility.
