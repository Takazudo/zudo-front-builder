# Phase 2 opt-in CLI integration

Tracking: #4301. Base: `21d6807e442bbed61e578c165a91d64c4a7dc141`.

> Status: integrated into `main` by #4368 (`47acd2a8`). The feature stays non-default
> and no release includes it. The integration-lane notes below are kept as written.

This work follows `CLAUDE.md#development-strategy-integrate-first-use-ci-to-find-regressions`.
The integration branch targets main; this lane owns shared files, the combined
native verification queue, CI diagnosis and final evidence reconciliation.
The reviewed error-identity lane is consumed before final acceptance. No merge,
default switch, release, Mac task or workflow dispatch is part of this work.

## Build selection

Prepare the existing checksum-pinned ICU input with `prepare-icu.py`, then run
these commands through the host's published heavy guard:

```sh
export ZFB_V8_ICU_DATA="$PWD/target/direct-v8-inputs/icu77/icudtl.dat"
cargo build --locked -p zfb --no-default-features --features direct-v8 --bin zfb
cargo test --locked -p zfb --no-default-features --features direct-v8 --lib v8_host_adapter
cargo test --locked -p zfb-render --no-default-features --features direct-v8,syntect-onig --test config_eval
```

There is no runtime config field or CLI switch. `embed_v8` remains the default
Deno implementation. When both features unify, the existing Deno render/config
facade wins; direct-only commands must explicitly disable defaults. Slim builds
with neither feature retain their existing Node config evaluator and no-V8
render behavior. Markdown/Wasm and CSS-addon feature choices remain unchanged.

`record-graph.py --cli` checks the actual CLI's normal/build closure, including
its render/build crates, against the Deno exclusion and pinned V8 requirement.
The standalone graph remains a separate check. Both record exact selections.

## Process and lifetime contract

The CLI initializes V8 on its main thread before creating Tokio or host threads.
Both host implementations share `zfb-js-host::process` initialization and the
one-live-isolate-per-thread guard. Config evaluation creates a fresh host per
call. Render workers retain the existing dedicated-thread adapter and owned
request/response DTOs. No unsafe Send/Sync implementation is introduced.

A combined-feature process uses one custom foreground platform. Deno receives
its tasks through its own main-context task spawner; native hosts use the
existing wake-driven queue. Queue registration is closed before isolate disposal.
ZFB's Deno extensions remain ops-only during construction: authored JavaScript
starts after queue registration. The shared-engine example exercises both
initialization orders in separate executions and concurrent finite work:

```sh
cargo build --locked -p zfb-render --features direct-v8 --example direct_v8_shared
target/debug/examples/direct_v8_shared native-first
target/debug/examples/direct_v8_shared deno-first
```

Dev refresh retains its existing private-candidate boot, route validation,
publication and old-host retirement sequence. A failed candidate keeps the last
good host. Fresh hosts deliberately give changed source at the same specifier a
new module cache. Build/prerender/lazy SSG/content tracing remain build-time;
only explicit SSR requests receive request-time mode.

Finite completion tests do not prove interruption of CPU-bound or unresolved
JavaScript. The existing bounded join can detach on timeout; such an outcome is
not clean shutdown. #4289 remains the separate hard-hang/drain policy owner.

## Evidence status

Implementation and verification are recorded separately. Phase-1 receipts in
README, CONTRACT, FINDINGS and ACTIVATION remain historical, source-bound
observations. Their old CLI-integration holds do not describe the new code,
but they are not retroactively upgraded to phase-2 verification.

Linux x86_64 verification (2026-10-18; Rust 1.99.1) uses the exact
`--no-default-features --features direct-v8` CLI selection. The reviewed
error-identity commit `5162d753b1b771749a75c227ad91e653ac3045cd` is included as
`322ad117`. Implementation is `733364ee`; subsequent changes strengthen tests,
engine receipts and documentation provenance.

| Gate | Linux result |
| --- | --- |
| Exact native CLI build and normal/build graph | Pass; V8 147.4.0, no Deno or serde_v8 |
| Native host library, including reviewed error identity | 49 passed |
| Threaded adapter: console, request modes, replacement, finite exit | 16 passed |
| Config evaluator / typed CLI config diagnostics | 14 / 273 passed |
| Build/dev request-time, content reload, supervision, Wasm E2E | 16 passed |
| Explicit ignored SSR self-heal tests | 2 passed |
| Actual CLI build/dev continuity and recovery | 6 rows pass; finite 3s candidate boot, every sampled request under 1s; wrong hash rejected |
| Outer typed config observer | 7/7 native and 7/7 default Deno |
| Combined-engine foreground/ownership probe | Both fresh-process orders pass, plus concurrent hosts |
| Default Deno render / adapter regression controls | 210 / 16 passed |
| Slim `cargo check -p zfb --no-default-features` | Pass (existing warnings retained) |
| Checker protocol and negative controls | 105 passed |
| Frozen config / services / modules parity | 63 / 27 / 47 per engine |
| Live transport protocol | 8 rows pass, including build/raw/forged denial with zero requests |
| Native-CLI-produced frozen real inputs | 54 frozen files; generated gate passes |
| Real installed Chromium | Hydration Count 0→1, expected CSS, no page/console errors |
| Wind reconciliation | 99 unchanged non-lock inputs, 164 unchanged reachable lock records |
| Wind reference / preview tests | 22 / 19 passed; 173 previews fresh, CSS unchanged |

The frozen generated gate covers canonical build/dev-wrapper output, plugin and
binary dispatch, mapped throws and finite recreate/drop. Its historical
limitations remain valid for that checker alone; actual CLI reload is covered
separately by `check-cli-lifecycle.py`. Full local receipts, frozen input bytes
and executable copies remain ignored under `target/direct-v8-cli-final-evidence`. The compact result and binary hashes are
in [CLI-LINUX-VERIFICATION.json](CLI-LINUX-VERIFICATION.json).

First failures were retained rather than silently called passes: missing installed
Hono input, esbuild download failure (then the existing checksum-verified 0.25.12
local override), standalone config-test Syntect selection (corrected to explicit
`syntect-onig`), and CI's untyped nonce-test channel (fixed to `channel::<()>`).
CI also found stale Wind fingerprint/provenance after Cargo/source changes;
those were regenerated without changing support pins or CSS bytes.

The island-size health gate deliberately remains blocked: the changed lockfile
SHA is not admitted by the approved Darwin/Linux contracts. Its assertion
requires fresh platform-native evidence and reviewed rebaseline. No budget pin,
ceiling, allowance, or gate has been changed to pass this lane. No Mac validation
was launched here. Final draft/CI status belongs to PR #4307; a green native
subset is not a green all-platform release gate. Native Linux observations do not prove Mac
or other platform behavior. The independently reported Mac main-baseline red
suite (#4294) remains separate and is not attributed to this lane.

The final rebuild differed from the earlier cached binary, so both were retained
and all frozen producer acceptance was rerun against final CLI SHA-256
`f46bbac741afdd3de2eeaaf34c4611af7555bac6d7a02266d0553a3aa8bf69e6`. A hash mismatch was never counted as a pass.
