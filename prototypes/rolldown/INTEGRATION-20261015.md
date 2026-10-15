# Experimental integration review — 2026-10-15

The requested direction is an explicitly experimental opt-in for dogfooding,
with esbuild still the default. This pass prepares existing PR #4120 for the
owner's merge decision; it does not mark ready, merge, deploy or publish.
This Cloud session owns verification and CI triage under
`CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions`.

## Approved repair increment

The owner approved explicit unsupported raw CSS in the experimental backend and
a narrowly reviewed nonadditive producer-lock transition, without changing the
native pin, numerical ceilings or platform evidence gates. The prior rejected
runs below retain their original identities and conclusions.

- Raw CSS now fails with `ZFB_ROLLDOWN_RAW_CSS_UNSUPPORTED` based on the effective
  native loader, before publication. Explicit text/file CSS resources still
  work. Native negative tests assert no newly created output and preserve
  another owner's bundle/metafile; both original esbuild CSS assertions remain.
- A private prepared-path adapter delegates mapping selection, extension probes,
  package main/index, imports and side-effect metadata to the pinned resolver.
  Only selected trailing-directory mappings receive basename-file-before-main
  handling. It does not trim every slash or restore excluded live-tree files.
  Independent review reproduced a `rootDirs` escape in the native relative
  directory probe. That combination now fails explicitly before resolution or
  publication; a red-before-fix/green-after-fix native test retains the guard.
- WASM now calls the existing YAML parser through a typed Markdown-only result
  after the same filename gates. This removes redundant filename/TSX dispatch
  and impossible absent-body branches; it does not change parsing, diagnostics,
  exports, compiler settings or ceilings. Local Linux render gzip-9 moved from
  1,100,167 to 1,099,914 bytes with Rust 1.99.0, wasm-bindgen 0.2.121,
  Binaryen 130.0.0 and Node 24.14.0. CI remains the authoritative size gate.
- The [reviewed Cargo transition](../../research/v3-island-size/cargo-transitions/README.md)
  binds the exact old/new locks, every changed record and an approved manifest
  digest. Default additive proof and the live strict checker stay intact.
  Policy-test success is not platform budget admission. No held #4234 policy is
  activated and no package/dependency pin or lockfile changes in this repair.

Darwin's existing contract additionally records package 4.5.0 while this source
is 4.5.2. Its handoff first measures incorporated main `57f625a2` natively and
uses the existing release-identity transition in an isolated old-lock checkout,
then measures the final candidate and applies the reviewed Cargo transition.
Both stages retain unchanged-total/source/platform gates; no composed policy is
needed. No native Mac task has been started. This Cloud session remains the
verification/CI-triage owner.

### Repair candidate evidence

Candidate `61af5455bb34fdc78ddb89b3b7e0f0fb43338783` was pushed non-force and
read back. [Health run 38717438201, WASM job 116363531178](https://github.com/Takazudo/zudo-front-builder/actions/runs/38717438201/job/116363531178)
built all four artifacts under their unchanged ceilings, passed 238 npm tests
and export/resource assertions, then failed only the stale measured-size record
comparison. These CI values refresh the manifest and eight documentation files:

| Artifact | Final WASM | gzip-9 | Unchanged ceiling |
| --- | ---: | ---: | ---: |
| root | 3379537 | 1517312 | 1600000 |
| highlight | 1540963 | 821001 | 880000 |
| render | 2212451 | 1099769 | 1100000 |
| parse | 701755 | 285136 | 325000 |

The render result is 318 bytes below the prior rejected CI candidate and 231
below its ceiling. The existing graph upgrades remain; no dependency downgrade,
size allowance, compiler flag or assertion changed. Local Linux output differs
and is not copied into the CI-only manifest.

Local `bac4c3b3` integrated verification passed after genuine regeneration of all
173 Wind previews; only the compiler fingerprint changed. The replay passed
42 example + 11 library tests, all 36 original differential rows, all 67 prepared
contracts for both selected backends, 24 islands + one CLI cross-pipeline test
in each of three configurations, 394 client-library tests, and the real
browser/SSR/production/client-worker harnesses using Chromium 154.0.8037.57.
Rustfmt and native all-target Clippy passed. Cargo policy tests passed 164/164.
The default/render WASM native tests, 20 frontmatter tests and 238 npm tests
passed; the four-artifact local strict record comparison remained correctly red
because only CI values belong in that manifest.

The broad local root run passed 926 tests and failed two existing docs-supervisor
process-reaping tests; isolated retry also failed (three cases). The reported
descendants were observed as `Z` processes adopted by container PID 1. Assertions
and concurrent docs work were left intact; hosted exact-head Health is required.
The initial expanded candidate also rejected the stale Wind fingerprint; that
failure is retained, followed by the single-field regenerated manifest repair.

## Integrated source and scope

Merge `d5bff12f08cd55548df42f7fb8f45a7bdff9fbfa` has parents
`e6da2aba57b486738a1108ae4c211c57fa534499` (prototype) and
`57f625a2e3fc4bf69eb526c7bb8ef2103d4669d6` (actual main, v4.5.2).
All 11 intervening main commits were integrated. Generated-data conflicts used
main's newer CI-measured WASM records and corresponding docs as the baseline;
Wind previews were then regenerated with the combined-source workspace compiler.
No held prerequisite or direct-V8 #4242 branch was imported.

The source review covers optional Cargo edges, the native adapter, shared SSR /
islands / standalone-client dispatch, fixtures, workflows and derived records.
Only exact `ZFB_ROLLDOWN_PROTOTYPE=1` with the compile feature selects native.
Feature-absent and compiled-but-unselected execution retain esbuild; native
errors propagate without fallback. Plugin/config production callers stay on
esbuild. A reproduced error-publication defect changed the shared adapter to
in-memory generation, sequential normal-return close, warning/graph validation,
and publication after the synchronous scoped worker/runtime join. It still has
no supervisor, admission bound or request cancellation. Returned filesystem write
errors can leave partial output, as before; this is not a transactional publisher.
Its sole GraphPlugin has no close hook, so it does not require the standalone
probe's scan-error close observer. No new backend or native feature was added.

Independent review found the existing worker stage-escape ignored test missing
from the governance manifest. It is now registered; the measured inventory is
85 ignored tests (60 env-gate, 23 heavy, two verification). Existing health
integration-binary coverage already includes it; parity reports zero uncovered.
The whole-PR verifier preserves exact-SHA checkout, read-only permissions,
locked/frozen dependencies, assertions and tracked cleanliness. The explicit
installed-Chromium override changes only test launch and prints browser version.

## Cargo resolution evidence

| Input | Packages | SHA-256 |
| --- | ---: | --- |
| Main Cargo.lock | 601 | `2726cdb828d0f3c6cea16f1d5f00d03625d21dfe9e0995d5142ef0246157678e` |
| Prototype Cargo.lock | 696 | `13956cf2b75baea8b68442c750174714b9591d6ab7ba9f923a800c5653e1ac37` |

Main to prototype adds 111 identities, removes 16 and changes 46 surviving
records. Fourteen removed records are version replacements; `papaya` and `seize`
are replaced by the resolver's new dependency structure.

| Shared package | Main | Prototype | Required native minimum |
| --- | --- | --- | --- |
| bitflags | 2.11.1 | 2.13.2 | 2.13.0 |
| fast-glob | 1.0.1 | 1.1.2 | 1.1.0 |
| json-escape-simd | 3.1.0 | 3.1.2 | 3.1.1 |
| memchr | 2.8.0 | 2.8.3 | 2.8.2 |
| oxc_resolver | 11.19.1 | 11.24.3 | 11.24.3 |
| regex | 1.12.3 | 1.13.1 | 1.13.0 |
| regex-automata | 0.4.14 | 0.4.18 | 0.4.16 via new regex |
| regex-syntax | 0.8.10 | 0.8.11 | 0.8.11 via new regex |
| rustc-hash | 2.1.2 | 2.1.3 | 2.1.3 |
| serde_json | 1.0.149 | 1.0.151 | 1.0.150 |
| simd-json | 0.17.0 | 0.18.1 | 0.18.0 via resolver |
| smallvec | 1.15.1 | 1.16.3 | 1.16.0 |
| unicode-segmentation | 1.13.2 | 1.13.3 | 1.13.3 via OXC diagnostics |
| uuid | 1.23.1 | 1.28.0 | 1.27.0 |

The minima were checked against Rolldown commit
`24bc2d0b5c9a8c87ac8d1ce8cb9a2df61b8624b9` and resolved dependency manifests.
Starting from actual main's lock with combined manifests, Cargo metadata fails
on rustc-hash 2.1.2 versus >=2.1.3. Unlocking only that package fails on
oxc_resolver 11.19.1 versus >=11.24.3. Unlocking the 11 directly required shared
packages resolves successfully. Preserving existing prototype serde_json 1.0.151
and syn 3.0.7 rather than taking two fresh upgrades reproduces the prior lock
**byte for byte**. No lock was hand-assembled and no dependency/source pin changed.

Nine Windows dependency-edge reselections also reproduce from the main-seeded
resolution. `anstyle-query`, `anstyle-wincon` and `socket2` select windows-sys
0.60.2; `errno`, `is-terminal`, `rustix`, `tempfile` and `winapi-util` select
0.52.0; `nu-ansi-term` selects 0.59.0 instead of main's 0.61.2. Their manifests
allow 0.61.2, so these are compatible solver consequences, not native minimum
requirements. Cargo offers no narrow per-parent edge update. They are recorded
Windows verification debt; Linux results do not prove that platform.

Default-V8 and no-V8 graphs contain no Rolldown when the feature is absent, but
they share the newer resolver, JSON, CSS and SWC dependencies. Default V8 also
shares newer smallvec/bitflags; deno_core and V8 versions are unchanged. The
verifier retains actual locked normal/build/feature graphs for all three modes.
Thus an optional backend does not imply an unchanged normal producer closure.

## Historical provenance blocker (before owner approval)

The active `cargo-producer-lock` transition accepts additions only when all old
package records remain byte-identical. Changed versions, dependency features,
removed records and even dependency-name disambiguation violate it. This delta
is not admissible under that reason, the pnpm-only #4218 direction, release-bump
or runtime-identity. Held draft #4234 remains untouched.

A narrowly reviewed nonadditive Cargo transition decision needs this exact delta
and minima, final feature graphs, ordinary-producer checks, qualifying Linux and
native Darwin measurements, and unchanged numerical ceilings. The parent must
authorize that prerequisite separately; this pass neither activates a held
policy nor changes a live contract digest. Historical platform artifacts do not
qualify the new source.

For separately authorized native collection, the existing command is:

```sh
gh workflow run island-size-darwin.yml --ref prototype/318-rolldown-native \
  -f ref=<reviewed-final-SHA> -f platform=both -f enforce=false
```

This is an outstanding parent-coordination action, **not dispatched here**.
Require native macos-15 arm64 and Linux x64 artifacts, the pinned toolchain,
eight cases in each workspace/packed mode, two complete saved dist passes,
normalization and unchanged strict checker results. `enforce=false` collection
success would not itself mean budget/provenance acceptance.

## Historical CI-authoritative WASM regression

[Health run 38705578695, job 116324103686](https://github.com/Takazudo/zudo-front-builder/actions/runs/38705578695/job/116324103686)
on candidate `51649ccaba6e86cc499783062084d78e9f124f8b` built all four outputs:

| Artifact | Final WASM bytes | gzip-9 bytes | Unchanged gzip ceiling |
| --- | ---: | ---: | ---: |
| root | 3398069 | 1526466 | 1600000 |
| highlight | 1540963 | 821001 | 880000 |
| render | 2213332 | **1100087** | **1100000** |
| parse | 703502 | 285765 | 325000 |

Render is 2,989 gzip bytes above main's measured 1,097,098, and 87 above its
ceiling. The build correctly failed; downstream npm/export/package checks were
skipped. These are measured rejected outputs, not new accepted shipped records.
Main's shipped manifest/doc baseline is retained rather than labeling the
out-of-budget result accepted or increasing a ceiling.

Independent graph diagnosis found six upgraded runtime packages in render WASM:
serde_json, memchr, smallvec, rustc-hash, regex-automata and regex-syntax. It has no
Rolldown/OXC/SWC/V8/Tokio, no serde_json preserve_order, and flate2 still uses
miniz_oxide. Render/compiler source, WASM build configuration and root profiles
match main. No supported native-only feature removal fixes the cause. A measured
code-size/dependency remediation with the unchanged ceiling is required; no
speculative patch downgrade, Unicode/performance-table removal, allow-over-ceiling
flag or hand-edited measurement is used to manufacture green.

## Verification ledger

The reproducible command is `bash prototypes/rolldown/verify-integration.sh`
through the configured heavy guard, after package/tool preparation. It covers
feature absent; compiled feature with environment unset; explicit native opt-in;
no-V8 compilation; Wind library/preview/freshness; prepared/browser/SSR/production
and standalone-client real Chromium/dev-edit/negative harnesses. The existing
42 native plugin tests, four library tests and 36 semantic rows remain in the
same read-only exact-SHA workflow. This is a broader experimental check, not a
replacement for required health or provenance acceptance.

All 173 Wind previews were genuinely regenerated. Only the compiler fingerprint
changed; rendered CSS/assets are unchanged. The reviewed Wind source closure
still matches its existing digest; no support pin/catalog assertion changed.
Local browser download received a domain-forbidden response even on the exact
escalated retry. Tests use installed Chromium 154.0.8037.57 explicitly; CI uses
Playwright 1.58.2's Chromium. Local Node was initially 24.19.0; final toolchain
replay uses the installed pinned Node 24.14.0. Rust is 1.99.0, pnpm 12.8.2, Linux
x86_64. Distinct environments are not labeled equivalent.

Initial health workspace tests reported 909 root tests passed and two failures,
both the unchanged Linux/Darwin Cargo provenance assertions. Required broad
Rust steps after that failed prerequisite were skipped, not passed. Native and
normal-path measurement evidence is collected independently. Final exact-head
results and remaining failures are recorded in the PR description.

Expanded candidate `415c702f05396cbd3fda996382f32b1265c0f5f6` reproduced
two native adapter defects in both local execution and
[exact-source CI run 38706267179](https://github.com/Takazudo/zudo-front-builder/actions/runs/38706267179):
terminal `?url` resources failed loading, and a package-selected `jsconfig.json`
change updated a worker URL without changing its emitted class-field semantics.
The unchanged suite passed 22 of 24 tests under explicit native selection;
all 24 passed with the feature absent and with it compiled but unselected.
The native loader now reads the underlying terminal-`?url` file while retaining
the full resolved provenance ID. Native hashes use the pinned alphanumeric
alphabet so the resource name/hash delimiter stays unambiguous. Resource
admission rules and their negative assertions remain unchanged.
The alphabet is a bundler-wide native option: other hashed native chunks/assets
can change filenames too; the esbuild path is unaffected.

Native islands/client jobs now materialize the selected per-entry config with
nearest `tsconfig.json`/`jsconfig.json` discovery, package-declared inherited
configs and ordered recursive inheritance. The temporary config stays owned
through native runtime teardown. The ordinary esbuild branch is unchanged.
Project references and `${configDir}` templates fail explicitly in this bounded
normalizer; they are not claimed as supported. Independent review checks
inheritance precedence and config-relative paths, including diamond inheritance.
Review found and corrected premature inherited-path absolutization and lost
`--preserve-symlinks` config anchoring before integration. Six focused tests
cover those cases, package-config reload, explicit selection/search boundaries,
diamond precedence and invalid/cyclic/excessive inheritance. The repair pass
passed all 24 original real-native integration tests and 394 client-library
tests, including the existing resource-oracle rejection cases.

The ordinary Linux size collection completed all eight fixtures in both
workspace and packed modes, including two complete saved-dist passes and
normalization. The strict checker rejected the local/workspace/packed Cargo
lock identities; it reported no numerical ceiling violation. Collection is
evidence, not accepted provenance, and does not replace native Darwin evidence.

### Additional prepared-contract gates

The first clean replay at `ee33548b` reached the wider prepared-bundle suite and
found nine failures that earlier selected suites had not exercised. The excluded
alias test proved a real defect: native `write()` emitted disallowed fallback
bytes before warning validation returned an error. Generation and validation now
precede publication after actual join; that original assertion passes. A separate
paths/plugin-alias fixture used its Data import only in an unused local array;
native tree shaking legitimately removed it. Data now reaches the observable JSX
call, retaining the original assertion and making the oracle independent of that
optimization.

At `adb507f7`, seven assertions remained compatibility gates, not skipped/passed checks:

- Six directory-target tests expose absolute tsconfig paths ending `/` in pinned
  `oxc_resolver` 11.24.3. `tsconfig.rs:437–447` and `path.rs:110–111` retain the
  trailing slash; `cache/cache_impl.rs:165–171` compares raw parent path spelling
  and loses package-main lookup. `tsconfig_resolver.rs:333–338` resolves `.` and
  does not reproduce esbuild's directory/basename-child precedence. The explicit
  main-fields option is preserved; the neutral-platform hint is not the cause.
  Minimal native fixtures reproduce the failure with a main entry and pass only
  after an index entry is supplied. Trimming the slash would permit an excluded
  outside sibling and violate the existing assertions. Correct ordered contextual
  resolution is a separate targeted compatibility repair, not a default change.
- The CSS-context test reaches CSS bundling explicitly rejected by Rolldown at
  `24bc2d0`. Adding a CSS backend/plugin or changing the native pin is outside this
  increment. Existing CSS-module behavior and browser/production CSS preparation
  are distinct from this raw-CSS graph test.

The verifier runs every prepared test binary with `--no-fail-fast`, continues
independent checks, and returns failure if any prepared suite failed. No assertion
is removed, caught as success, ignored or relabeled optional.

## Automatic merge effects and decision boundary

The main-relative diff includes `docs/scripts/wind-compatibility-pages.mjs` and
the regenerated preview manifest. A merge therefore matches `docs-deploy.yml`'s
main-push paths: successful build runs Wrangler production deployment, Cloudflare
domain configuration and IFTTT notification. The parent must include those
external effects in its final merge authorization. No workflow permission or
secret change, manual deployment or release dispatch is made here. Package
publication requires the separate release/manual workflow.

**The historical rejected candidates below were not ready to merge. Current
acceptance still requires exact-source CI and native producer provenance.** Experimental opt-in remains the intended direction;
these are integration gates, not a proposal to remove the experiment. Hard
native termination, production plugin lifecycle, optional-native platform claims
#4123, packaging/performance #4124 and broader #318 adoption remain separate.
PR #4120 stays open/draft; #318/#4098/#4123/#4124/#4133 stay open and roadmap
#3328 -> #3329 -> #4099 and supervisor #3230 remain unchanged.
