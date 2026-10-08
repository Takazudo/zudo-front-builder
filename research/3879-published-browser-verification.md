# #3879 — published browser verification

Implemented an opt-in installed-registry harness in
[`tests/published-browser-verification`](../tests/published-browser-verification/README.md).
This is preparation, not a claim that browser verification passed. The integration
manager owns the serialized heavy-guard run and exact-commit evidence under
`CLAUDE.md` development strategy. Parent: captured main
`cb4b6dbd3af21980b302916c5d97d4f767c51574`; this unit has no sibling dependency.
No release, deploy, migration, admin bypass or gate change is authorized here.

The candidate is frozen at 4.1.0; known-broken controls are 3.2.0 for late root
Show unmount (#3814), and 4.0.0 for the bare SDK alias (#3917 / #3824).
SDK, router runtime and Linux x64 glibc carrier registry resolutions, SHA-512
integrity and provenance metadata are checked in. Actual installations and native
binary identity are recorded at execution. Unlike `sdk-alias-consumer/run.mjs`,
this harness invokes the registry carrier rather than a current-source binary.

The MDX recipe comes from each matching release Git tag, with source hashes and
commits captured. Both snapshots contain the Fragment fixed-target recipe with
bare `zfb` imports. The harness does not rewrite those imports or install a bare
npm alias to conceal the historical failure. Candidate acceptance includes
production build, registration, real hydration, both 0-to-1 clicks and clean
browser diagnostics. A single-variable Fragment-removal rejection check guards
the boundary escape restriction. The 3.2.0 scoped Fragment form historically
passed and is not asserted broken.

Cheap child checks: JavaScript syntax for both modules; registry metadata and
release snapshot extraction; source-level review of provenance and assertions.
Manager run: follow the README commands, retain `/tmp/zfb-published-browser-261008`
(or another fresh output directory), and attach `evidence.json` plus command logs
and lockfiles. Record tested commit, OS, browser and all named controls' actual
results before calling #3879 verified. Browser installation/build/execution,
required CI and current-base revalidation remain manager-owned and pending.

Manager first run on `75e51f810214d3c850bd2280b08e747dea5c2067` used Linux x64
and Chromium `151.0.7922.173` at `/usr/bin/chromium`. Candidate late-Show mount
and hydrate, both intrinsic-wrapper controls, 3.2.0 leak reproduction, candidate
Fragment recipe hydration/clicks, and non-Fragment rejection passed. The overall
run **failed** because the final negative assertion incorrectly expected an
unresolved `zfb/zudo-react` import. Its actual 4.0.0 output reproduced the
verbatim-import defect: no scanner targets, skipped islands bundle, then
`ZR_ISLAND_IDENTITY: Counter is not registered by the scanner`.

[#3917's variant A](https://github.com/Takazudo/zudo-front-builder/issues/3917)
records exactly that failure. Its variant C changes only the Island import and
then exposes the unresolved client import. The
[corrected diagnosis](https://github.com/Takazudo/zudo-front-builder/issues/3917#issuecomment-6022639657)
confirms scanner and client alias parity were separate gaps. The `v4.0.0` scanner
source (`crates/zfb-islands/src/scanner/registration.rs`, `sdk_export` and
`is_sdk_namespace`) recognizes scoped specifiers only. The corrected assertion
requires both missing-target/skipped-bundle evidence and the exact Counter
identity diagnostic, while preserving all recipe imports and candidate checks.

The first failed run is retained at
`/workspace/scratch/zfb-sweep-261008/published.log` and
`/workspace/scratch/zfb-sweep-261008/published-evidence/evidence.json`.
It remains failed; the corrected commit needs a fresh manager run and exact-head
CI evidence. No old artifact is rewritten or retroactively labeled passed.
