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
