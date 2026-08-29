// Fixture for tests/unit/check-exam-ignore-parity.sh (issue #2690).
// Exercises COVERED-BY-HEALTH-SCOPE via an exact package/binary predicate in
// a workspace nextest ignored-only filterset.

#[test]
#[ignore = "env-gate: fixture-tool — covered by nextest binary predicate"]
fn covered_by_nextest_binary_test() {}
