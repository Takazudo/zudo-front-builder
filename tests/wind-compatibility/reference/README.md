# First bounded reference admission

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
