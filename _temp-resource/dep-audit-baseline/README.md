# Dependency-audit baseline

These artifacts capture the pre-edit state at commit
`50445b02f47c2b46f7b50a87e07afd621a6ad2ea`. Later dependency-audit work can
compare its dependency graph, generated asset names, and md-wasm size report
against these files.

- `baseline-sha.txt` — output of `git rev-parse HEAD`.
- `baseline-ci.txt` — output of `gh run list --branch main --limit 10 --json workflowName,conclusion,headSha`, followed by the per-workflow interpretation. The three runs for the baseline SHA are green; no already-red check was found for that SHA.
- `baseline-package-count.txt` — output of `grep -c '^\[\[package\]\]' Cargo.lock` (603 packages).
- `baseline-duplicates.txt` — workspace duplicate dependency tree from `cargo tree --workspace --duplicates`.
- `baseline-features.txt` — workspace feature tree from `cargo tree -e features --workspace`.
- `baseline-features-nov8.txt` — no-default-features `zfb` feature tree from `cargo tree -e features -p zfb --no-default-features`.
- `baseline-hashes.txt` — CSS/JS filenames emitted by the docs build (`pnpm --filter docs build`, then `ls dist/**/*.{css,js}` from `docs/`); hashed names are the asset-content oracle.
- `baseline-wasm-size.txt` — the four final gzip-9 sizes reported by the passing `pnpm test:md-wasm` build/test lane (12 test files, 215 tests).

No production source, manifest, lockfile, workflow, or documentation file was
changed for this baseline. The local standalone budget checker reports a
pre-existing manifest mismatch under Rustup stable 1.96; the passing lane's
size report is retained verbatim for later same-environment comparisons.
