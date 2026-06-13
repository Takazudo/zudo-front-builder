---
name: Flaky test quarantine
about: Track a quarantined flaky test. The inline marker next to the test links here.
title: "[flaky] <crate/file>::<test name>"
labels: flaky
---

<!--
This issue is the paper trail for ONE quarantined flaky test, per the
zudo-test-wisdom quarantine pipeline (see CLAUDE.md → Testing → Flaky tests).

Quarantine is a pipeline with an exit, not a parking lot. Before opening this,
confirm Step 0 below — a test that can pass NOWHERE is broken, not flaky.

The quarantined test must carry an inline marker pointing back at this issue, e.g.
    #[ignore = "flaky: https://github.com/Takazudo/zudo-front-builder/issues/<N>"]   // Rust
    // @flaky: <issue-url>                                                            // TS
-->

## Test

- Location: `<crate-or-package>/<path>::<test_name>`
- Marker added in: `<commit / PR>`

## Step 0 — has it ever genuinely passed?

> A test that has never produced a real green run is **broken, not flaky** — fix or delete it, do not quarantine.

- [ ] Confirmed at least one genuine green run (not pass-by-skip) on some host: `<run URL / local evidence>`

## Symptom

<How does it fail? How often? Local only, CI only, or both? Paste a failing run URL.>

## Suspected root cause

The five usual causes (fix the cause, do not add a sleep):

- [ ] Timing wait / fixed `sleep` (use an event/condition-keyed wait instead)
- [ ] Waiting on the wrong completion signal
- [ ] Animation / transition / async work in flight
- [ ] Shared or test-order-coupled state
- [ ] Startup / readiness race
- [ ] Other: <describe>

## Exit — fix / demote / delete (with a deadline)

> Quarantine suspends **product** coverage, not just test coverage: the behavior this test guards is unguarded until it is fixed.

- Owner: <who>
- Deadline: <date>
- Decision: <fix | demote to a lower level | delete and watch>
