// Fixture for tests/unit/check-exam-ignore-parity.sh (issue #2690).
// Exercises COVERED-BY-HEALTH-SCOPE via an exact test predicate nested under
// a package conjunction in a workspace nextest ignored-only filterset.

#[test]
#[ignore = "env-gate: fixture-tool — covered by nextest test predicate"]
fn covered_by_nextest_exact_test() {}
