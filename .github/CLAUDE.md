# .github/ — workflow context pointer

Before editing test wiring in `workflows/exam.yml` or `workflows/health.yml` (env-gate steps, `--ignored` scopes, exam filtersets), read `crates/CLAUDE.md` — the authoritative `#[ignore]` manifest, taxonomy, and maintenance rules. `scripts/check-exam-ignore-parity.sh` mechanically verifies workflow-scope parity. The T1 gate, branch ruleset, and tier strategy live in the root `CLAUDE.md`.
