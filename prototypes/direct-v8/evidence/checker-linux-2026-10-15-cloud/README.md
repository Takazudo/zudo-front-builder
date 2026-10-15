# Patched-source Linux acceptance

Refs #4258. Native acceptance passed at clean source
`cc56b15a38173b9a276cb997ef12230fdc370cd0`, containing the hash-verified checker
patch and independently reviewed nesting repair. The later evidence-only commit
was not natively executed; only these receipts and FINDINGS changed afterward.

- `acceptance.json`: exact commands, tool versions, 45 native/9 shared tests,
  all six gates, engine receipts and repeated executable identity checks.
- Individual gate reports: actual checker observations, including strict protocol
  counts, before/after executable hashes and loaded corpus identity.
- `generated-results.json` and `generated-manifest.json`: new frozen producer
  input, authored source mapping, exact parity and real Chromium observation.
- `review.json`: independent proposed-code and nesting-repair review, real focused
  regression results and separately labeled historical-report compatibility.
- `session.json`: patch recovery/hash, original regression failures, combined
  64 → 92 → 95 test inventory, setup, source distinctions and remaining holds.

JSON copies use repository formatting without changing values. Manifest and
original-output digests identify original run bytes under the ignored
`target/direct-v8-accept-checker-20261015-cc56b15a/`, not formatted copies.
Binaries, frozen working outputs and large logs remain ignored. All historical
receipts are preserved; earlier patch-transfer blockers were resolved by a
verified exact-text handoff, not by bypassing the prior upload denial.

The first patched-source native replay passed; no native retry was needed.
The earlier baseline's missing frozen JavaScript dependency failure remains in
the preceding evidence directory. No deadline or test expectation was weakened.
All 54 frozen files and six native executable digests/permissions were checked
again after the replay. Both engine input files are byte-identical. Snapshot
generation was fresh; equality to an older snapshot was not required.

This is standalone Linux host evidence, not candidate CLI adoption, Darwin size
evidence, live activation, a cold-build benchmark or proof of child-tree cleanup.
The prerequisite PRs remain held; phase one is not complete. Final pushed head
and exact-head CI observations are recorded in the PR discussion.
