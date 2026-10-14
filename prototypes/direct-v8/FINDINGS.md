# Phase 1 execution findings

The standalone Linux runtime feasibility gate passes. Delivery remains blocked
by the existing island-size Cargo-lock provenance check; this is not a claim
that all required CI passes or that candidate CLI integration is complete.
The sole delivery PR is [draft #4242](https://github.com/Takazudo/zudo-front-builder/pull/4242).
All six delivery issues remain open. No default/runtime selector, release,
package version, deployment or preview upload is part of this experiment.

## Environment and checkpoints

- Refreshed base: `6fb3fbe0103c5c90799c3262ca5c1ab9ade0f27e`.
- Reviewed extraction: `4a1bc103a8af99f0a0fde8f58fff43f27a333657`, pushed on
  `prototype/deno-core-exit`. Includes the narrowly scoped showcase draft guard.
- Linux x86_64 managed environment, Node `v24.19.0`, pnpm `12.8.2`, Rust
  `1.99.0 (b940084d7 2026-09-28)`. This is native Linux evidence only.
- Exact engine: `v8 147.4.0`; baseline `deno_core 0.399.0`. API source inspected
  at rusty_v8 tag `v147.4.0`, commit `348006707529fa4559d931169ad985b3ce518460`.
- Shared target `target/`; Cargo/Rustup homes `/workspace/zfb-cargo` and
  `/workspace/zfb-rustup`. Manager build settings:
  `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=4`.
- Heavy commands use the published portable profile's
  `.codex/scripts/heavy-guard.sh`, with its original machine-wide HOME queue.
  No alternative lock or second cold target was created.
- Portable startup snapshot validated
  `d359c1d1db723a5677aee1ce168586b87f16dc1758cdfc27f5063d5f4e86530e`.
  Startup instructions arrived through workspace context; delivery through the
  saved startup field was not demonstrated. Explicit context loading is not
  claimed as native skill discovery.

## Extraction gate (#4236)

Fresh checks on the reviewed extraction:

| Command (under settings above) | Result |
| --- | --- |
| `cargo test --locked -p zfb-render --lib --test config_eval --test embedded_v8_smoke --test embedded_v8_plugin_resolver` | 210 library + 14 config + 18 plugin + 32 smoke tests passed; guard PASS, 4 seconds. |
| `cargo test --locked -p zfb-js-host --features services` | 9 tests passed. This unit-only invocation inadvertently omitted the profile overrides; no heavy process ran concurrently. |
| `node_modules/.bin/vp test run scripts/__tests__/showcase-workflow-tools.test.mjs` | 6 tests passed. |
| `node_modules/.bin/vp fmt --check scripts/__tests__/showcase-workflow-tools.test.mjs` | Passed after formatting. |
| `git diff --check` and staged equivalent | Passed. |

Independent read-only extraction review compared canonical transport, crypto,
limits, dispatch types/mode, module authorization and retained tests against
main. It found a missing-backend diagnostic regression in bridge getters;
guarded getters and an exact diagnostic/untouched-buffer regression fixed it.
The reviewer confirmed the correction. Independent upload-guard review passed:
only `showcase-preview` gained a draft conjunct, with existing verification jobs
and workflow triggers unchanged. Docs preview already has a draft guard;
production deploys require main, releases require release/manual triggers.
Existing baseline CI remains distinct from direct-V8 prototype verification.

## Baseline characterization

`cargo build --locked -p zfb-render --example direct_v8_baseline` passed under
the guard (1 second). The runner is a separate native baseline process; its
ordinary Deno evaluator is unchanged. `check-config.py` sends the exact source
strings in `config-cases.json` and checks independently specified literal
values, error categories and message fragments, rather than accepting mere
agreement between engines.

`python3 prototypes/direct-v8/check-config.py --baseline target/debug/examples/direct_v8_baseline --output /tmp/direct-v8-config-baseline.json`
passed **55/55**: 25 success values and 30 errors. Candidate results were pending at this initial characterization; later gates below supersede that status.
Every line creates and drops a fresh isolate; the runner exits normally.

Baseline module characterization observed:

- Main `file:///zfb/baseline-probe.mjs`, `main: true`, filename
  `/zfb/baseline-probe.mjs`, dirname `/zfb`.
- `Object.keys(import.meta).sort()` is
  `["dirname","filename","main","resolve","url"]`.
- `import.meta.resolve('./identity.mjs')` is `file:///zfb/identity.mjs`.
- Static and both dynamic imports have the same namespace, dependency evaluates
  once, exported marker is 42; dependency `main` is false.

## Failures, retries and limits of current evidence

- Host pnpm 11.25 rejected the repo's pnpm >=12 requirement. Installation and
  git hooks succeeded using the pinned pnpm 12.8.2 via `npm exec`; hooks were not
  disabled. npm's default HOME cache was unavailable, so the repository-owned
  `/workspace/zfb-npm-cache` was used.
- The heavy guard's first setup probe could not create its HOME lock in the
  sandbox. Only that queued process was terminated, then the same command ran
  with escalation and the original queue. It passed. The upstream hello-world
  link probe is environment evidence, **not candidate acceptance**.
- The first extraction test run failed one of 209 library tests because the
  intermediate bridge cached/wrapped unavailable ops. Getter correction passed
  all 209; subsequent independent review added the absent-backend case and the
  final reviewed run passed all 210. No assertion was weakened.
- A transient executor disconnect occurred during checkpoint commit. A read
  confirmed the commit had succeeded before any retry; the checkpoint was then
  pushed. No completed native build was repeated because of the disconnect.
- GitHub CLI API reads returned Forbidden, including the scoped escalated
  retry. Authorized connector reads/writes and git transport work; no denied API
  operation was bypassed or repeatedly retried.
- At the extraction checkpoint, native config (#4237), async/cancellation (#4238), module/Worker (#4239),
  generated bundles, source maps, graph isolation and bounded hydration (#4240)
  were **pending**, not passed by the extraction tests or baseline CI. Later
  sections record the subsequent gates; module/Worker and generated evidence
  remain pending at the async checkpoint.
- Darwin arm64/x64, Linux arm64 and Windows x64 candidate execution are not run.
  Existing repository CI platforms do not imply candidate platform evidence.

## Native config gate (#4237)

**Corrected gate passed:** the 18 shared/native unit tests pass, and both
independent runners pass **62/62** literal config cases. The guarded corrected
test plus example-link command completed in 2 seconds. Independent lifetime and
bootstrap re-review is clear. `evidence/config-gate.json` retains every actual
receipt and executable/corpus SHA-256; `evidence/config-graph.json` records the
152 selected package/feature rows and exact V8 version, with no Deno-named
package, `serde_v8`, compiler or baseline renderer in the candidate closure.

The corrected native command adds
`ZFB_V8_ICU_DATA=/workspace/zudo-front-builder/target/direct-v8-inputs/icu77/icudtl.dat`
to the settings above. Run `python3 prototypes/direct-v8/prepare-icu.py` first
(or pass `--archive` for an already fetched, checksum-verified Cargo archive).
Missing ICU input fails compilation; there is no runtime fallback. Official V8
prebuilt archive was used; no ad hoc engine build or V8-version change occurred.

The first newly added bootstrap-installation test failed (17/18 passed): V8's
sloppy `set` reported success for a readonly property without installing the
callback. Changing bootstrap to `create_data_property` corrected the check;
the unchanged assertion now passes. The expanded corpus initially had one
incorrect URL-object expectation: the canonical URL shim serializes a file URL
with a single slash. The observed baseline behavior was recorded as its own
literal case; the boxed-String coercion case independently tests resolve.

ICU, chained self-import, and resolve arity/coercion failures described below
are now corrected and covered. Multiple live hosts on one thread are rejected
explicitly to prevent non-LIFO `OwnedIsolate` disposal; finite sequential hosts
and recovery after errors are verified. The following section records the subsequent async gate.

The first real candidate link passed under the guard in 9 seconds:
`cargo build --locked -p zfb-js-host --no-default-features --features direct-v8 --example direct_v8`.
Both independent runners passed the original 55 fixed config expectations.
The selected `cargo tree --edges normal,build` closure contains no Deno runtime
crate or `serde_v8`; the final selected-feature receipt is now `evidence/final-graph.json`.

Additional probes found real failures outside that initial corpus:

- `Intl.DateTimeFormat` fatally trapped without ICU data (exit -5); baseline
  returned the fixed date/number/locale results. This is a bootstrap failure,
  not an accepted omission. `prepare-icu.py` now prepares the exact baseline
  ICU 77 bytes from the checksum-pinned data archive. No Deno Rust code or
  runtime dependency is linked for this data input. Data SHA-256:
  `1cf67874b5a87a8363a86fb3f81e3cbbed54d389062dab8fb52308d5cf8c8612`.
  It is local/ignored under `target/direct-v8-inputs`; the candidate build uses
  the explicit `ZFB_V8_ICU_DATA` path. The corrected config and final generated gates pass.
- Independent review found a chained dynamic self-import was not fully
  drained. Actual paired probe returned `{done:true}` in baseline and
  `{done:false}` in candidate. The correction and regression now pass in the final native/config suites.
- Review found import.meta.resolve coercion/arity divergence; correction and
  fixed probes now pass in the final native/config suites.
- Both hosts correctly rejected an uncaught queued callback and an unhandled
  promise rejection, preserving authored message/location. Both rejected an
  unresolved top-level await promptly; no watchdog kill was necessary. Candidate
  currently states the unsupported no-pending-work condition explicitly.

## Async native gate (#4238)

The final reviewed native suite passes **29 tests** (9 shared plus 20 native),
including the async config extension. Guard PASS: 3 seconds for tests and the
example relink. The same driver handles V8 foreground tasks, module promises,
dispatch promises and owned Rust transport futures. No additional Cargo package
was added. Config now also awaits real `WebAssembly.compile` platform work;
the expanded fixed config corpus passes **63/63 in each runner**.

`service-cases.json` passes **27/27 in each runner**. The deterministic
`check-http.py` differential harness passes **8/8 paired cases**: parallel
request handshake plus sibling microtask progress, binary/duplicate headers,
303 and 307 method/body behavior, preabort, build denial, raw-callback denial
and forged-mode denial. It records identical source bytes, bound endpoint and
server request observations for each pair; request counters do not masquerade
as socket-closure evidence. See `evidence/services-gate.json` and
`evidence/http-gate.json` for actual receipts/hashes.

The native temporal tests separately prove:

- An abort followed immediately by a completed dispatch closes the partially
  read loopback socket while the same host remains alive and idle. The server's
  closure semaphore completes before any subsequent host operation or drop.
- Orphan A retains its captured two-request redirect budget while B receives
  a fresh budget and completes two requests. The third A hop never opens.
- Caller abort reason, host deadline versus a later caller deadline, narrower
  caller timeout, late abort, settled/unknown cancel and cancellation before
  first native poll preserve first completion and leave no pending entry.
- Typed request bytes are copied before authored mutation; response bytes and
  duplicate headers are copied out. Small configured request/response caps and
  timeout fail with real errors. OS entropy invocation is instrumented; injected
  entropy failure leaves the original view untouched, with no weak fallback.
- Async Wasm foreground wakeups complete. Thirty-two unfinished-Wasm
  create/drop cycles followed by a fresh awaited compile succeed.

Independent lifetime/authority review found and corrected a stale foreground
queue snapshot before liveness checks; the final order snapshots background
work before rechecking the queued work. Platform callback boundaries contain
Rust panics, including a custom executor Waker panic. Review found no remaining
concrete blocker. The exact pinned V8 API routes platform callbacks by raw
isolate address, as does the current Deno baseline. Pinned Wasm teardown
cancels foreground tasks and waits for background work; the finite tests prove
that reachable path, not a generic generation-isolation or catastrophic
recovery guarantee. CPU-bound interruption and hard heap isolation remain
outside the existing contract.

An additional raw throwing-getter diagnostic is retained in
`raw-getter-negative.json`. The baseline aborted with a non-unwinding
`serde_v8` panic; the candidate returned a transport-specification error and
continued to return 42. This is not in the passing differential corpus and does
not redefine incidental raw-op inputs as a supported application API.

## Normal graph integration checks so far

- Normal `cargo build --locked --workspace --all-targets`: PASS, 103 seconds.
  Initial attempt failed downloading pinned esbuild after five retries. The
  documented override was supplied with npm's exact 0.25.12 Linux binary,
  independently checked against the repository's SHA-256
  `bab29b2ca7a9e89b67cf720b77b2d743f9f31f5cf0d5bd74ee8c8de30ced7014`.
- `cargo check --locked --no-default-features -p zfb --tests`: PASS, 74 seconds.
- Existing `scripts/assert-zfb-md-wasm-graph.sh`: all seven surfaces and both
  compiler boundaries passed. Real default md-Wasm `cargo rustc --locked -p
  zfb-md-wasm --target wasm32-unknown-unknown --lib --crate-type cdylib` linked
  successfully in 23 seconds.
- Isolated `cargo rustc --locked -p zfb-css-node --lib --no-default-features
  --crate-type cdylib`: PASS, 75 seconds. Its resulting library was copied to an
  ignored `.node` path and `node scripts/smoke-css-node.mjs` passed a real
  Linux x64 load and CSS compilation.
- At pushed checkpoint `4c6e619df3f185c377ca5edc19afc0f5ab7fdaff`, existing CI
  binary/scaffold/built-site verification passed; showcase preview and
  production deploy jobs explicitly skipped in run `38536741157`. No candidate
  platform fleet was activated. CI no-V8 and seven md-Wasm rows passed.
- That checkpoint's health/docs jobs failed stale generated fingerprints and
  Cargo-lock provenance assertions. A reviewed Wind-only closure-preserving
  refresh and genuine preview manifest regeneration are being reconciled.
  Existing island-size policy permits only additive lock records, while this
  extraction changes the existing render record. No size ceiling, historical
  provenance or unrelated #4234 policy was changed to silence those failures.

Playwright's pinned browser download returned `403 Domain forbidden`, including
the scoped escalated retry. The preinstalled local Chromium is
`154.0.8037.57` (Debian 13); the bounded hydration run recorded this actual
browser and passed. It does not claim the unavailable downloaded revision.

## Outer typed-config characterization

The unchanged normal CLI's real TS-config loader was executed through
`crates/zfb/examples/direct_v8_outer_config.rs` (default Deno backend), including
real pinned esbuild preparation. `check-outer-config.py` passes **7/7** and
`evidence/outer-config-gate.json` retains requests and receipts. This resolves
CFG-08/A-01 without tightening raw candidate semantics: `{}` and `[]` both
produce defaults, `['array-output']` sets `outDir`, `[42]` fails at `[0]` expecting
a path string, a top-level Date's serialized string fails typed Config parsing,
and an invalid `port` names that field. User config overrides its preset.
Only the temporary fixture directory is replaced with `<fixture>` in errors.
This is baseline outer-validation evidence, not candidate CLI integration.
The example build was fresh, manager guarded, PASS in 43 seconds. A brief
execution-server disconnect during harmless reads recovered in the same session;
the already completed async checkpoint `6706ab18` remained pushed.

## Initial module baseline characterization

`check-modules.py` prepares one fixed filesystem/JSON corpus before invoking any
runtime. The initial baseline passed **37/37** Worker/module policy cases, including
canonical alias namespace identity and evaluation-once, symlink authority,
percent-encoded sibling names, missing and TypeScript aliases, virtual modules,
strict invalid shapes, actual binary headers/body dispatch, and compiled but
uninstantiated copied Wasm consumed as answer 42. The final expanded paired
47-case gate below supersedes this initial baseline-only observation.

Initial authored expectations produced 33/36 then 35/36 passes: bare unresolved
imports fail at resolution (not the later unknown-load message), and the Node
stub sentinel is a module export rather than a global. Inspecting canonical
source corrected those fixture mistakes. A new baseline probe additionally
established that authored `file:` modules cannot import the internal `ext:`
helper directly; node/ext internal imports may use it. This is now a separate
negative case, not normalized away. Original failure receipts remain at
`/tmp/direct-v8-module-baseline-{1,2}.json`; no product assertion was weakened.

## Native module and Worker gate (#4239)

Final frozen source passes **40 native tests** (0.25 seconds; guarded test plus
runner link PASS in 3 seconds) and **47/47 paired module/Worker/utility cases**.
The config **63/63**, services **27/27**, and loopback HTTP **8/8** gates were
rerun with the actual module-backed candidate script runner. Final receipts
live in `evidence/{modules,config,services,http}-gate.json`; the final native
log is `/tmp/direct-v8-module-tests-final.log`.

Canonical shared resolution/source authorization is reused by a native V8
cache with canonical found URLs. Tests execute static/dynamic/cyclic ESM,
namespace identity, top-level await, import.meta, virtual/alias sources,
strict inner validation and wrapper reuse, copied Wasm compilation and real
instantiation, binary requests and ordered duplicate headers. Utility modules
remain tolerant; narrow get-export and sync/async default-call probes cover
JSON values, boxed strings and string-key Maps. Non-string Map keys fail.
This is not a general replacement for all serde_v8 conversions.

Independent native review found a caught dynamic ext-import Error/TypeError
difference, boxed String and Map conversion differences, and missing syntax
error coordinates. Literal probes reproduced each applicable difference before
correction. The first coordinate fix exposed a real V8 unknown-column sentinel
overflow in broad corpora (module runner stopped after 21 receipts; config after
28), despite 39 focused tests passing. Checked column conversion and a no-source
error regression corrected it; both message paths omit unavailable coordinates
rather than invent them. The fixed syntax dependency retains exactly
`file:///zfb/syntax-dependency.mjs:3:14`. Original failure receipts are preserved
under `/tmp/direct-v8-{modules,config}-diagnostic-overflow.json`.

A standalone unresolved-top-level-await probe returned explicit errors from
both engines and exited normally, without its eight-second watchdog firing.
The candidate reports no pending native work; the baseline reports unresolved
TLA with a module location. This does not prove CPU-loop interruption, hard
heap limits, catastrophic recovery or shutdown of indefinitely active work.
Finite host cycles, pending Wasm disposal, failed boot/dispatch recovery and
live-host idle transport cancellation have separate successful evidence.

The generated-input producer and its single Chromium checker passed independent
source review. They are authored and compiled prerequisites only at this gate;
real generated-bundle execution and hydration remain the next gate.

## Real generated inputs and bounded hydration (#4240)

The generated-input gate **passes** on Linux x86_64. The normal ZFB CLI produced
one frozen 54-file snapshot from clean fixture checkpoint
`f2ffe216997601f5e0aa68b5625553e6700cb206`, with actual compiler build provenance,
tool hashes/versions, policy inputs and each emitted file's bytes/hash recorded
in `evidence/generated-manifest.json`. Its SHA-256 is
`85708be0784247c25b95bfda3eb9cb146d39e85367499b958ddecccde92e8fed`.
The hash above identifies the original frozen manifest. Commit hooks reformatted
the tracked JSON copies; parsed equality with the retained originals was verified,
and both original and formatted hashes are documented in `PRODUCER.md`.
The source compiler had been built at `8dcdfb35`; the intervening commit changed
only prototype fixtures/harness/evidence, and this distinction is recorded.

Both independent native runners passed eight packets: emitted config; five
real JSX/MDX/content/paths/CSS-module/SDK/Wasm route observations; the actual dev
wrapper with separately validated unchanged inner; prepared plugin virtual/alias
sources with binary POST and exact binary response; authored mapped throw; and
fresh finite host cycles after failure. Seven successful packets have exact
response/value parity, not just matching markers. The canonical Rust mapper
resolves both thrown stacks to `throw.ts:6:9` with exact authored source content.
Wrapper/bootstrap URLs are not fed into the inner source map.

The one local Chromium representative passed using version `154.0.8037.57`:
candidate-rendered HTML with emitted client/CSS assets shows `Count: 0`, computed
color `rgb(12, 34, 56)`, and after a real click `Count: 1`, with zero page or console
errors. The runtime/browser receipt is `evidence/generated-gate.json`.
Preparation took 2 guarded seconds and consumption/browser 3 guarded seconds;
these are fixture-run observations, not performance comparisons.

The first real CLI producer failed before candidate execution: production asset
postprocessing reads rendered routes as UTF-8 and rejected an opaque binary
page. Its original source and independent binary expectation remain under
`negative/producer-binary/`, and `evidence/producer-first-failure.json` preserves
the actual failure. The positive real site excludes that unsupported output;
a separate emitted Worker still proves exact `[0,255,128,65]`, status 201 and
ordered duplicate headers in both hosts. No product pipeline or oracle was
weakened to accept candidate output. Preparation also corrected an argv0 mistake
where resolving the `rustc` symlink invoked `rustup`; the successful manifest
records the actual rustc version. Failed output directories remain untouched.

## Ownership, dependencies and costs

`evidence/final-graph.json` records **152 selected package/feature rows**, exact
`v8 147.4.0` with `default,use_custom_libcxx`, and no Deno runtime crate,
`serde_v8`, `zfb-render` or `zfb-build` in the candidate's normal/build closure.
The only new Cargo.lock package record is unpublished `zfb-js-host 0.0.0`.
No new external package/version or second HTTP/TLS/crypto/web-platform family
was introduced. The default-empty leaf enables shared services explicitly and
native V8 only through non-default `direct-v8`; the default CLI stays on Deno.
ICU77 is checksum-pinned data preparation from the exact baseline data archive,
not a linked Deno crate or alternative JS interpreter.

Approximate physical source footprint, including comments/blanks, is recorded
in `evidence/footprint.json`: 2,539 native implementation lines in 13 files,
734 native regression-test lines in three files, 4,744 mostly extracted shared
source/shim lines in 19 files, and 837 lines of evidence runners/postprocessors.
The native additions own isolate/bootstrap, fixed callbacks, task/promise driving,
module identity and narrow Worker conversions. Canonical transport, authorization,
crypto and shims stay shared rather than being independently reimplemented.
The temporary differential harness is not a permanent all-backend/platform CI
matrix. The private-wrapper export is explicitly an ignored verification test;
its governance manifest and ignore-parity check were updated (85, zero uncovered).

Native coupling remains real: V8 callback scopes require unsafe entry, pinned
147.4.0 raw isolate pointer bits are used only as foreground-queue registry keys,
and teardown relies on upstream isolate/Wasm task cancellation. No unsafe
Send/Sync wrapper or cross-thread engine entry was added. Independent review
and finite disposal tests are evidence for these paths, not a general address
reuse guarantee, hostile-code sandbox or CPU/heap isolation proof. The owning
thread rejects overlapping hosts to preserve isolate drop ordering. More general
multi-isolate scheduling, arbitrary serde conversion, platform adapters and
actual CLI/factory/reload ownership remain future work.

Build/setup cost is separate from runtime evidence: pinned tool installation,
ICU data preparation and official V8 prebuilt linking were required. Reported
3-second final native checks reuse compiled dependencies; they are not cold-build
benchmarks. Normal compiler production and final integration checks are logged
separately. No runtime speed, binary-size advantage, RSS reduction or advisory
elimination is claimed. Six GiB of disposable Cargo incremental cache was removed
when local disk became tight; retained artifacts/evidence were not removed.

## Contract row disposition

The table describes the accepted phase-1 boundary. A pass for a preserved
production caller does not claim candidate CLI adoption. Existing limitations
are classified separately; platform and future integration obligations remain
explicit. Machine-readable equivalent: `evidence/contract-status.json`.

| Contract row | Disposition | Evidence / limit |
| --- | --- | --- |
| GRAPH-01 | pass | Exact final Linux normal/build dependency graph; baseline workspace/no-V8 and isolated md-Wasm/CSS checks. |
| GRAPH-02 | pass | Exact final Linux normal/build dependency graph; baseline workspace/no-V8 and isolated md-Wasm/CSS checks. |
| NATIVE-01 | pass | 40 native tests; independent ownership/callback review; finite Wasm disposal and fresh-host recovery. |
| CALL-01 | pass | Existing production callers preserved; baseline controls plus standalone native DTO/utility probes. Candidate CLI integration remains phase 2. |
| CALL-02 | pass | Existing production callers preserved; baseline controls plus standalone native DTO/utility probes. Candidate CLI integration remains phase 2. |
| CALL-03 | pass | Existing production callers preserved; baseline controls plus standalone native DTO/utility probes. Candidate CLI integration remains phase 2. |
| CALL-04 | pass | Existing production callers preserved; baseline controls plus standalone native DTO/utility probes. Candidate CLI integration remains phase 2. |
| CALL-05 | pass | Existing production callers preserved; baseline controls plus standalone native DTO/utility probes. Candidate CLI integration remains phase 2. |
| BOOT-01 | pass | 63 config and 27 service cases; native bootstrap tests; canonical shim order retained. |
| BOOT-02 | pass | 63 config and 27 service cases; native bootstrap tests; canonical shim order retained. |
| BOOT-03 | pass | 63 config and 27 service cases; native bootstrap tests; canonical shim order retained. |
| BOOT-04 | unsupported-by-existing-contract | Real delays/clearTimeout/cloned MessagePort delivery remain unsupported; paired cases prove existing no-op/reference semantics. |
| CFG-01 | pass | 63 literal paired config cases, including async Wasm and stringify tampering. |
| CFG-02 | pass | 63 literal paired config cases, including async Wasm and stringify tampering. |
| CFG-03 | pass | 63 literal paired config cases, including async Wasm and stringify tampering. |
| CFG-04 | pass | 63 literal paired config cases, including async Wasm and stringify tampering. |
| CFG-05 | pass | 63 literal paired config cases, including async Wasm and stringify tampering. |
| CFG-06 | pass | 63 literal paired config cases, including async Wasm and stringify tampering. |
| CFG-07 | pass | 63 literal paired config cases, including async Wasm and stringify tampering. |
| CFG-08 | pass | Seven real normal-CLI outer-loader probes; raw arrays preserved and typed sequence behavior explicitly observed. |
| CFG-09 | unsupported-by-existing-contract | Node fallback array rejection is an existing path difference; no slim-path parity claim or behavior change. |
| MOD-01 | pass | 47 paired module/Worker/utility cases; canonical policy controls and generated copied-Wasm consumption. |
| MOD-02 | pass | 47 paired module/Worker/utility cases; canonical policy controls and generated copied-Wasm consumption. |
| MOD-03 | pass | 47 paired module/Worker/utility cases; canonical policy controls and generated copied-Wasm consumption. |
| MOD-04 | pass | 47 paired module/Worker/utility cases; canonical policy controls and generated copied-Wasm consumption. |
| MOD-05 | pass | 47 paired module/Worker/utility cases; canonical policy controls and generated copied-Wasm consumption. |
| MOD-06 | unsupported-by-existing-contract | Runtime TS/TSX transpilation remains rejected; canonicalize/read TOCTOU remains an inherited limitation. |
| MOD-07 | pass | 47 paired module/Worker/utility cases; canonical policy controls and generated copied-Wasm consumption. |
| MOD-08 | pass | 47 paired module/Worker/utility cases; canonical policy controls and generated copied-Wasm consumption. |
| MOD-09 | pass | 47 paired module/Worker/utility cases; canonical policy controls and generated copied-Wasm consumption. |
| MOD-10 | pass | 47 paired module/Worker/utility cases; canonical policy controls and generated copied-Wasm consumption. |
| WORK-01 | pass | 47 paired cases and eight generated packets per engine; unmodified canonical wrapper/inner and five real routes. |
| WORK-02 | pass | 47 paired cases and eight generated packets per engine; unmodified canonical wrapper/inner and five real routes. |
| WORK-03 | pass | 47 paired cases and eight generated packets per engine; unmodified canonical wrapper/inner and five real routes. |
| WORK-04 | pass | 47 paired cases and eight generated packets per engine; unmodified canonical wrapper/inner and five real routes. |
| WORK-05 | pass | 47 paired cases and eight generated packets per engine; unmodified canonical wrapper/inner and five real routes. |
| HTTP-01 | pass | Canonical transport tests, 27 service cases, eight paired loopback probes and native limits/budget tests. |
| HTTP-02 | pass | Canonical transport tests, 27 service cases, eight paired loopback probes and native limits/budget tests. |
| HTTP-03 | pass | Canonical transport tests, 27 service cases, eight paired loopback probes and native limits/budget tests. |
| HTTP-04 | pass | Canonical transport tests, 27 service cases, eight paired loopback probes and native limits/budget tests. |
| HTTP-05 | pass | Canonical transport tests, 27 service cases, eight paired loopback probes and native limits/budget tests. |
| HTTP-06 | pass | Canonical transport tests, 27 service cases, eight paired loopback probes and native limits/budget tests. |
| HTTP-07 | pass | Canonical transport tests, 27 service cases, eight paired loopback probes and native limits/budget tests. |
| HTTP-08 | pass | Canonical transport tests, 27 service cases, eight paired loopback probes and native limits/budget tests. |
| HTTP-09 | pass | Canonical transport tests, 27 service cases, eight paired loopback probes and native limits/budget tests. |
| CANCEL-01 | pass | Native live-host idle socket closure, preabort/settled/race tests and paired loopback probes. |
| CANCEL-02 | pass | Native live-host idle socket closure, preabort/settled/race tests and paired loopback probes. |
| CANCEL-03 | pass | Native live-host idle socket closure, preabort/settled/race tests and paired loopback probes. |
| CRYPTO-01 | pass | 27 literal service cases; native OS-entropy instrumentation, offsets/failure tests; canonical digest tests. |
| CRYPTO-02 | pass | 27 literal service cases; native OS-entropy instrumentation, offsets/failure tests; canonical digest tests. |
| CRYPTO-03 | pass | 27 literal service cases; native OS-entropy instrumentation, offsets/failure tests; canonical digest tests. |
| CRYPTO-04 | pass | 27 literal service cases; native OS-entropy instrumentation, offsets/failure tests; canonical digest tests. |
| DIAG-01 | pass | Boot/dispatch console evidence, retained canonical console-limit tests; syntax origin and exact mapped authored throw. |
| DIAG-02 | pass | Boot/dispatch console evidence, retained canonical console-limit tests; syntax origin and exact mapped authored throw. |
| LIFE-01 | pass | Fresh hosts for corpora, 32 pending-Wasm disposal cycles, repeated generated boot/dispatch/drop and recovery. |
| LIFE-02 | unsupported-by-existing-contract | Unresolved TLA fails explicitly without watchdog; CPU interruption/hard heap limits/catastrophic recovery are not provided. |
| LIMIT-01 | unsupported-by-existing-contract | Existing absent/limited platform APIs retained; no new runtime platform promised. |
| EVID-01 | pass | Frozen 54-file manifest, separate native receipts, independent literals and exact response parity. |
| EVID-02 | pass | Frozen 54-file manifest, separate native receipts, independent literals and exact response parity. |
| EVID-03 | pass | One Chromium154 session: candidate HTML, emitted CSS, SSR Count0 and hydrated click Count1; no page/console errors. |
| EVID-04 | fail | Local selected checks pass; current-head CI must reconcile existing island-size Cargo-lock provenance policy. No ceiling/policy/provenance bypass. |
| EVID-05 | pass | Linux x86_64 native execution passed. Darwin arm64/x64, Linux arm64 and Windows x64 not run; no platform fleet. |

## Recommendation and next authorization boundary

**Continue narrowly with the standalone prototype; do not cut over the CLI.**
The local runtime feasibility gate passes, including the mandatory native,
cancellation, real-bundle, source-map and hydration evidence. Integration remains
blocked by the repository's existing island-size Cargo-lock provenance policy;
this task does not activate a new policy, fabricate old-lock evidence, change
size ceilings or adopt unrelated draft #4234. That draft was confirmed open and
unmerged during final review. The sole task PR remains draft and all six delivery
issues remain open.

After that independent integration blocker is resolved through its authorized
workflow, the next separately authorized step is opt-in actual CLI/config/factory
integration with dev start-before-swap, failure rollback, thread lifecycle and
reload tests. Then validate all supported native/package platforms and selected
Rolldown output interoperability without adopting draft #4120 here. A default
switch and final Deno deletion require their own authorization and evidence.
No later phase, deployment, preview upload, release or version change ran here.

## Final integration reconciliation

The final affected baseline invocation passed **296 tests**: 210 library,
14 config, 4 browser-event, 3 console, 9 dispatch-mode, 6 Node-stub, 18 plugin
resolver and 32 smoke. The same serialized lane completed normal
`cargo build --locked --workspace --all-targets` and
`cargo check --locked --no-default-features -p zfb --tests` (57 guarded seconds
total; `/tmp/direct-v8-final-integration.log`). Its source is the final Rust
implementation committed at `8dcdfb35`; later changes are fixture/evidence/docs.
The all-target build can overwrite example binaries with other feature
selections, so the explicit direct-V8 and no-V8 postprocessor commands restored
the intended executables before generated acceptance. Receipts bind their hashes.

Native `cargo clippy --locked -p zfb-js-host --no-default-features --features
direct-v8 --all-targets -- -D warnings` passed. Workspace
`cargo clippy --locked --workspace --all-targets -- -D warnings` passed, followed
by the real no-V8 CLI build and genuine generation/check of **173 Wind preview
samples** (93 guarded seconds; `/tmp/direct-v8-final-docs-integration.log`).
Only the preview manifest's compiler source digest changed; emitted sample
assets did not. Preview-generator tests pass **19/19**. The reviewed Wind support
digest refresh separately passed **22/22**, with all 155 Wind dependency records
unchanged. Final `cargo fmt --all -- --check` and `git diff --check` pass.
Earlier isolated md-Wasm linking, seven graph surfaces, CSS addon native load
and CSS compilation are reused: their manifests, canonical feature boundaries
and changed shared-default edges are unchanged by the subsequent opt-in native
implementation. They are not rerun platform-fleet evidence.

The remaining local policy check is explicitly **red: 30/32** in
`scripts/__tests__/island-size-budget.test.mjs`. Both failures are the existing
Linux/Darwin reviewed-contract assertions: local Cargo.lock SHA and workspace/
packed provenance differ from the historical contract. This is not a measured
size growth. Existing cargo-producer-lock re-pin accepts added records only,
while the necessary extraction modifies the existing zfb-render record; the
other current reasons do not authorize that movement. No island measurement,
ceiling, saved provenance, policy or related PR was changed. A fresh platform
fleet is forbidden and would not by itself authorize the policy mismatch.

At checkpoint `8dcdfb35`, PR preview workflow `38541671790` built docs successfully
and explicitly **skipped** upload job `115778713897`. Earlier showcase verification
also passed with preview and production jobs skipped. Final-head workflow links
and conclusions are recorded in the draft PR description after the final push;
previous-checkpoint green jobs are not substituted for current-head status.
The unresolved provenance test remains an integration blocker, not a passed or
deferred native capability. All six issues stay open for review/integration.
