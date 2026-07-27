// Fixture for tests/unit/check-exam-ignore-parity.sh (issue #2072).
// Exercises UNCOVERED: absent from the fixture exam.yml filterset AND from
// any fixture health.yml step whose *scope* actually reaches it. The
// fixture health.yml also carries a decoy step named to mention "delta"
// whose `run:` line scopes a different binary (beta_integration) — a loose
// name-similarity grep against step names would wrongly call this covered;
// a real scope parser must not be fooled.

#[test]
#[ignore = "env-gate: fixture-tool — deliberately absent from both lanes"]
fn uncovered_test() {}
