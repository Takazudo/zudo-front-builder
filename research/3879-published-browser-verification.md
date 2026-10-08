# #3879 — published browser verification

Implemented an opt-in installed-registry harness in
[`tests/published-browser-verification`](../tests/published-browser-verification/README.md).
The integration manager completed the serialized installed-package browser run
on `e7349f3928f2f9e9b163281b976ed1f2b7ad6983`; all named candidate/control
checks passed. Independent review and CI remain separate evidence boundaries under
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
results for the named verification boundary. The completed browser run is recorded below;
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
It remains failed. The corrected commit received the fresh run below; exact-head
CI evidence remains pending. No old artifact is rewritten or retroactively labeled passed.

The corrected full manager run began at `2026-10-08T07:42:37.987Z` on Linux
`6.18.44` x64, Node `v24.19.0`, Chromium `151.0.7922.173` at `/usr/bin/chromium`.
The heavy guard reported `PASS`, exit 0, 11 seconds. All four isolated installs
verified frozen registry resolutions/integrity and installed versions; installed
native binaries reported their exact versions and embedded esbuild `0.25.12`.

| Check | Measured result |
| --- | --- |
| 4.1.0 late root Show, mount and hydrate | Branch rendered before unmount; 0 child nodes afterward; disposed true |
| 3.2.0 late root Show, mount and hydrate | Exactly 1 leaked child, `<p>later</p>`; disposed true; expected old defect reproduced |
| Intrinsic-wrapper controls, both methods and versions | 0 remaining child nodes; disposed true |
| 4.1.0 exact Fragment recipe | Production build exit 0; Counter and NamedCounter registered and mounted; both clicked 0 to 1 |
| 4.1.0 non-Fragment map | Build exit 1 with exact opaque-container boundary escape diagnostic |
| 4.0.0 verbatim bare-import recipe | Build exit 1 with both missing-target/skipped-bundle and exact Counter identity diagnostics |
| Browser/runtime diagnostics | Empty for all successful browser cases and all lifecycle roots |

The durable compact capture is
[`measured-result.json`](../tests/published-browser-verification/measured-result.json).
It includes tested SHA, environment, all 11 subprocess outcomes, lifecycle
measurements, registration/click results, native binary and lockfile SHA-256,
release-doc source pins, pin-manifest hash, and original-failure disposition.
Full retained evidence:

- `/workspace/scratch/zfb-sweep-261008/published-confirmed/evidence.json` — SHA-256 `7a76dcdf46a38867503f802d36a4742f9c6fa31d8efb8c2d0dbcc3c78faaad4f`
- `/workspace/scratch/zfb-sweep-261008/published-confirmed.log` — SHA-256 `72bf9c16cafbda6b9bc6a0d2069711c1a3fe7a9264c0f71b2e1aa5f8c052ee42`

This proves the scoped Linux Chromium installed-package contracts at the tested
harness commit. The subsequent report-only commit does not change executable
harness code. Independent review, exact-head required CI and current-base
revalidation remain manager-owned. No downstream consumer migration, other
browser/platform, visual appearance or cryptographic attestation verification is
claimed. No further browser run was performed for this report update.
