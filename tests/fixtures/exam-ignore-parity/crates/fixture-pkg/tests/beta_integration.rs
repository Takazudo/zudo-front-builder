// Fixture for tests/unit/check-exam-ignore-parity.sh (issue #2072).
// Exercises COVERED-BY-HEALTH-SCOPE via an explicit `--test beta_integration`
// health.yml step scope. Deliberately absent from the fixture exam.yml
// filterset.

#[test]
#[ignore = "heavy: run with --ignored — fixture"]
fn covered_by_health_test() {}
