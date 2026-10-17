# PR #4234: refreshed candidate requires new evidence

This checkpoint separates historical evidence from the candidate after merging
released main `d70dd6d94ab1b89a65fee9ed7727747d1de860f2` (v4.5.3, including Windows repair #4286).
The release reconciliation merge is `61c01f01`; the final candidate includes this documentation.
Use the full final PR head recorded in the PR receipt for every new request.

## Historical receipts remain bound to their original source

The old candidate is `35a68f1711333bbab9d34ecd41b8718b28fcd613`.
[Native control 38854369335](https://github.com/Takazudo/zudo-front-builder/actions/runs/38854369335)
succeeded on Linux x64 and Darwin arm64, with enforcement enabled and CSS carriers disabled.

| Platform | Artifact | ZIP SHA-256 |
| --- | --- | --- |
| Linux x64 | 12026876906 | `b0de24e99650a86aa8dd944fa6be94593c5d686eab8e8d89a67a5e1bdb386955` |
| Darwin arm64 | 12027425143 | `a40f5633c77d2ea35138607638a6761c44d2a053ab2b4464d572f4a09cefdfda` |

The [Linux replay receipt](https://github.com/Takazudo/zudo-front-builder/pull/4234#issuecomment-6208765404)
and [independent Mac replay receipt](https://github.com/Takazudo/zudo-front-builder/pull/4234#issuecomment-6208744212)
record digest verification, saved-byte recomputation, checkout identities and unchanged ceilings.
These are real historical results, not missing work and not qualification of the refreshed head.
The old [Health run 38852240208](https://github.com/Takazudo/zudo-front-builder/actions/runs/38852240208)
remains failed; later main repairs do not retroactively change its result.

The intermediate candidate `64b9be27ab425f8cb4441d6ffb83e5d55ca8e12f`
incorporated main `308160a9`: its 440 focused tests, independent review, five green
workflows and Linux Health measurement in run 38891170976 are also historical now.
[That receipt](https://github.com/Takazudo/zudo-front-builder/pull/4234#issuecomment-6214490597)
retains measured merge SHA `93c741d18e3ed0ed5436168f87f03b97310352c5` and artifact

12042618220. Cloud could read its logs but the ZIP returned HTTP Forbidden; its

metadata/upload digest is not locally verified. Do not retry blocked artifact storage.

## Refreshed candidate verification

The release merge resolved conflicts in the README and Linux contract by retaining
both dated narratives and the exact released-main Linux contract. The release-owner
[probe checkpoint](https://github.com/Takazudo/zudo-front-builder/pull/4288#issuecomment-6214128473)
says measurements were carried into the bump commit. No new baseline was inferred.
Linux's superseded Cargo-transition predecessor metadata remains historical; it is
not transplanted into the v4.5.3 release baseline. Darwin's existing authenticated
predecessor metadata is retained. Released package versions, locks, defaults and
ceilings are preserved. The existing Git `--no-replace-objects` hardening,
immutable lock and immediate-predecessor provenance, paired transition checks and
strict verification remain intact. No new pin, threshold, allowance, runtime default
or additional release change is authorized. PR #4260, child #4259 and V8 PR #4242 remain separate.

The manager runs the nine focused suites (Linux/Darwin rebaseline, reviewed Cargo
transition CLI, Cargo producer transition, budget, lock fingerprint, identity
normalization, Darwin workflow and showcase tools), obtains independent review,
and records exact final-source outcomes plus terminal CI in the PR receipt.
A successful upload step alone is not a successful measurement. Linux Health
measurement, when it runs, must retain its distinct PR head and measured merge SHA.
No local Cloud run substitutes for a native platform receipt.

## Exact host requests after the final push

The existing Linux/WSL2 lane owns the sole both-platform dispatch and artifact
collection. First refresh PR #4234 and confirm the full final SHA from its receipt;
check that no matching control has already been dispatched. Then run:

```sh
CANDIDATE='<full final PR4234 SHA from the current receipt>'
gh workflow run island-size-darwin.yml \
  --repo Takazudo/zudo-front-builder \
  --ref base/post-4-5-0-island-maintenance \
  -f ref="$CANDIDATE" -f platform=both -f enforce=true -f css_carriers=false
```

Record the workflow/run/job IDs and actual source SHA. Download both original ZIPs,
verify SHA-256 against GitHub/upload digests, preserve them, and replay read-only
through the final candidate's exported loaders in a clean exact-SHA checkout.
Verify sidecars by recomputing from saved bytes, producer/checkout/lock/fingerprint
identity and all eight fixture totals in both modes. Preserve failed evidence.
Do not stage, apply, rewrite sidecars or change contracts. Report hosted budget
checker results separately from off-runner replay and declare any path substitutions.

The Mac lane independently downloads and verifies the Darwin artifact from that
same run, confirms native host identity and exact candidate checkout, and performs
its read-only loader/strict-budget replay with unchanged artifact-byte digests.
It does not dispatch a duplicate run. Report unavailable steps as unavailable.
The released main still carries a v4.5.2 Darwin baseline alongside v4.5.3 packages;
record exactly which strict checks accept or reject that evidence. A new baseline
or policy decision requires separate authorization, not a softened assertion or
silent re-pin. Preserve the original failed run if any check rejects the candidate.

Keep PR #4234 draft and unmerged pending the integration decision. New exact-head
CI, supported measurements, both-platform control/replay and the integration decision
are separate gates; historical receipts cannot satisfy them for a new head.
