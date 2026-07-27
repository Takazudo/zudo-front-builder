// Fixture for tests/unit/check-exam-ignore-parity.sh (issue #2072).
// Exercises EXCEPTION(verification): a one-time proof, not a regression
// guard, exempt from both coverage lanes by tag alone — absent from both the
// fixture exam.yml filterset and any fixture health.yml step scope.

#[test]
#[ignore = "verification: one-time fixture helper, not a regression guard"]
fn verification_helper_test() {}
