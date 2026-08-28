# Chromium Back-race findings

## Verdict A: the harness cannot guarantee the sampled window

The evidence supports **(A), a harness race**, not a missed product abort boundary. `waitForURL()`
observes the early same-document history update from outside the page, but it does not guarantee that
Chromium's native View Transition still has the old DOM when the test resumes.

The decisive comparison is the router's `popstate` entry against `domCommitStarted`, not the time at
which Playwright requested Back:

- All 19 passing runs reached `router-popstate-entry` before the forward transition's native callback
  could start the DOM commit. `abortAndRecreateMostRecentNavigation` emitted
  `zfb:navigation-aborted` in all 19.
- All seven abort-timeout failures reached `router-popstate-entry` after the forward transition set
  `domCommitStarted = true`. The same abort call site ran in all seven and correctly recorded
  `early-return:domCommitStarted`.
- Four of those seven failures are especially strong counterexamples to using the command timestamp:
  Playwright requested Back 0.5-2.8 ms before `domCommitStarted`, but Chromium delivered the router's
  `popstate` 6.1-10.6 ms after it. The command was early; the traversal was not.
- Separately, four old-body failures completed Page B's swap before the test's assertion could
  observe the window. The assertion saw `Page B` throughout its five-second retry period, so those
  runs never issued Back.

No run reached the router before `domCommitStarted` and then omitted the abort event. That is the
counter-evidence required for verdict (B), and it did not occur.

## One bounded round reproduced both failure signatures

The manager ran the issue's single-spec command under the machine-wide Playwright guard, with zero
retries and `--repeat-each=30`. The result was 19 passes and 11 failures in 2.1 minutes:

| Outcome | Runs | What happened |
| --- | ---: | --- |
| Pass | 19 | Router entry won the boundary; abort emitted; URL and DOM remained on A |
| Abort-event timeout | 7 | Forward DOM commit won; Back later returned to A; abort correctly suppressed |
| Old-body assertion failure | 4 | Forward swap finished before the harness sampled it; heading stayed `Page B` |

The complete per-repeat JSON evidence and command log are in
`20260828-issue-2669/browser-evidence/round-1` in the repository-scoped cclogs directory. Each JSON
file contains the full `window.__zfb.events` sequence plus one page-side `performance.now()` timeline.

## The measured pre-commit window is only a few milliseconds wide

All values below are page-side milliseconds. “Callback” is native `startViewTransition` callback
entry. Passing runs have no forward `domCommitStarted`: the real Back traversal aborted them first.

| Measurement | n | Min | Median | p95 | Max |
| --- | ---: | ---: | ---: | ---: | ---: |
| Completed forward swap: early history commit → `domCommitStarted` | 11 | 2.4 | 14.5 | 23.9 | 23.9 |
| Pass: early history commit → native callback | 19 | 13.0 | 16.6 | 25.6 | 25.6 |
| Pass: early history commit → Playwright Back request | 19 | 3.7 | 6.9 | 9.6 | 9.6 |
| Pass: Back request → router `popstate` entry | 19 | 1.5 | 3.0 | 14.6 | 14.6 |
| Abort timeout: early history commit → `domCommitStarted` | 7 | 7.5 | 14.5 | 23.9 | 23.9 |
| Abort timeout: Back request → router `popstate` entry | 7 | 4.3 | 8.9 | 20.9 | 20.9 |
| Abort timeout: `domCommitStarted` → router `popstate` entry | 7 | 6.1 | 10.6 | 69.5 | 69.5 |
| Old-body failure: early history commit → `domCommitStarted` | 4 | 2.4 | 9.5 | 19.1 | 19.1 |

The pass/fail split is governed by two browser-scheduled hops: when Chromium invokes the native
update callback and when it delivers the real history traversal. An out-of-process URL notification
cannot order those page-side events.

## Raw pass timeline: run 03 aborts before the commit boundary

Times are `performance.now()` values from `back-race-03-passed.json`.

| Seq | Time | Marker | URL | Heading | Decision |
| ---: | ---: | --- | --- | --- | --- |
| 1 | 102.3 | `event:zfb:page-load` | A | Page A | |
| 2 | 193.1 | `event:zfb:before-preparation` | A | Page A | forward / push |
| 3 | 225.9 | `event:zfb:after-preparation` | A | Page A | |
| 4 | 226.1 | `early-history-commit` | B | Page A | |
| 5 | 232.0 | `playwright-go-back-dispatch` | B | Page A | |
| 6 | 237.3 | `browser-popstate-dispatch` | A | Page A | state index 0 |
| 7 | 237.3 | `router-popstate-entry` | A | Page A | state index 0 |
| 8 | 238.4 | `notify-navigation-aborted` | A | Page A | abort/recreate: **emit** |
| 9 | 238.4 | `event:zfb:navigation-aborted` | A | Page A | |
| 10 | 251.7 | `native-view-transition-callback-entry` | A | Page A | aborted forward callback |
| 11 | 251.9 | `notify-navigation-aborted` | A | Page A | finish update: already emitted |

Full lifecycle sequence: `page-load → before-preparation → after-preparation → navigation-aborted`.
This run exercises both required notify call sites: the synchronous abort/recreate path emits once,
then `finishAbortedUpdate` correctly avoids a duplicate event.

## Raw timeout timeline: run 09 proves command time is not traversal time

Times are from `back-race-09-failed.json`. Playwright requested Back before the boundary, but the
router received it afterward.

| Seq | Time | Marker | URL | Heading | Decision |
| ---: | ---: | --- | --- | --- | --- |
| 1 | 36.2 | `event:zfb:page-load` | A | Page A | |
| 2 | 123.0 | `event:zfb:before-preparation` | A | Page A | forward / push |
| 3 | 134.8 | `event:zfb:after-preparation` | A | Page A | |
| 4 | 135.1 | `early-history-commit` | B | Page A | |
| 5 | 149.1 | `playwright-go-back-dispatch` | B | Page A | 0.5 ms before boundary |
| 6 | 149.4 | `native-view-transition-callback-entry` | B | Page A | |
| 7 | 149.5 | `event:zfb:before-swap` | B | Page A | forward / push |
| 8 | 149.6 | `dom-commit-started` | B | Page A | point of no return |
| 9 | 153.9 | `event:zfb:after-swap` | B | Page B | |
| 10 | 155.4 | `event:zfb:page-load` | B | Page B | |
| 11 | 155.7 | `browser-popstate-dispatch` | A | Page B | state index 0 |
| 12 | 155.8 | `router-popstate-entry` | A | Page B | 6.2 ms after boundary |
| 13 | 156.0 | `notify-navigation-aborted` | A | Page B | abort/recreate: **early return, DOM committed** |
| 14 | 156.0 | `event:zfb:before-preparation` | A | Page B | back / traverse |
| 15 | 160.6 | `event:zfb:after-preparation` | A | Page B | |
| 16 | 185.6 | `native-view-transition-callback-entry` | A | Page B | Back transition |
| 17 | 185.7 | `event:zfb:before-swap` | A | Page B | back / traverse |
| 18 | 185.7 | `dom-commit-started` | A | Page B | Back transition |
| 19 | 185.9 | `event:zfb:after-swap` | A | Page A | |
| 20 | 186.5 | `event:zfb:page-load` | A | Page A | |

Full lifecycle sequence: `page-load → before-preparation → after-preparation → before-swap →
after-swap → page-load → before-preparation → after-preparation → before-swap → after-swap →
page-load`. The product completed the forward commit, then handled the real Back traversal and ended
on A. The test timed out only because it expected the now-inapplicable forward-abort event.

## Raw old-body failure: run 01 misses the window entirely

Times are from `back-race-01-failed.json`.

| Seq | Time | Marker | URL | Heading |
| ---: | ---: | --- | --- | --- |
| 1 | 133.9 | `event:zfb:page-load` | A | Page A |
| 2 | 230.2 | `event:zfb:before-preparation` | A | Page A |
| 3 | 360.0 | `event:zfb:after-preparation` | A | Page A |
| 4 | 360.3 | `early-history-commit` | B | Page A |
| 5 | 369.5 | `native-view-transition-callback-entry` | B | Page A |
| 6 | 369.8 | `event:zfb:before-swap` | B | Page A |
| 7 | 369.8 | `dom-commit-started` | B | Page A |
| 8 | 374.4 | `event:zfb:after-swap` | B | Page B |
| 9 | 381.2 | `event:zfb:page-load` | B | Page B |

Full lifecycle sequence: `page-load → before-preparation → after-preparation → before-swap →
after-swap → page-load`. The test never recorded a Back request because its preceding old-body
assertion observed Page B for the full retry window.

## Ranked remediation: gate the native callback in the test

| Rank | Candidate | Cost | Level-4 coverage preserved | Level-4 coverage lost | Decision |
| ---: | --- | --- | --- | --- | --- |
| 1 | Test-only native callback gate | Medium harness complexity | Real Chromium, native VT call-through, early history commit, real Back, abort boundary | No material behavior; adds a test scheduling seam | Recommend |
| 2 | Weaker final-state invariant | Low | Real Chromium, native VT, real Back, final URL/DOM | Early supersession and abort boundary | Use only if split-level coverage is accepted |
| 3 | Existing router-event trigger | Low as written; medium-high if a new product event is added | Real browser/history mechanics | Deterministic ordering with existing events; a new event contaminates the product surface | Rule out |
| 4 | Product boundary change | High semantic risk | Real browser mechanics | Correct point-of-no-return and abort semantics | Rule out under verdict A |

1. **Use a condition-keyed, test-only call-through wrapper around
   `document.startViewTransition` — recommended.** Install it with `addInitScript`, retain the bound
   native function, and hold only Page B's update callback after Chromium enters it. Once the gate is
   entered, assert URL B/body A, start a real `page.goBack()` without immediately awaiting it, and
   release the held callback from a page-side listener on the real `popstate`. This has moderate
   harness complexity but preserves Level-4 coverage of real Chromium, the native View Transitions
   API, the real router, the early history entry, and a genuine Back traversal. It removes both CDP
   scheduling races without a sleep or product hook. The load-bearing deadlock guard is that release
   must happen page-side on `popstate` (or Back must remain unawaited); awaiting `page.goBack()` before
   release can wait forever.
2. **Assert a weaker deterministic invariant — viable only if exact race coverage is intentionally
   dropped.** Let Page B finish, issue real Back, and assert final URL/body A plus the normal traverse
   lifecycle. This is low cost and keeps real Chromium/native VT/history coverage, but it no longer
   proves that Back supersedes a forward navigation in the early-commit/pre-swap window. Existing
   Level-2 coverage still proves the abort branch, so this is defensible only if Wave 2 accepts split
   coverage rather than a single Level-4 regression proof.
3. **Re-key Back onto an existing router event — rule out as a standalone fix.**
   `zfb:after-preparation` fires before the early history entry exists; `zfb:before-swap` is followed
   by only a microtask before the synchronous commit section, while real `popstate` delivery takes
   1.5-20.9 ms in this sample. An out-of-process response to either signal still cannot order the real
   traversal before `domCommitStarted`. Adding a new product event after `safePushState`, or calling
   Back synchronously from product lifecycle code, costs more and contaminates the behavior under
   test. The recommended wrapper supplies the missing condition entirely in the test.
4. **Change the product abort boundary — rule out under verdict A.** The router emitted in every run
   where traversal arrived before commit and suppressed the event in every run where commit had
   started. Moving the boundary later would allow a committed DOM transition to be mislabeled as
   aborted and would change correct runtime semantics to accommodate an out-of-process harness race.
   A product change becomes appropriate only if future evidence finds router entry before
   `domCommitStarted` with no abort emission.

## Instrumentation was evidence-only and is not retained

The round used an environment-keyed fixture-server transform of the built router plus a page-side
recorder. It changed neither `packages/zfb-runtime/**` nor the spec's assertions. After capturing all
30 timelines, the transform, timestamp recorder, and JSON writer were removed; this findings file is
the durable deliverable.

## Locked remediation decision

Implement the ranked test-only native callback gate in
`tests/router-chromium/back-race.chromium.spec.mjs`. The `HARNESS_INIT` call-through wrapper must
retain the bound native `document.startViewTransition`, hold only the Page B update callback after
Chromium enters it, and expose that condition to the test. A page-side listener for the real
`popstate` back to Page A must release the callback; the test must start `page.goBack()` without
immediately awaiting it and await the returned promise only after the abort signal, avoiding the
call-through deadlock while preserving real Chromium, native View Transitions, and history traversal.

The implementation burn-in is exactly 50 zero-retry repetitions:
`pnpm test:router-chromium -- back-race.chromium.spec.mjs --repeat-each=50 --retries=0`.
Reject the weaker final-state invariant because it loses Level-4 early-supersession coverage; reject
existing router-event triggers because neither orders real traversal before `domCommitStarted` and a
new event would contaminate the product API; reject a product boundary change because Verdict A
shows the current boundary is correct. No runtime or Level-2 change is warranted.
