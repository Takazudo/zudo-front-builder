# Live activation checklist and next runtime milestone

This checklist tracks the prepared `cargo-internal-rewire` transition for
[#4258](https://github.com/Takazudo/zudo-front-builder/issues/4258). It is an
execution procedure for a later authorized activation, not authorization to
merge, dispatch native probes, or change live contracts in this continuation.
The standalone direct-V8 implementation and its original receipts remain in
[#4242](https://github.com/Takazudo/zudo-front-builder/pull/4242).

## 1. Exact identities and evidence boundaries

| Input | Value |
| --- | --- |
| Immutable old lock source | `6fb3fbe0103c5c90799c3262ca5c1ab9ade0f27e:Cargo.lock` |
| Old lock SHA-256 | `2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e` |
| Prepared new lock SHA-256 | `d0f3e6764ef9233e74ea89a931f9ebfcd6b05f429db36e5ce65f2ddd5685b1bb` |
| Current prepared-input source | `1170596fc09de6ad8e3f7a0f33349e310320f2a5`; future collection must record its own actual source and revalidate these inputs |
| Declaration | `prototypes/direct-v8/evidence/cargo-rewire/declaration.json`: add unpublished `zfb-js-host@0.0.0`, rewire unpublished `zfb-render@0.0.0` by removing direct `sha1`/`sha2` and adding `zfb-js-host` |
| Closure receipts | `evidence/cargo-rewire/closure-{old,new}-{x86_64-unknown-linux-gnu,aarch64-apple-darwin}.json`: Linux 496→497 rows, Darwin 480→481; one declared internal row joins, no external drift |
| Live contracts | `research/v3-island-size/decision-linux-x64.json` and `decision.json`; unchanged by this continuation |

The old source is **not a moving `main` reference**. Read its bytes with
`git show 6fb3fbe0103c5c90799c3262ca5c1ab9ade0f27e:Cargo.lock` even after
integration. The new reference must also bind the actual manifests, not merely
another commit with equal lock bytes.

Keep these identities separate in the activation record:

- **Probe head:** Linux PR head supplied as `--head-sha`.
- **Measured source:** actual checked-out SHA in measurement provenance and
  `--source-sha`; normally the PR merge checkout on Linux, not the probe head.
- **Tooling source:** full commit whose re-pin scripts perform staging/apply.
- **Contract-containing commit:** immutable commit holding each predecessor's
  exact contract bytes, supplied as `--previous-contract-ref` when needed.
  This is not necessarily its `baselineSourceSha` or measured source.

The historical native acceptance receipt
`evidence/acceptance-linux-2026-10-14.json` binds clean source
`34ffcba76fa54e88745656c609b2e5a178ae4227`. Through prepared-input source
`1170596fc09de6ad8e3f7a0f33349e310320f2a5`, the intervening changes are
prototype documentation and evidence only. This continuation refreshes Wind
preview generated assets and activation documentation and reconciles integrated
main changes: #4253's draft-preview guard and #4256's dev-server asset-serving
repair. The latter changes compiler source inputs, so Wind is rebuilt and
regenerated from the refreshed workspace; compiler-input equality is not claimed.
Independent review confirmed that the exercised standalone host, typed-config,
build-generated producer, harness and corpus paths remain unchanged. That scoped
input equivalence supports reuse of the native replay, not a claim that it
executed this continuation's final head or verified the changed dev-server paths.
It also does not constitute Linux/Darwin island-size evidence. Final-head CI,
dev-server checks and genuine generated-asset freshness are reported separately
in the PR.

## 2. Ordered activation procedure

### A. Infrastructure is available

- [ ] Foundation [#4234](https://github.com/Takazudo/zudo-front-builder/pull/4234)
  has crossed its separately authorized external-impact integration boundary.
- [ ] Cargo admission [#4259](https://github.com/Takazudo/zudo-front-builder/pull/4259)
  is actually integrated after its foundation. Its reviewed capability exists
  at `714e65cd01ab674d668aac7db1a3a5a31c03e9e7`; that draft is not activation.
- [ ] Linux evidence reachability [#4260](https://github.com/Takazudo/zudo-front-builder/pull/4260)
  is integrated where the measurement runs. It is an independent main-based
  repair, not an import of the held foundation/admission stack into #4242.

The implemented Health route retains one native build and the existing Linux
producer. Collection is allowed after early workspace tests fail only when its
explicit setup/build/package prerequisites succeeded and the job is not
cancelled. Workspace tests and enforced budget rejection still fail required
`health`. The budget wrapper keeps the checker exit code and diagnostics unchanged.
It emits `complete=true` only after successful checking or the exact three
old-Cargo-pin errors, plus source, row, sidecar and unchanged-total validation.
Only that output enables upload of the existing `real-workspace/` and
`real-packed/` trees. Missing or partial output is diagnostic, never qualifying
evidence. No extra cold V8 build or platform matrix is needed.

These dependencies gate live activation, not independent Wind regeneration,
orchestration tests, review, or this documentation repair. Keep drafts protected
by the actual showcase/docs preview predicates while ordinary PR validation runs.

### B. Refresh the runtime branch against integrated dependencies

- [ ] Refresh #4242 against the main that actually contains A; do not copy
  unmerged #4234/#4259 sources into the runtime branch.
- [ ] Record that main SHA, runtime head, tooling SHA, and both immutable
  predecessor-contract sources. Inspect preview/deployment predicates again.
- [ ] Revalidate both lock hashes, declaration, unpublished workspace manifests
  and all four closure receipts after any relevant source/manifest/lock change.
  A changed lock requires a newly reviewed declaration; do not rewrite the
  expected hash to admit unexplained movement.

For closure regeneration, use a clean source checkout of the immutable old
commit and the refreshed candidate checkout. Do not replace the live checkout's
lock to simulate the old graph. These metadata commands do not compile either
platform or constitute native measurements. Set `OLD_CHECKOUT` and
`NEW_CHECKOUT` to those actual directories and `RECEIPTS` to a new scratch
output directory; run the integrated tooling from its recorded checkout:

```sh
for target in x86_64-unknown-linux-gnu aarch64-apple-darwin; do
  node scripts/record-cargo-producer-closure.mjs --producer zfb \
    --root "$OLD_CHECKOUT" --target "$target" \
    --output "$RECEIPTS/closure-old-$target.json"
  node scripts/record-cargo-producer-closure.mjs --producer zfb \
    --root "$NEW_CHECKOUT" --target "$target" \
    --output "$RECEIPTS/closure-new-$target.json"
done
```

Compare schema, producer, target, edges, lock digest and every row by package
identity (normalizing only row order) with
the committed receipts. Recorder command/Cargo version fields can differ by
host and must be recorded honestly; do not demand byte equality across hosts.
Review any semantic delta and bind the actual receipts' digests when staging.
The re-pin proof checks the declaration against the entire lock and referenced
manifests. No external package/version/source/feature drift is admissible.

Continuation validation used #4259 tooling at
`714e65cd01ab674d668aac7db1a3a5a31c03e9e7` against the prepared runtime
source above. Both real lock/manifest/declaration/closure proofs passed. Fresh
metadata resolution with Cargo 1.95.0 matched all four saved semantic row sets
(496/497 Linux, 480/481 Darwin); initial byte-order-sensitive comparison was
replaced by package-identity comparison after confirming only row order differed.
The declaration digest remains
`5ea09a4dbcb01a74d1c62aad10485e3ce363e0b463b6fe38e396fe7b0ba3043e`.
This is metadata/proof validation, not native evidence or live admission.

### C. Collect native evidence from compatible identified sources

- [ ] Linux: ordinary #4242 PR Health validation, `health.yml`, job `health`,
  artifact **`island-size-linux-x64`** (seven-day retention). Record run/job,
  artifact id/expiry, probe head and actual checkout SHA from the saved
  provenance. The repaired route reaches collection despite early contract
  assertion failures when its real prerequisites succeed.
- [ ] Darwin arm64: only after separate dispatch authorization, use
  `island-size-darwin.yml` with `ref=<recorded source>`,
  `platform=darwin-arm64`, `enforce=true`, `css_carriers=false`. Retain
  **`island-size-darwin-arm64-<source sha>`** and its run/job/id/expiry.
  Darwin must measure the Linux merge source or its probe head; staging checks
  this compatibility. Record which one was used, never relabel the artifact.
- [ ] Preserve both workspace and packed rows, actual saved bytes, normalized
  sidecars, binary/package hashes, pinned Node/zlib and platform provenance,
  checker exit code and full log. Download before expiry without flattening or
  editing the accepted artifact layout.
- [ ] Classify every failure. An initial enforced rejection exclusively for
  the old Cargo pin is retained input, not a pass. The known early workspace
  Cargo-pin assertions must likewise be identified explicitly. A missing
  artifact, failed prerequisite, additional unexplained failure, stale output,
  altered sidecar, or moved per-fixture raw/normalized total blocks activation.
  **Decreases are refused as well as increases.**

The three checker signatures for an initial old-pin-only rejection are
`local Cargo.lock: SHA-256 differs from the reviewed contract`,
`workspace provenance: Cargo lockfile differs from the contract`, and
`packed provenance: Cargo lockfile differs from the contract`.
Do not accept an arbitrary red run merely because one of those lines occurs.
The staging consumers independently recompute totals from saved bytes and
validate identities/sidecars; logs alone do not prove complete evidence.

### D. Stage and review both platforms as one transition

Use the refreshed checkout with the recorded integrated tooling, unchanged live
contracts, and reviewed receipts. Set the variables below from actual run/API
records (full 40-character source SHAs and unexpired artifact metadata), not
from branch-name guesses. `NEW_CARGO_REF` identifies the candidate lock and
manifests; `PREVIOUS_LINUX_REF` and `PREVIOUS_DARWIN_REF` identify exact existing
contract bytes. `STAGE` is a new scratch directory, shared by both commands.

```sh
OLD_CARGO_SHA=2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e
NEW_CARGO_SHA=d0f3e6764ef9233e74ea89a931f9ebfcd6b05f429db36e5ce65f2ddd5685b1bb
OLD_CARGO_REF=6fb3fbe0103c5c90799c3262ca5c1ab9ade0f27e
DECLARATION=prototypes/direct-v8/evidence/cargo-rewire/declaration.json

node scripts/rebaseline-island-size-linux.mjs --artifact "$LINUX_ARTIFACT" \
  --run-id "$LINUX_RUN" --job-id "$LINUX_JOB" \
  --head-sha "$LINUX_HEAD" --source-sha "$LINUX_SOURCE" \
  --previous-contract-ref "$PREVIOUS_LINUX_REF" \
  --reason cargo-internal-rewire --expected-old-cargo-sha "$OLD_CARGO_SHA" \
  --expected-new-cargo-sha "$NEW_CARGO_SHA" --old-cargo-ref "$OLD_CARGO_REF" \
  --new-cargo-ref "$NEW_CARGO_REF" --rewire-declaration "$DECLARATION" \
  --old-closure "$RECEIPTS/closure-old-x86_64-unknown-linux-gnu.json" \
  --new-closure "$RECEIPTS/closure-new-x86_64-unknown-linux-gnu.json" \
  --stage "$STAGE"

node scripts/rebaseline-island-size-darwin.mjs --artifact "$DARWIN_ARTIFACT" \
  --run-id "$DARWIN_RUN" --job-id "$DARWIN_JOB" --artifact-id "$DARWIN_ARTIFACT_ID" \
  --artifact-expires-at "$DARWIN_EXPIRES" --source-sha "$DARWIN_SOURCE" \
  --previous-contract-ref "$PREVIOUS_DARWIN_REF" \
  --reason cargo-internal-rewire --expected-old-cargo-sha "$OLD_CARGO_SHA" \
  --expected-new-cargo-sha "$NEW_CARGO_SHA" --old-cargo-ref "$OLD_CARGO_REF" \
  --new-cargo-ref "$NEW_CARGO_REF" --rewire-declaration "$DECLARATION" \
  --old-closure "$RECEIPTS/closure-old-aarch64-apple-darwin.json" \
  --new-closure "$RECEIPTS/closure-new-aarch64-apple-darwin.json" \
  --stage "$STAGE"
```

- [ ] Review `stage-manifest.json`, both run identities and the complete staged
  diff. The pair must bind the same reason, declaration digest and old/new lock
  transition, with compatible measured sources.
- [ ] Contract movement is limited to `toolchain.cargoLockSha256`, `outcome`,
  `baselineSourceSha`, `platformStatus`, and `baselineEvidence`; accompanying
  README and budget-test pins reflect the same pair. All per-fixture totals,
  numerical ceilings, allowance, metric, normalizer, fixture/runner identities,
  pnpm fingerprint, unrelated toolchain identities and versions stay fixed.
- [ ] A partial stage or changed live/staged hash must be discarded and staged
  again from reviewed inputs; there is no force path. Two platform fixture
  successes do not substitute for the native artifacts above.

### E. Apply the pair, then obtain final enforced verification

Only after explicit live-activation authorization and review of D:

```sh
node scripts/rebaseline-island-size-linux.mjs --apply "$STAGE"
git diff --check
git diff -- research/v3-island-size scripts/__tests__/island-size-budget.test.mjs
```

- [ ] Review and commit both platforms together. Apply verifies live/staged
  hashes, prepares temporary siblings and attempts rollback on rename failure;
  it does not promise multi-file crash atomicity. Inspect all files after any
  interrupted apply before continuing.
- [ ] Run affected budget/re-pin tests and obtain final-head enforced Linux
  Health and separately authorized enforced Darwin verification. Both must
  pass with no provenance mismatch. Reconcile previously upstream-skipped
  Health steps at that exact head and record passed/failed/skipped/cancelled/
  pending separately; older green runs cannot fill these gaps.
- [ ] Record the new contract-containing commit separately from both measured
  sources and tooling source. Only then describe the producer transition as
  activated and its integration checks as verified. CLI migration is still
  outside this milestone.

This continuation stops before A's held integrations and before C's manual
Darwin dispatch or E's live apply. It repairs independent collection/freshness
work and documents the route; it neither merges the drafts nor closes delivery
issues, changes live contracts, or asserts phase-one integration is complete.

## 3. Next runtime milestone (recorded, not executed)

After phase one is accepted and the producer transition is admitted:

1. **Actual CLI typed-config path.** Load `zfb.config.ts` through the existing
   `zfb-config-loader` esbuild preparation into the direct-V8 host behind an
   internal build-time opt-in (a non-default Cargo feature on `zfb`, no public
   backend selector), keeping `parse_loaded_config` defaults, presets and
   field-path diagnostics unchanged (`check-outer-config.py` is the oracle).
2. **Render factory / threaded adapter.** Implement `EmbeddedV8HostFactory` and
   `ThreadedV8Host` on the direct host with owned DTOs across the pinned
   thread, one process initializer, deterministic ICU preparation, and prove
   the candidate CLI's selected graph excludes Deno runtime crates
   (`record-cargo-producer-closure.mjs` on the opt-in feature).
3. **Dev lifecycle.** Start-before-swap, failed-replacement rollback and finite
   shutdown through the existing dev supervision tests, plus reload under the
   direct host. Standalone-host evidence from this branch does not prove any
   of this.

Entry conditions recorded by the 2026-10-14 native re-review (FINDINGS.md),
which the phase-2 design must satisfy before any combined binary exists: one
shared V8 process initializer and one thread guard covering both the deno_core
and direct hosts (a second initializer panics), V8 initialized from the main
thread before any host thread is spawned or the unprotected platform mode
chosen deliberately, and a decision on registered error classes
(`instanceof` parity) and on same-specifier module reload semantics.

Five-target packaging/native adoption, Rolldown interoperability (#4120 stays
independent), default switching and final Deno deletion remain later,
separately authorized work.
