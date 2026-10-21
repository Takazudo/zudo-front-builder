# Phase 1 execution findings

> Phase 2 status: the nondefault actual CLI integration is on `main` (merged by
> #4368 from `feat/4301-direct-v8-cli`); see [CLI.md](CLI.md) for selection, ownership and
> current verification status. The phase-1 observations below remain historical
> and source-bound; an old hold is neither a current implementation claim nor
> proof of phase-2 acceptance.

The standalone Linux runtime feasibility gate passes, and was replayed from one
documented command on a second machine on 2026-10-14 (see "Fresh Linux replay"
below). Delivery remains blocked by the existing island-size Cargo-lock
provenance check: the bounded `cargo-internal-rewire` re-pin capability that
can admit this branch's lock move exists as draft PR
[#4259](https://github.com/Takazudo/zudo-front-builder/pull/4259) (stacked on
held #4234) and its activation inputs and checklist live in
[ACTIVATION.md](ACTIVATION.md); no contract has been re-pinned. This is not a
claim that all required CI passes or that candidate CLI integration is complete.
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
| EVID-04 | fail | Local selected checks pass (replayed 2026-10-14); current-head CI must reconcile the island-size Cargo-lock provenance policy through the paired `cargo-internal-rewire` activation in ACTIVATION.md once #4259 integrates. No ceiling/policy/provenance bypass. |
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

## Fresh Linux replay through the acceptance entry point (2026-10-14, #4258)

A second session replayed the whole selected Linux acceptance from one
documented command, `prototypes/direct-v8/accept.py` (see README), on a
different machine from the original run. This is **fresh evidence** that
supersedes nothing above: the original receipts in `evidence/` are preserved,
and the fresh receipt is `evidence/acceptance-linux-2026-10-14.json`
(condensed from `target/direct-v8-accept-5/acceptance.json`; the full gate
JSON stays ignored under `target/`). A fresh session is not a cold-build
benchmark; the recorded step durations reuse compiled dependencies.

- Source: ``34ffcba76fa54e88745656c609b2e5a178ae4227`` (clean tree; the producer recorded `status` empty).
  Linux x86_64 (WSL2), rustc `1.99.0 (b940084d7 2026-09-28)` selected with
  `RUSTUP_TOOLCHAIN=1.99.0` to match the shared target, Node `v24.13.1`,
  Python 3.12.3, machine-wide Cargo `target-dir` honored via `cargo metadata`.
  Heavy commands ran through this host's published `heavy-guard.sh`.
- ICU: the pinned `deno_core_icudata-0.77.0` archive from the local Cargo
  cache (`--icu-archive`); data SHA-256 `1cf67874…8612`, equal to the pin.
- Executables (digests bound before the gates and re-read after each one,
  unchanged throughout): baseline `direct_v8_baseline` `c7405139…`, candidate
  `direct_v8` `a6711fd7…` (`666def06…` before the registry-guard fix in
  `d2b57fdb`, see the re-review below), outer-config `779eda2f…`, postprocessor
  `5e19aff9…`, normal CLI `zfb` `cc597199…`, wrapper test binary `ef756011…`.
  Both engine probes answered with their exact engine identity
  (`deno_core 0.399.0 / v8 147.4.0`, `direct v8 147.4.0`) and the probe value.
- Native library tests: **40 passed, 0 failed**; shared-services tests
  (`--features services`): **9 passed** (fresh).
- Paired gates, each engine: config **63/63**, services **27/27**, modules
  **47/47**; loopback HTTP **8/8** paired rows; outer typed config **7/7**.
- Generated inputs: the normal CLI (built here, esbuild 0.25.12 verified
  against the repository pin `bab29b2c…`) produced a new frozen **54-file**
  snapshot (manifest ``5bd9bab4…2a32``), consumed by both engines: **8/8**
  packets each, **7/7** exact parity rows, authored throw mapped to
  `throw.ts:6:9`, and one Google Chrome ``146.0.7680.153`` session on the
  candidate-rendered HTML with the frozen client/CSS bytes: `Count: 0`,
  computed `rgb(12, 34, 56)`, `Count: 1` after a real click, no page or
  console errors. The snapshot's nondeterministic bytes differ from the
  original run's, as expected; both engines consumed the same bytes.
- Original failures in this replay: two harness mistakes in the new entry
  point, not engine failures. Run 1 reported the generated gate failed because
  the entry point passed a file path where `check-generated.py` takes a
  directory (the gate itself had passed 8/8 + browser); run 2 reported the
  config/services/modules gates failed because the entry point demanded a
  top-level `passed` key that those checkers never write (each engine had
  passed 63/63, 27/27, 47/47). Run 4 was refused by the verifier itself:
  the newly added services-test step rebuilt `examples/direct_v8` without
  `direct-v8` right before verification, which is the all-targets trap in
  miniature; the step was reordered. All three fixes are unit-tested
  (`test_accept.py`, 10 cases) and the gates were rerun from scratch (run 5).
  No oracle, corpus or assertion changed.
- The all-targets trap was demonstrated on the real target after the replay:
  `cargo build --locked --workspace --all-targets` left
  `examples/direct_v8` as the feature-disabled form, `accept.py`'s verifier
  refused it (``a3bae3c6…` refused as the feature-disabled form`), the explicit feature build restored it, and the
  verifier accepted it again (``666def06…``).

Batched affected checks after the replay (one guard lane, same source):

| Check | Result |
| --- | --- |
| `cargo test --locked -p zfb-render --lib --test config_eval --test embedded_v8_smoke --test embedded_v8_plugin_resolver --test embedded_v8_browser_event --test embedded_v8_console_logs --test embedded_v8_dispatch_mode --test embedded_v8_node_stubs` | **296 passed** (210 + 14 + 32 + 18 + 4 + 3 + 9 + 6) |
| `cargo test --locked -p zfb-js-host --features services` | 9 passed |
| `cargo build --locked --workspace --all-targets` | pass, 1512 s (then the trap above) |
| `cargo check --locked --no-default-features -p zfb --tests` | pass |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | pass |
| `cargo clippy --locked -p zfb-js-host --no-default-features --features direct-v8 --all-targets -- -D warnings` | pass (a first attempt ran without `ZFB_V8_ICU_DATA` in its shell and failed to compile; that was the lane script, not the crate) |
| `cargo fmt --all -- --check` | pass |
| `scripts/assert-zfb-md-wasm-graph.sh` | pass (seven surfaces, both compiler boundaries) |

Not run here: Darwin arm64/x64, Linux arm64, Windows x64; the normal CLI on
the candidate host (phase 2); `pnpm b4push`; the isolated wasm32 md-Wasm link
and CSS addon native load (reused from the original run, whose feature
boundaries and shared-default edges this session did not change).

## Independent native-lifetime re-review (2026-10-14, #4258)

A fresh-context reviewer (strongest available model, read-only, nothing
executed) re-examined isolate/root drop ordering, callback panic boundaries,
foreground-task lifetime, pending transport cancellation, the shared-extraction
effects on the original host, the unsafe inventory and feature isolation. No
blocker was found. Every item below is a source concern unless a named test
covers it; none is a reproduced bug.

Fixed on this branch:

- `NativeHost::new_inner` registered the isolate's foreground queue before
  the bootstrap/bridge installs, whose early returns left the registry entry
  behind (harmless, since the `Weak` was dead and a later isolate at the same
  address overwrote the key). A guard now closes the entry on every early
  return and is disarmed once `NativeHost` owns the queue.
- The native loopback tests relied on reqwest enabling tokio `net`/`io-util`
  transitively; they are declared as dev-dependency features (lock unchanged).
- `accept.py` runs `cargo test -p zfb-js-host --features services` explicitly:
  the limits/dispatch-mode/shim pins moved out of zfb-render compile only with
  `services`, so a bare `cargo test -p zfb-js-host` runs none of them.

Recorded as phase-2 entry conditions (not triggerable by the phase-1 runners,
which are separate binaries):

- **One process initializer.** `bootstrap::initialize` owns V8 platform
  initialization behind its own `OnceLock`; rusty_v8 panics ("Invalid global
  state") if deno_core's `JsRuntime` initialized the platform first, or vice
  versa, and the per-thread host guard counts only `NativeHost`s, so a
  deno_core `OwnedIsolate` on the same thread is invisible to the LIFO rule.
  A combined CLI binary must route both hosts through one shared initializer
  and one thread guard before any host thread exists.
- **Platform protection mode follows the first host's thread.** Non-test
  builds initialize V8 with pkey-protected allocations from whichever thread
  first constructs a host; upstream requires every later V8 thread to descend
  from it. zfb's `ThreadedV8Host` spawns one thread per host, so the CLI must
  initialize V8 from the main thread before spawning, or select the
  unprotected mode deliberately and record why.

Known limits and parity notes retained without change:

- `registerErrorClass` is a no-op: host `TimeoutError`/`QuotaExceededError`/
  `NotSupportedError` are plain `Error`/`TypeError` instances with an own
  `name`, where the deno baseline makes them real registered classes. The
  polyfills and the paired corpora compare `.name` only, so `instanceof`,
  `.constructor` and the first `.stack` line differ unobserved. A contract
  divergence to settle in phase 2, not normalized away here.
- Re-executing the same module specifier with different source returns the
  cached module; the prototype runner never does this, and the baseline was
  not compared for it.
- JS cancel ids wrap at 2^30 and an older pending op's mapping is overwritten,
  exactly as the baseline registry comments; `forget` stays id-guarded.
- `poll_drive` parks on pending background work with no deadline and relies on
  V8's completion post (every exercised path posts); an unbounded wait would
  be a hang, not a wrong result. The Tokio time driver is a precondition that
  only `evaluate_script` checks.
- Unproven by any test: a panicking custom `Waker` (containment exists in the
  platform adapter) and `post_delayed_task`; cross-thread isolate address
  reuse is argued from V8's dispose semantics and the same-thread
  `repeated_pending_wasm_disposal_allows_fresh_foreground_work` test only.
- Six `unsafe` sites: five `callback_scope!` entries and one
  `repr(transparent)` pointer-to-`usize` transmute used only as a registry
  key; no `unsafe impl Send/Sync`; coordinates use checked arithmetic.

Properties the reviewer confirmed from source: drop ordering (`context`,
`modules`, `services`, `foreground` before `isolate`, thread guard last;
transports before resolvers; module `Global`s cleared with the isolate
alive), every V8-invoked Rust function wrapped in `catch_unwind`, no `RefCell`
borrow spanning a V8 call that can run JS, the stale-snapshot ordering in
`poll_drive`, exactly-once cancellation removal with the reqwest future dropped
before returning to JS, per-dispatch budget capture, byte-identical
`BuildTimeDenied` and transport/limits/digest/alias/Wasm-path code against
`main`, `DispatchMode` default unchanged, public re-exports intact, `services`
free of any V8 dependency and `direct-v8` enabled by no other manifest.

## Integration-gap continuation (2026-10-15, #4258)

The genuine Wind preview freshness failure at `1170596f` was reproduced:
`Stale wind preview asset: manifest.json; regenerate assets` (exit 1).
A manager-owned guarded `cargo build --locked -p zfb --no-default-features`
passed (guard PASS, 231 seconds; 29 existing no-default-feature warnings).
Cargo metadata selected `/home/takazudo/.cargo-target`, not repository-local
`target/`. The generator and subsequent `--check` used the absolute freshly
built compiler there and passed for all **173 samples**. The complete generated
diff changes only `manifest.json`'s compiler source digest (`4747bce8…` →
`d25f005a…`): CSS, HTML, inventory, sample definitions and diagnostic expectations
are unchanged. Generator tests passed **19/19**, and acceptance entry-point
refusal unit tests passed **10/10** using synthetic shell runners.

[The continuation receipt](evidence/integration-continuation-2026-10-15.json)
retains the original failure, compiler hash/build inputs and check results.
Original native receipts remain unchanged. The fresh Linux replay at
`34ffcba76fa54e88745656c609b2e5a178ae4227` is reused because runtime, Cargo,
harness, corpus and producer inputs have not changed; it is not native execution
at this continuation's final head. No new browser run is needed for the
byte-identical CSS/HTML; final-head ordinary Docs CI is reported separately.

The original current-head CI was refreshed, rather than inherited as green:
[Docs run 38583948009](https://github.com/Takazudo/zudo-front-builder/actions/runs/38583948009)
passed its build and root/subpath browser checks but failed freshness job
`115920879338` and therefore Docs gate. Independently,
[Health job 115920818230](https://github.com/Takazudo/zudo-front-builder/actions/runs/38583948103/job/115920818230)
failed the two Cargo-provenance assertions (root scripts 909 passed, 2 failed);
native build, native suites and size measurement were upstream-skipped, and
`island-size-linux-x64` was absent. No-V8 and all seven md-Wasm feature legs
passed; the dependent md-Wasm browser was skipped. These remain historical
results for `1170596f`; the draft PR records this continuation's final-head CI.

[ACTIVATION.md](ACTIVATION.md) now gives the non-circular infrastructure →
refresh → native collection → paired stage/review → authorized apply/final
verification route. Existing #4259 tooling revalidated the real declaration
and both producer closures (496→497 Linux; 480→481 Darwin), and fresh metadata
matched all four saved semantic row sets. That is lock/manifest/closure proof,
not native size evidence. Independent review confirmed the Wind-only delta and
source-bound replay reuse. Live contracts, ceilings, allowances, Cargo/npm locks,
package versions and runtime behavior are unchanged by this continuation.
Merges, manual native probes and paired activation remain separately gated.

The independent reachability repair is [draft #4260](https://github.com/Takazudo/zudo-front-builder/pull/4260),
based on refreshed main `3372fc29` after #4253 integrated the narrow showcase
draft guard. Its 205 synthetic orchestration/provenance/consumer tests across
seven suites pass; actionlint 1.7.12, scoped formatting and ignore parity pass.
Independent review corrected cancellation protection on the linked-watch step
and added a real-validator old-pin fixture before approval. These are not native
Linux/Darwin measurements. #4242 refreshes that actually integrated guard only;
#4234/#4259 remain unmerged. The first combined showcase test run was 7/8 because
the old branch test expected a different conjunction order; it now asserts the
exact integrated predicate, with all eight tests passing. No guard was removed.

A second concurrent main integration, `ce3c762b` (#4256), changed dev-server
asset routes and the broad compiler digest. The runtime branch incorporated it
to resolve the generated-manifest conflict, then rebuilt the genuine no-default
compiler (guard PASS, 10 seconds) and regenerated/checked all 173 samples again.
The final digest is `55172c4d…`; the complete generated delta remains manifest
provenance only. The receipt preserves both builds. Independent review confirmed
that server dev routes are not exercised by standalone host acceptance or the
production-build/private-wrapper generated gate; those paths, Cargo inputs and
harness/corpora remain unchanged. Linked CLI bytes and whole-workspace compiler
inputs are **not** claimed unchanged. Final ordinary CI covers the integrated
upstream dev-server change; no new candidate native replay is claimed.

## Lifecycle ownership continuation — 2026-10-14

The new replay in [lifecycle-linux-2026-10-14/acceptance.json](evidence/lifecycle-linux-2026-10-14/acceptance.json)
executed clean source `14e021f19d09287dbc3fb8a0ac86254379113092` after
refreshing integrated main `fbb7921d8fb3f28d859bbd4cf5737759f547866a`.
Subsequent receipt/documentation/generated-provenance commits did not run that replay. Older receipts
remain historical and were not overwritten. Actual session date is October 14;
the previous continuation's October 15 filename is retained unchanged.

The suspected ownership bug was reproduced: the success/drop and post-transfer
polyfill failure regressions both failed against `mem::forget(self)`. That call
forgot the registration's owned `Arc<ForegroundQueue>`, even though cleanup had
been correctly transferred to the host. An armed flag now disables registration
cleanup while allowing its fields to drop normally. Per-queue retained `Weak`
references prove eventual queue destruction after success, pre-transfer bootstrap
failure and post-transfer polyfill parse/runtime failure, across repeated good/bad
cycles and a fresh evaluation after each failure. These are queue-ownership
observations, not proof of an isolate, socket or process-memory leak.

Native coverage is now 45/45. Real V8 foreground tasks exercise delayed eligibility,
eventual host driving, queue closure and task disposal while the originating
isolate is alive. A retained synchronous Wasm module makes the subsequent async
compile take V8's native cache-hit foreground callback path. A panicking executor
Waker is observed and contained at that actual native callback boundary; the
promise subsequently completes. Registry-miss late posting is separately tested
through the Rust platform helper, not claimed as a second ABI test. Closed queues
retain neither late tasks nor a new driver Waker. Delayed eligibility uses a private
injected time; eventual completion does not claim that every run sleeps through
the driver's Pending/timer branch. Independent review found no remaining issues
in early returns, drop ordering, reference release, late posts or panic containment.

Original fixture attempts are preserved in
[original-observations.json](evidence/lifecycle-linux-2026-10-14/original-observations.json):
three 44-pass/1-fail native runs and a focused failure timed out waiting for a tiny
Wasm compile that the pinned streaming mode may finish inline. The final retained
module fixture follows the reviewed V8 cache path; the full suite then passed,
and the fresh acceptance independently passed it again. No watchdog became a
success and no failing case was removed. The initial Wind compiler build also
failed fetching pinned esbuild; retry used the independently obtained exact
0.25.12 executable. Genuine combined-source Wind regeneration/check covered all
173 samples, with 19 generator tests passing.

Fresh acceptance passed 9 shared-service and 45 native tests, 63 config and 27
service cases per engine, 47 module/worker cases per engine, 8 loopback HTTP rows,
7 outer-config rows, 8 generated packets per engine and 7 exact parity rows.
Separate explicitly built baseline/direct executables reported their expected
engine identities; before/after hashes matched. Both consumed the same frozen
snapshot `3486f80cd9c4ae216c7718cabea8f65637502e59f23d58a1d628e03a23451336`.
Chromium 151.0.7922.173 at `/usr/bin/chromium` observed `Count: 0` → `Count: 1`
and `rgb(12, 34, 56)` without page/console errors. Ten harness-refusal tests passed.
The 296 affected baseline Rust regressions also passed. Full commands, tool versions,
feature selections, runner hashes and browser observations are in the new receipts.

The standalone selected host graph has 152 package/feature rows, pinned V8
147.4.0 and no Deno/render/build dependencies. Separately, held #4259 tooling at
`714e65cd01ab674d668aac7db1a3a5a31c03e9e7` revalidated the immutable old source
`6fb3fbe0103c5c90799c3262ca5c1ab9ade0f27e` against this source: Linux 496→497
and Darwin 480→481 semantic rows, real lock bytes, declaration and manifests.
That metadata-only proof neither stages nor validates a native candidate artifact.
Its workspace-wide/default-producer closure cannot prove a future feature-exact
candidate CLI graph; [ACTIVATION.md](ACTIVATION.md) now distinguishes all three.

Infrastructure [#4260](https://github.com/Takazudo/zudo-front-builder/pull/4260)
artifact 11913810224 was downloaded and verified, with all 98 extracted files.
The measured checkout is `9ffc1ebcdcb4e6159e9c4c49d755bc3b1f9fa8f8`, distinct
from probe head `572b174f70664e2fe4d91c552439eb3d137f848a`. Exact-source
`assertCompletePair` recomputed saved bytes, complete workspace/packed matrices,
normalized sidecars and unchanged totals successfully with original Node 24.14.0
and zlib 1.3.1-e00f703. The initial Node 24.19 replay correctly refused its different
normalizer identity. The original successful job log independently records full
budget enforcement and artifact upload. Local content replay does not reconstruct
omitted producer tarballs/binaries. This is infrastructure/default-producer 4.5.0
evidence only, not #4242 adoption or Linux/Darwin candidate size evidence.

The scoped root tests retain two real failures (62 passed/2 failed): Linux and
Darwin reviewed-contract assertions reject local/workspace/packed Cargo lock
provenance. They are not waived. Main's legitimate 4.5.1 package/pnpm changes and
Linux predecessor contract were retained; this task adds no lock, package-version,
contract, ceiling, allowance, runtime-selection or dependency-family change. The
merge hook unexpectedly formatted an excluded raw Markdown fixture; the follow-up
restores exact upstream bytes using only the existing formatter exclusion. No hook
configuration changed. Startup/access and retry details are in
[session.json](evidence/lifecycle-linux-2026-10-14/session.json).

The implementation batch does not establish phase-one integration acceptance or
actual CLI adoption. Remaining ordering is #4234 → #4259, plus #4260 integration,
then refreshed #4242 and compatible Linux/Darwin native size evidence, paired
stage/review, separately authorized apply, and final enforced verification. Future
shared V8 process/thread ownership, a nondefault typed-config CLI path, render
factory/thread adapter and dev start-before-swap rollback/bounded shutdown remain
handoff work. No held-stack or PR merge, native dispatch, activation, preview or deployment occurred.

Broad verification passed default workspace/all-target build, no-V8 check, workspace
Clippy, formatting, all seven md-Wasm graph selections, actual wasm32 cdylib link,
and CSS addon build/load. Initial native Clippy failed `items_after_test_module`;
the unchanged test module moved after production items in `14e021f1`. Independent
byte comparison confirmed ordering only. Native Clippy then passed and the complete
acceptance replay passed again at that clean corrected checkpoint. The initial
`a15e9a26` replay and failed lint remain in the receipt. Final Wind freshness
correctly rejected the changed broad source digest; genuine regeneration using
the final acceptance-built compiler and a new check passed all 173 samples.
Only manifest provenance changed. Broader results remain attributed to their
recorded source rather than being relabeled as reruns.

The final Wind manifest delta is exactly `compiler.sourceSha256`
`84c52815…` → `a621c16c…`; all generated rows and asset bytes are unchanged.
That provenance-only refresh does not alter the tested host, corpora, build
producer, harness or frozen acceptance snapshot. The final commit records tested
predecessor `14e021f1`; it does not claim a replay of its own hash.

## Checker protocol continuation (2026-10-15)

The attachment was recovered through the parent's exact text handoff after
Library downloads failed in this environment. Decoding transport XML entities
reproduced exactly 43,482 bytes and SHA-256
`c7a5dc3b570b7c74892a60553d9a147d0e95278191fe38d4faccbaa1e22b6f12`.
It applied cleanly over `656a1b42`, whose only changes from `8bddd9ea` were
baseline evidence. Earlier failed transfers and baseline receipts remain intact;
their patch-blocked status is historical, superseded by this verified recovery.

The shared config/outer receipt parser now enforces exact framed counts, actual
boolean statuses, present values, string errors and strict JSON. Malformed slots
retain their positions, LF alone frames receipts, invalid UTF-8 retains raw
bytes, and failed baseline execution still permits the candidate observation.
Loaded corpus bytes and pre/post executable identity are checked. Independent
review confirmed compatibility with the stricter `accept.py` aggregation.

The unchanged baseline suite passed 64 tests; the exact attachment passed the
combined 92-test suite. Review then reproduced a pre-existing evidence-loss edge:
1,100-level JSON could parse but fail report serialization with `RecursionError`.
Three new regressions initially failed (two failures, one error). A small
iterative 128-level nesting limit and normalized decoder recursion failure now
preserve rejected slots, raw output and subsequent observations, and refuse deep
corpora before launch. Independent review approved the repair; all 95 combined
tests, 14 Python syntax checks, formatting and diff checks passed.

Fresh native acceptance passed at clean committed checkpoint
`cc56b15a38173b9a276cb997ef12230fdc370cd0`: 45 native and 9 shared-service tests;
63 config, 27 service and 47 module/worker cases per engine; 8 HTTP, 7 outer-config,
8 generated packets per engine and 7 exact parity rows. No corpus was reduced.
The patched checkers' real reports passed the actual aggregation path, including
matching pre/post corpus and executable digests with no protocol failures.

The producer generated a new frozen snapshot, manifest SHA-256
`f8ca3168c0a53badcf308677bc04f1d3686e1af3a4dbffe9d152038a22a150f2`.
Both engines consumed identical input bytes; all 54 frozen files were rehashed.
The authored throw matched `throw.ts:6:9` and its exact source content.
Chromium 154.0.8037.57 observed `Count: 0` → `Count: 1` and expected CSS
`rgb(12, 34, 56)` without page/console errors. All six executable digests and
execute permissions remained valid. V8 147.4.0, esbuild 0.25.12 and existing
dependency pins were preserved. This reused the warm target, not a cold benchmark.

New receipts and the independent review are under
[checker-linux-2026-10-15-cloud](evidence/checker-linux-2026-10-15-cloud/README.md).
The later evidence-only commit is distinct from measured source `cc56b15a`;
runtime, harness, corpus, producer, locks and toolchain inputs are unchanged.
Rust/runtime inputs and all six executable bytes also match the earlier baseline,
so the same-session runtime-risk review remains applicable. No new runtime defect
was observed. No separate workspace/default/no-V8, Clippy, md-Wasm or CSS-addon
batch was rerun for this Python-only change; older results retain their source.

#4234, #4259 and #4260 remain open drafts and unmerged. Cargo-provenance failures,
compatible paired Linux/Darwin native size evidence and integration/activation
remain unresolved. No held branch was imported, merged or modified. No phase-two
CLI adoption, live operation, dispatch, issue closure or Rolldown change occurred.
The non-UTF-8 page postprocessing limitation remains separate from Worker binary
responses. Phase-one integration acceptance is not complete.

## Combined-source replay and durable receipts (2026-10-16)

Accepted main `151d9d97` was reconciled into clean source
`546fce7cd3efccb1a9f05e8a51a9d889a545c256`. The actual combined-source compiler
regenerated and checked all 173 Wind samples.

[The source-bound evidence bundle](evidence/refresh-linux-2026-10-16/README.md)
records the full successful generated acceptance replay with Chromium, 296
Deno-backed renderer tests, the no-V8 check, 117 Wind tests, 52 MDX tests, 22 CSS
tests, 95 strict Python tests, 19 Wind generator tests and 22 reference tests.
The original failed attempts remain recorded separately from successful retries.
Independent review validated source reconciliation, executable hashes, frozen
inputs, strict reports, parity, mapped errors and browser observations.

Default-producer metadata, standalone Deno-free host metadata and future actual
CLI feature-exact proof remain separate. The last remains unimplemented. Two
inherited provenance-budget assertions remain failing; this lane changes no
thresholds or health/re-pin scripts. The current activation section retains the
ordered integration, both-platform evidence, paired review and separately
authorized live-apply gates. No live contract or runtime default changed.
