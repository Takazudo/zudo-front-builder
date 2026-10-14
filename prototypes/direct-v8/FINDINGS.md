# Phase 1 execution findings

This is a running evidence ledger for #4235–#4240, not a completion claim.
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
passed **55/55**: 25 success values and 30 errors. Candidate results are pending.
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
- Native config (#4237), async/cancellation (#4238), module/Worker (#4239),
  generated bundles, source maps, graph isolation and bounded hydration (#4240)
  are **pending**, not passed by the extraction tests or baseline CI.
- Darwin arm64/x64, Linux arm64 and Windows x64 candidate execution are not run.
  Existing repository CI platforms do not imply candidate platform evidence.

## Native config gate work in progress (#4237)

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
and recovery after errors are verified. Async native work is the next gate.

The first real candidate link passed under the guard in 9 seconds:
`cargo build --locked -p zfb-js-host --no-default-features --features direct-v8 --example direct_v8`.
Both independent runners passed the original 55 fixed config expectations.
The selected `cargo tree --edges normal,build` closure contains no Deno runtime
crate or `serde_v8`; full resolved-graph receipt is still pending final features.

Additional probes found real failures outside that initial corpus:

- `Intl.DateTimeFormat` fatally trapped without ICU data (exit -5); baseline
  returned the fixed date/number/locale results. This is a bootstrap failure,
  not an accepted omission. `prepare-icu.py` now prepares the exact baseline
  ICU 77 bytes from the checksum-pinned data archive. No Deno Rust code or
  runtime dependency is linked for this data input. Data SHA-256:
  `1cf67874b5a87a8363a86fb3f81e3cbbed54d389062dab8fb52308d5cf8c8612`.
  It is local/ignored under `target/direct-v8-inputs`; the candidate build uses
  the explicit `ZFB_V8_ICU_DATA` path. A corrected run is pending.
- Independent review found a chained dynamic self-import was not fully
  drained. Actual paired probe returned `{done:true}` in baseline and
  `{done:false}` in candidate. Correction and regression are pending.
- Review found import.meta.resolve coercion/arity divergence; correction and
  fixed probes are pending.
- Both hosts correctly rejected an uncaught queued callback and an unhandled
  promise rejection, preserving authored message/location. Both rejected an
  unresolved top-level await promptly; no watchdog kill was necessary. Candidate
  currently states the unsupported no-pending-work condition explicitly.
