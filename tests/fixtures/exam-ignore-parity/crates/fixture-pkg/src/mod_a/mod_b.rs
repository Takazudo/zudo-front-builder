// Fixture for tests/unit/check-exam-ignore-parity.sh (issue #2072).
// Exercises COVERED-BY-HEALTH-SCOPE via a `--lib mod_a::mod_b::`
// module-prefix health.yml step scope — the lib-test counterpart to
// beta_integration.rs's integration-test `--test` case.

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "heavy: run with --ignored — fixture lib test"]
    fn lib_covered_test() {}
}
