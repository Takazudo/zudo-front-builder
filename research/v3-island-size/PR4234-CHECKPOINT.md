# PR #4234: refreshed candidate requires new evidence

This checkpoint separates historical evidence from the candidate after merging
main `308160a944d86880d7d1222a2ebca584825f8cea` (including Windows repair #4286).
The merge commit is `101cbac6`; the final candidate includes this documentation.
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

## Refreshed candidate verification

Main merged without conflicts. The existing Git `--no-replace-objects` hardening,
immutable lock and immediate-predecessor provenance, paired transition checks and
strict verification remain intact. No new pin, threshold, allowance, runtime default
or release change is authorized. PR #4260, child #4259 and V8 PR #4242 remain separate.

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

Keep PR #4234 draft and unmerged pending the integration decision. New exact-head
CI, supported measurements, both-platform control/replay and the integration decision
are separate gates; historical receipts cannot satisfy them for a new head.
