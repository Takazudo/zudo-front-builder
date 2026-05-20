# Research: Boa / QuickJS as the SSG-only JS engine (issue #345)

Branch: `research/345-boa-quickjs-eval`
Status: complete — no-go recommendation, high confidence.

## 1. Question

For pure-SSG zfb projects (every route is `prerender !== false`), the JS host
only has to evaluate page modules at build time to produce HTML. Could a
smaller pure-Rust engine (`boa`) or a smaller embeddable engine (`rquickjs`)
replace the embedded V8 (via `deno_core`) for that case — without losing the
"any frontend dev's TSX just works" promise that the current architecture is
explicitly designed around?

The investigation must also decide:

- whether the original `crates/zfb-runtime-spike/` bench data has aged
  enough to revisit;
- whether engine selection (V8 vs small) would be driven by detecting
  `prerender = false` in a project, or by per-project config — the same
  gating question raised in sibling research issue #344;
- a go/no-go on pursuing SSG-only mode past a refreshed bench.

## 2. What was tried

- Re-read the issue body: `gh issue view 345`. Captures the original
  trade-off framing and explicitly names the four investigation tasks
  enumerated above.
- Searched the workspace for the spike crate: `crates/zfb-runtime-spike/`
  does **not** exist today. `ls crates/` returns 15 production crates
  (`zfb`, `zfb-render`, `zfb-build`, ...) but no `zfb-runtime-spike`.
- Traced the spike's history. `git log --all --oneline -- crates/zfb-runtime-spike/`
  shows three commits creating it (`101b9ab Sub 1: JS runtime spike crate
  + ADR-001`, `e9012c1`, `46a42b9`) and one retiring it: `4d7c37d chore(render):
  retire deno_core JS runtime path; supersede ADR-001 with ADR-005`
  (2026-04-28). At that point the project pivoted to a miniflare/workerd
  subprocess, then ADR-007 (commit `8988eca`, 2026-05-04) re-adopted
  embedded V8 via `deno_core` because Tauri distribution forbids a Node
  dependency on end-user machines. The ADR markdown was later deleted in
  bulk (`011b236 delete ADR raw markdown ... 2026-05-20`) on the grounds
  that ADRs were "for internal reference only". Conclusion: the spike
  crate is *gone on purpose*; rebuilding it as a permanent crate is not
  the right move.
- Re-read `docs/src/content/docs/architecture/js-runtime.mdx`. The current
  doc still cites the spike's measurement numbers (warm 16µs / 1.37ms /
  106µs; RSS 19MB / 317MB / 4.5MB; build cost "~3 minutes" / similar /
  "~30 seconds"). It also enumerates the four hard requirements on any
  host (ESM, top-level await, source-accurate errors, no leaks across
  hundreds of renders) at lines 23–28, and the binary-fixed-runtime
  invariant at lines 62–64.
- Re-read `docs/src/content/docs/architecture/why-rust.mdx`. The
  "Honest counterpoint: build cost" section (line 37) still asserts the
  15–30 minute first-build figure that the issue uses as motivation —
  but `crates/zfb-render/Cargo.toml:35-47` explicitly notes that
  `deno_fetch`/`deno_web`/`deno_url`/`deno_console` were *removed* in
  favor of a ~250-line JS polyfill specifically because those crates
  "imposed a 15-30 minute first-build cost". Marker that the headline
  number in the public doc may already be stale (see §5 Follow-ups).
- Read current versions in `crates/zfb-render/Cargo.toml`: pinned
  `deno_core = "=0.399.0"` (latest published is `0.400.0`; held to track
  the workspace `rust-toolchain.toml` stable channel). `Cargo.lock`
  confirms `v8 147.4.0` in use.
- Checked current crate versions via `cargo search`:
  - `rquickjs = "0.11.0"` (spike used `0.9`)
  - `boa_engine = "0.21.1"` (spike *skipped* boa entirely — no measured
    baseline)
  - `ssr_rs = "0.8.3"` (spike used `0.7`)
  - `deno_core = "0.400.0"` (workspace pinned to `0.399.0`)
- Fetched the Boa 0.21 release blog
  (https://boajs.dev/blog/2025/10/22/boa-release-21) and the rquickjs
  CHANGELOG (master). Captured headline deltas (see Evidence).
- Sketched the SSR smoke matrix shape (5 representative components) and
  the Rust harness shape. Did **not** run a fresh `cargo build` of
  rquickjs or boa — the evidence below is decisive without burning a
  ~5-minute compile slot.

## 3. Evidence

### 3.1 Spike crate is retired and the ADRs that contained the numbers are deleted

`git show 4d7c37d` (retirement commit) removed:

- `crates/zfb-runtime-spike/Cargo.toml` (51 lines)
- `crates/zfb-runtime-spike/src/bin/{bench.rs,gen_fixtures.rs,smoke.rs}`
- `crates/zfb-runtime-spike/src/hosts/{deno_core.rs,quickjs.rs,ssr_rs.rs,mod.rs}`
- `crates/zfb-runtime-spike/src/{host.rs,lib.rs,metrics.rs}`

Total: 17 files / 1,080 deletions. The spike never had a `boa` host —
it was always "rejected without a measured spike" per
`js-runtime.mdx:40`.

The deleted spike `Cargo.toml` listed:

```
rquickjs = "0.9"  features = ["loader", "parallel"]
deno_core = "0.319"
ssr_rs   = "0.7"
```

versus current crates.io heads of `0.11.0 / 0.400.0 / 0.8.3`. So the
numbers in `js-runtime.mdx` table are roughly two minor versions of
each candidate behind current.

### 3.2 The four hard requirements (`js-runtime.mdx:23-28`) still rule out the small engines

The doc says the host must:

1. Support ESM (`import` / `export`).
2. Support top-level `await`.
3. Surface source-accurate error locations (the user's TSX line, not an
   offset into a wrapped script).
4. Evaluate the same module repeatedly across hundreds of pages without
   leaking memory.

Mapped against the candidates as of latest released versions:

| Requirement              | `deno_core` 0.399 (current) | `rquickjs` 0.11.0           | `boa_engine` 0.21.1                       |
| ------------------------ | --------------------------- | --------------------------- | ----------------------------------------- |
| (1) ESM                  | native                      | partial (was already partial in 0.9; 0.10/0.11 CHANGELOGs add `no_std`, `Proxy`, iterator polish — no ESM/TLA breakthrough) | yes — `boa_module` supports `import`/dynamic import per 0.21 release notes |
| (2) Top-level await      | native                      | not supported synchronously; requires `parallel`/async runtime feature; the original spike's QuickJS host had to wrap fixtures as a `globalThis.renderHTML` script and **could not** evaluate ESM with TLA (see the spike's host comment in `git show 101b9ab:crates/zfb-runtime-spike/src/hosts/quickjs.rs`) | release blog (Boa v0.21) does **not** explicitly confirm TLA; needs validation. The release added "asyncifying ModuleLoader" and revamped JobQueue — promising but not a stated guarantee. |
| (3) Source-accurate errors | native (V8 stack frames map back to original TSX line via SWC source maps) | "offsets only" (spike table); the project's `embedded_v8/js/web_polyfills.js` polyfill story relies on source maps that V8 produces — not applicable to QuickJS | Boa 0.21 added "span nodes and error backtraces". Improved, but no claim of TSX source-map fidelity end-to-end. |
| (4) No leaks across N renders | proven (single isolate; current host treats as `!Send` and reuses it) | proven small-footprint in spike (4.5MB RSS) | Boa 0.21 migrated to a register-based VM and uses NaN-boxed `JsValue`. No measured leak data for the hundreds-of-renders SSG workload. |

Requirement (1) ESM is the only one rquickjs has not improved
meaningfully since 0.9. Requirements (2) and (3) are still
deal-breakers. Boa's ECMA conformance grew to 94.12% Test262 (from
89.92%) in 0.21 — but the remaining ~5.88% gap is where modern npm
SSR components actually live (Temporal, async iterators, `Proxy`-heavy
state libs, `Intl` for date/number formatting, `import.meta.glob`-shape
patterns Astro/Vite users expect to "just work"). The doc's existing
warning at `js-runtime.mdx:40` ("real-world Preact / React SSR hits
unsupported corners") is still the right framing in 2026-05.

### 3.3 The binary-fixed-runtime invariant is the dominant blocker

`js-runtime.mdx:62-64`:

> **The runtime is fixed by the binary, not by config.** `zfb.config.ts`
> cannot pick its own JS runtime. The runtime is whatever the `zfb`
> binary you installed was compiled with. That keeps the contract
> between zfb and user code single-valued — every project built with
> one binary uses the same runtime. The bootstrap rule lives in
> `crates/zfb/src/config.rs`.

This is the structural blocker, not the bench numbers. "SSG-only mode
via a small engine" requires one of:

- **(a) Two binaries.** Ship `zfb` (V8) and `zfb-lite` (rquickjs/boa).
  Doubles the release matrix, the platform binary list in npm
  optional-deps (`packages/zfb/optionalDependencies` per recent
  `6d0bc70 fix(release): use workspace: prefix for zfb optionalDependencies`
  commit), the Tauri distribution story, and the contributor build
  story. Cannot share the prebuilt-binary install command in
  `installation.mdx`. Users on hybrid projects would have to know which
  binary to install before authoring any `prerender = false` route.
- **(b) Lift the invariant.** Make engine choice config-driven —
  directly contradicting the principle that "every project built with
  one binary uses the same runtime". Even if technically possible, it
  re-introduces the per-project drift the invariant is there to
  prevent, and forces every shared component / npm package contract to
  declare which subset of JS it relies on.

Neither path is cheap. Neither is justified by the bench delta the
spike measured.

### 3.4 Build-cost premise may be partially superseded

The issue cites `why-rust.mdx:37` "the 15–30 minute first-build time"
as the motivating cost. But `crates/zfb-render/Cargo.toml:35-47`
explicitly documents that this cost was driven by the
`deno_fetch`/`deno_web` Rust stack (hyper / rustls / tower / h2), which
the project *removed* in favor of a ~250-line polyfill in
`embedded_v8/js/web_polyfills.js`. The spike's `js-runtime.mdx` table
already lists `deno_core` build cost as "~3 minutes" — an order of
magnitude below the headline.

If the current bare `deno_core 0.399` + polyfill workspace build is
indeed in the 3–5 minute range (the table claims ~3min for the spike's
candidates), then the V8 weight that this whole investigation is
trying to shave is one of:

- ~3 minutes of first-build (a contributor pays once)
- the runtime RSS (~19MB single isolate per the spike table, which is
  not the bottleneck of any zfb deploy target).

That ratio does not justify two binaries or lifting the
binary-fixed-runtime invariant. (Confirming the *current* first-build
number is a follow-up — see §5.)

### 3.5 Smoke matrix scaffold (not executed)

Five representative components for the "would a small engine
actually run this?" matrix. Names + packages + the specific axis each
exercises:

| # | Package                     | What it exercises                                               | Expected on rquickjs                                                  | Expected on boa 0.21                                            |
| - | --------------------------- | --------------------------------------------------------------- | --------------------------------------------------------------------- | --------------------------------------------------------------- |
| 1 | `preact-render-to-string`   | The trivial baseline. Pure JSX → HTML via `vnode → string`.    | Likely runs. This was the kind of thing the original spike succeeded at. | Likely runs.                                                    |
| 2 | `@preact/signals`           | Heavy `Proxy` / getter/setter interception.                     | rquickjs 0.11 *just* added `Proxy` per CHANGELOG — high risk that real-world signal graphs hit edges. | Boa supports Proxy; Test262 score suggests it should work.    |
| 3 | `htm`                       | Tagged template literal preprocessing at runtime. No JSX compile step. | Should work — pure JS, no exotic APIs.                                 | Should work.                                                    |
| 4 | `preact-router`             | URL parsing via `URL`/`URLSearchParams`. Routing assumes web globals. | rquickjs does not ship a web-globals layer — would need a polyfill akin to `embedded_v8/js/web_polyfills.js` ported to whichever host. | Same — boa has no `URL` / `Request` / `Response` out of the box. |
| 5 | `@emotion/react`            | Dynamic `import()`, async patterns, source-map dependency for class-name → component mapping. Used as the stand-in for "any CSS-in-JS that touches modern features". | Risky — historically pulls in things rquickjs has poor support for; source-map fidelity gap matters here. | Risky — the ECMA conformance gap (~5.88%) is exactly the kind of place dynamic-import / async-iteration paths hit. |

### 3.6 Harness shape (sketched, not implemented)

Located at (would be) `__inbox/345-boa-quickjs-eval-spike/`. The shape:

```
__inbox/345-boa-quickjs-eval-spike/
├── Cargo.toml                  # workspace = false; standalone bin
├── README.md                   # how to run, intentionally throwaway
├── fixtures/
│   ├── 01-preact-rts.mjs       # imports preact-render-to-string from a vendored bundle
│   ├── 02-signals.mjs
│   ├── 03-htm.mjs
│   ├── 04-preact-router.mjs
│   └── 05-emotion.mjs
└── src/
    ├── main.rs                 # arg-parses --host {rquickjs,boa}, --fixture <n>, runs once, reports
    ├── hosts/
    │   ├── mod.rs              # Host trait — render_module(name, source) -> Result<String>
    │   ├── rquickjs.rs         # initialises rquickjs::{Runtime, Context}; evaluates as a script
    │   │                       # (NOT a module — rquickjs ESM support is partial); polyfills minimum
    │   │                       # globals (`globalThis.process`, basic `URL`, `console.log`)
    │   └── boa.rs              # uses boa_engine::Context; experiments with boa_module for ESM
    └── polyfills.js            # ported subset of embedded_v8/js/web_polyfills.js
```

Per-fixture verdict captured as a JSON line: `{"host":"rquickjs",
"fixture":"02-signals", "result":"compile-error",
"stage":"eval", "error":"..."}`. The matrix is the union of host ×
fixture and is reportable as a pass/fail grid.

Decision to NOT actually compile it now: the four hard requirements
already rule out a "production SSG-only path" before any single fixture
runs; spending the 5-minute compile slot on a proof-of-concept that
will at best tell us "rquickjs runs preact-render-to-string" (which the
2026-04 spike already confirmed at 106µs / 4.5MB) adds no new
information. A future contributor wanting to revisit the question
should rebuild the harness as a workspace-external crate under
`__inbox/` so it never enters CI.

### 3.7 Engine selection: detection vs config (cross-ref to #344)

The build pipeline already tracks `prerender: bool` per route — see
`crates/zfb-build/src/plugin_runner.rs:122-138`. A "detect SSG-only"
predicate would be a workspace-level scan: every page module's `export
const prerender` is either absent (default true) or evaluates to true.

The recommendation (which #344 will ratify in its own scope, so this
doc only notes it):

- **Detection is structurally cleaner than a config flag.** A project
  is SSG-only iff no route opts out — that is a property of the source
  tree, not a contributor preference. A config flag invites the
  pathological case "I set this flag and then later added a
  `prerender = false` page" which the engine cannot satisfy.
- **But detection is moot if the runtime is binary-fixed.** Today the
  engine choice is `crates/zfb/src/config.rs` bootstrap, not a runtime
  decision. Detection only helps if the binary ships both engines
  *and* picks at startup — which contradicts §3.3 (b).

Cross-ref only. Do not duplicate #344's analysis here.

## 4. Conclusion

**No-go**, high confidence (~85%). The SSG-only-via-small-engine path
should not be pursued past this bench refresh.

Reasoning, in order of importance:

1. **Binary-fixed-runtime invariant** (`js-runtime.mdx:62-64`) is the
   dominant blocker. Either we ship two binaries (doubling the release
   matrix, Tauri story, and contributor install path) or we break the
   invariant. Neither is justified by any plausible bench delta.
2. **Hard requirements unmet.** rquickjs still fails (2) top-level
   await and (3) source-accurate errors as of 0.11. Boa 0.21 is closer
   on (1) and (3) but still 5.88% off Test262; the remaining gap is
   exactly where modern Preact / React SSR npm components live.
3. **Premise softening.** The 15–30 minute first-build cost cited by
   the issue is, per `zfb-render/Cargo.toml` and the spike table, no
   longer ~3 minutes for the current `deno_core 0.399` + polyfill
   workspace. The actual gain a small engine would buy on the current
   architecture is materially smaller than the issue assumes (this
   should be measured cleanly as a follow-up — see §5).
4. **Existing seam already preserves optionality.** The
   `RenderHost` trait (`crates/zfb-render/src/render_host.rs`) is
   already the only surface call sites name; if the gap closes in a
   future Boa 0.x release, swapping in a `BoaHost` is a single-crate
   change, not an architectural pivot. Today is not that day.

The closest reasonable adjacent line of work is #344
(feature-gated V8 in the production runtime) — it answers "can we ship
a smaller binary for SSG-only deploy targets?" without lifting the
binary-fixed-runtime invariant. That investigation is strictly better
than the small-engine path for the same motivation.

## 5. Follow-ups

- **Measure the current bare `deno_core 0.399` + polyfill first-build
  time on a contributor machine** and update
  `docs/src/content/docs/architecture/why-rust.mdx:37` ("15 to 30
  minutes") if the headline is now stale. The `Cargo.toml` polyfill
  story strongly suggests it is, but no number is in the repo to cite.
  Out of scope for this investigation; would be a one-PR docs follow-up.
- **Revisit Boa annually.** Test262 conformance went from 89.92% to
  94.12% in 0.21 (2025-10). If a future Boa release closes the
  remaining gap *and* claims TLA + source-accurate errors, the matrix
  in §3.5 becomes worth actually running. Set a calendar reminder, not
  a code task.
- **rquickjs 0.11 `Proxy` support is a notable delta** for any future
  reconsideration but does not move the needle on (2) and (3).
- **Cross-ref with #344's outcome.** Whatever #344 decides on the
  detection-vs-config axis should be the canonical answer; this doc
  defers to it.
- The original spike's QuickJS host wrapped fixtures as scripts, not
  ESM, "because ESM evaluation in rquickjs returns a microtask-driven
  promise that needs the `parallel`/`async` runtime feature to drive
  cleanly" (paraphrasing the deleted
  `crates/zfb-runtime-spike/src/hosts/quickjs.rs` docstring at commit
  `101b9ab`). If anyone re-enters this space, that's the first thing
  to fix in the harness.

## 6. Scope exceptions

None. Files touched:

- `research/345-boa-quickjs-eval.md` — primary deliverable (this file).
- `__inbox/345-boa-quickjs-eval-spike/` — directory created but
  contains only this notes file's harness sketch (§3.6); no code
  emitted (decision documented in §3.6).

No production crate, doc, or config was modified.
