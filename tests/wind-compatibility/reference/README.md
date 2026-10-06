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

Run the full required `health` lane against a clean checkout of the exact
integration source. The optional Chromium-only `wind-computed-style` workflow
does not qualify. The retained `wind-required-health-evidence` must be from
`ci-run.sh full`, with a passing three-engine comparison and authenticated
production shipping evidence from that same Linux runner.

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

## Checked promotion

The accepted-reference transition is performed only by
`reference-cli.mjs promote`. It replays the comparison against the current
inputs, checks the reviewed classification and shipping report, then writes the
accepted and reviewed-through records atomically. Run both commands on the same
Linux runner while the production, dist, and browser executables and original
report trees from the successful full run are still present. Keep the
classification file outside the checkout. For GitHub Actions, use the same job
that produced the full evidence; downloading reports to another machine does
not preserve the executable identity required by shipping validation.

```sh
EVIDENCE="${RUNNER_TEMP:?}/wind-gate"
CACHE="${RUNNER_TEMP:?}/wind-reference-cache"
CLASSIFICATION="${RUNNER_TEMP:?}/wind-reference-classification.json"

node scripts/wind-compatibility/reference-cli.mjs promote \
  --plan "$EVIDENCE/plan.json" \
  --assessment "$EVIDENCE/assessment.json" \
  --cache "$CACHE" \
  --output "$EVIDENCE/comparison" \
  --classification "$CLASSIFICATION" \
  --shipping "$EVIDENCE/shipping/report.json" \
  --shipping-output "$EVIDENCE/shipping" \
  > "$EVIDENCE/admission-dry-run.json"

node scripts/wind-compatibility/reference-cli.mjs promote \
  --plan "$EVIDENCE/plan.json" \
  --assessment "$EVIDENCE/assessment.json" \
  --cache "$CACHE" \
  --output "$EVIDENCE/comparison" \
  --classification "$CLASSIFICATION" \
  --shipping "$EVIDENCE/shipping/report.json" \
  --shipping-output "$EVIDENCE/shipping" \
  --apply yes \
  > "$EVIDENCE/admission-applied.json"
```

Before applying, independently inspect the comparison, shipping report,
classification, and exact case/difference membership. The classification must
name every observed upstream, browser, control, Wind, and inventory change with
the exact report identity and a rationale; `accept` cannot contain unexplained
drift. Preserve both command outputs and compare their `next` state values and
tested-input identities exactly. Keep the two resulting JSON state records as
the original pair from this checked transition; do not hand-edit either one.

Only `accepted.json` and `reviewed-through.json` are excluded from the schema 2
tested-input digest, to avoid a hash-of-itself cycle. Every other tracked or
unignored file, including Markdown/MDX, fixtures, helpers, lockfiles, generated
compatibility data, and workflow contracts, is part of the digest. After the
successful run, change none of those inputs before dry-run and apply. Commit
only the two transition records after verifying the pair matches the
dry-run/applied `next` values. The source commit used as
`finalVerificationSha` is recorded by the CLI; the later metadata commit is not
part of its own tested preimage.

Admission remains incomplete while any required result is missing, red,
cancelled, skipped, or deferred. Source-inspected inventory rows stay at
`source-inspected`; this bounded baseline does not claim universal Tailwind or
all-platform compatibility.
