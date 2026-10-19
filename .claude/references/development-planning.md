# Development planning policy example

The canonical policy is [CLAUDE.md](../../CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions).
Use this compact block in an epic; children inherit it by reference and add their affected checks.
Replace the example ownership and branch names with the actual plan values.

```text
Policy: CLAUDE.md — Development strategy: integrate first, use CI to find regressions.
Integration: topic branches → reviewed base/example; draft aggregation targets main, no main push/merge.
External effects: inspect actual triggers; draft suppresses preview uploads, not CI/artifact uploads.
CI/fixing owner: implementation manager for this epic.
Checks: child formatting and affected unit checks; manager batches broad CI verification.
Evidence: development head separate from frozen qualification SHA/toolchain/artifacts; first results and retries.
Native: serial command-scoped low-memory builds, reusable identity-keyed caches; no chasing each new head.
Progression: reviewed, dependency-safe development merges may proceed with CI pending.
Red CI: continue independent work only when understood/localized and repair is tracked;
        otherwise pause the affected chain and diagnose/repair before dependent work.
Completion: reconcile required evidence and every alert on the integrated result.
Publication: separate approval and authoritative release verification; no branch-protection bypass.
Exceptions: none (record task-specific deviations and their reasons here).
```

## Dependency example

| Topic | Depends on | Cheap child checks | Manager responsibility |
| --- | --- | --- | --- |
| Parser fix | none | Affected parser regression checks if inexpensive | Review/integrate; track exact-SHA CI |
| Generated fixtures | Parser fix | Inspect expected fixture delta | Start after parser merge; serialize shared generated files |
| Independent docs | none | Formatting and link checks | May proceed during a localized parser repair |
| Confirmation | Parser fix, Generated fixtures, Independent docs | Review evidence; add missing coverage if needed | Reconcile integrated-state alerts; do not invent passed results |

A wave number is descriptive; explicit dependencies gate execution. If parser CI reports an
unknown foundation failure, generated fixtures pause. Independent docs may continue only after
the manager establishes that they do not depend on the broken behavior and records the decision.

## Deterministic plan-review cases

Review a proposed plan against each row. Reject or revise it when its action disagrees.

| Given | Required decision |
| --- | --- |
| Reviewed child, dependencies merged, CI pending | Allow development integration; record CI pending |
| Every child requires a full cold Rust build with no specific reason | Revise to cheap child checks and batched manager verification |
| Every development merge waits for green CI | Remove the blanket wait; retain review and dependency gates |
| Branch merged, checks still pending | Report integrated, not verified |
| Red result without a named repair owner | Assign diagnosis/fixing ownership before moving on |
| Localized failure understood, next work independent, repair tracked | Continue independent work and retain the alert |
| Unknown/cross-cutting/foundation failure or attribution becoming unclear | Pause affected dependency chain; diagnose and repair |
| Dependent work based on an older base or overlapping generated files | Rebase/reconcile against the actual merged base and enforce ordering |
| Proposed weaker assertions, filters, CI, or rulesets to obtain green | Reject the weakening; repair the failure |
| Cancelled/skipped/deferred/superseded run or pass-on-retry | Preserve the actual state and original evidence; never label it passed |
| Older SHA green, newer integrated head unverified | Record the newer head's evidence separately; do not inherit green |
| Admin bypass or main push proposed as routine development | Reject; use approved development topology and respect protection/publication boundaries |
| Native Mac qualification is slow while independent topics are ready | Continue reviewed development; keep qualification on its selected immutable SHA |
| Build timeout before test execution | Record build incomplete / tests not run; no behavioral verdict |
| Release candidate frozen, unrelated new topic ready | Keep topic on development branch; only relevant reviewed fixes enter candidate |
| Four shards each compile cold | Measure aggregate cost; do not assume fourfold speedup |
| Non-draft PR targets base branch | Inspect preview triggers; use existing draft suppression where no preview is authorized |

These are text review fixtures, not an executable test suite. For text-only edits, check
formatting, reference targets, policy coverage, and the diff's scope. Existing CI and release
procedures remain authoritative; this reference does not change workflow execution.
