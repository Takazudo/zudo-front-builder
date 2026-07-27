// Fixture for tests/unit/check-exam-ignore-parity.sh (issue #2072).
// Exercises the COVERED-BY-EXAM outcome: this test's exact bare name appears
// in the fixture exam.yml's quarantine-heavy filterset.

#[test]
#[ignore = "env-gate: fixture-tool — exercised only by the offline parity guard test"]
fn covered_by_exam_test() {}
