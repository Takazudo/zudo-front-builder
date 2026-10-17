# Existing-source Linux replay; checker patch unavailable

Refs #4258. This is a new native observation of clean source
`8bddd9ea8a6256972afeeec284dcd38b2c32452d`, not the requested combined checker
patch replay. The evidence-only commit containing this directory was not itself
natively executed. Its runtime, harness, corpus, producer, lock and toolchain
inputs are unchanged from the measured source.

The required 43,482-byte patch could not be recovered. Both supported Library
materializations (including one bounded retry to an explicit local destination)
failed with `library file transfer failed: download failed`; neither destination
exists. The expected SHA-256 is
`c7a5dc3b570b7c74892a60553d9a147d0e95278191fe38d4faccbaa1e22b6f12`;
actual bytes/hash, applicability and proposed-code review remain unavailable.
No patch was reconstructed, applied or uploaded. The earlier upload denial was
not bypassed.

The existing isolated Python suite passed 64 tests. The unavailable patch's 28
checker tests are not present, so the combined 92-test inventory is unverified.
Independent review reproduced integer status/missing-null acceptance, discarded
extra outer receipts, and malformed/Unicode receipt failures. These remain open;
the successful native observation does not establish checker robustness.

The first native attempt failed while building outer-config because frozen
JavaScript dependencies were absent. After `pnpm@12.8.2 install --frozen-lockfile`,
the new retry passed 45 native tests, 9 shared-service tests, 63 config and 27
service cases per engine, 47 module/worker cases per engine, 8 HTTP cases, 7
outer-config cases, 8 generated packets per engine and 7 exact parity rows.
Counts match the historical corpus; no coverage was removed. Both attempts are
retained and only the retry passed.

A new frozen snapshot fed both engines. Its manifest SHA-256 is
`7cf6dcfb1dec29bfe9afa07a4130f87256c63269ca90f5a0d56cbeda1f15a1ae`.
The authored throw maps to `throw.ts:6:9`, with the authored source content
checked. Chromium 154.0.8037.57 observed `Count: 0` → `Count: 1` and
`rgb(12, 34, 56)`, with no page or console errors. All six executable digests
and execute permissions passed the acceptance entry point's repeated checks.
Engine receipts remain independent Deno-core 0.399.0/V8 147.4.0 and direct
V8 147.4.0. No runtime fallback or pin change was introduced.

`acceptance.json` contains commands, counts, tool identities, engine receipts and
executable checks. The gate reports retain individual observations;
`generated-manifest.json` binds the newly produced input bytes.
`initial-acceptance.json` preserves the original failed attempt.
`session.json` records setup, transfer failure, source/CI distinctions and holds.
`independent-review.json` and `reproductions.json` describe current-source
findings; `historical-compatibility.json` is historical-report validation only.
Large logs, binaries and frozen working outputs remain ignored under `target/`.
Committed JSON uses repository formatting with unchanged values. The manifest
digest and `session.json`'s original-output hashes identify the original run
bytes, not the formatted copies. Independent evidence review confirmed all 54
frozen files, both engines' identical input bytes, the authored mapping, all six
native executable identities and the recorded guard outcomes; no blocking issue
was found.

No new runtime defect was reproduced. No additional workspace/default/no-V8,
Clippy, md-Wasm or CSS-addon checks were run for this evidence-only change.
Previous checks are not relabeled as current results. The non-UTF-8 page
postprocessing limitation remains distinct from Worker binary-response support.
This is not a cold-build benchmark or proof of child-tree cleanup.

#4234, #4259 and #4260 remain open drafts and unmerged. The starting exact SHA
has zero check runs, statuses and unfiltered Actions runs; missing CI is not a
pass. PR metadata still reports the preceding `bcc29ea1` while the actual branch
ref is `8bddd9ea`; the newer ref was preserved. Existing Cargo-provenance
failures are unresolved. Post-push observations belong in the PR discussion.

Next: recover the actual patch through an authorized transfer, review and
reconcile it, run the combined inventory, commit a clean source checkpoint and
replay native acceptance there. Separately, prerequisite integration, compatible
Linux/Darwin size evidence, paired staging/review and authorized live apply remain
gated. No merge, ready transition, held-stack import, live operation or phase-two
CLI adoption occurred. Phase one is not complete.
