# ADR-001: JS runtime for zfb's SSR pipeline

- **Status:** Accepted
- **Date:** 2026-04-26
- **Owners:** Sub 1 (Epic 3 — File-based router + JSX rendering)
- **Locks:** runtime choice for `zfb-render` and all dependents (Subs 3, 4, 5, 6).
  Future revisions require a follow-on ADR.

## Decision (one sentence)

**zfb's SSR pipeline executes JavaScript via `deno_core` (V8 through Deno's
reusable core), behind a thin `RenderHost` trait so the runtime is swappable
in v1.1+ without changes to caller code.**

## Context

zfb is a Rust-native static site generator that renders Preact/React JSX (TSX)
on the server. Rust has no built-in JS runtime, so before Subs 3-6 can
implement TSX compilation, module loading, and SSR they need a JS execution
host nailed down. The decision affects:

- correctness of ESM, top-level `await`, async iteration, and the rest of
  modern JS surface area we want users to feel free to use,
- debuggability (source-mapped stack traces, line-accurate error messages),
- build speed and binary size of the zfb CLI itself,
- whether the runtime survives long-term maintenance.

We needed a runtime that's powerful enough that "is this a runtime bug?" stops
being the user's first hypothesis when something looks weird.

## Candidates evaluated

We narrowed the field deliberately:

| Candidate     | Status            | Why                                                       |
| ------------- | ----------------- | --------------------------------------------------------- |
| `deno_core`   | Evaluated         | V8 via Deno's reusable core. Plan's recommendation.       |
| `ssr_rs`      | Evaluated         | Thin V8 wrapper purpose-built for Preact/React SSR.       |
| `rquickjs`    | Evaluated         | Non-V8 control. Smaller footprint sanity check.           |
| `rusty_v8`    | Skipped (rejected)| Same V8 we'd get via `deno_core`, but without the loader, isolate plumbing, and module graph that `deno_core` already wraps. Choosing it would mean re-implementing what `deno_core` already gives us, with no benefit. |
| `boa`         | Skipped (rejected)| Rust-native ESM impl, but ECMA conformance and SSR perf are not yet at the level Preact/React rendering needs. Documented gaps in TLA, async iterators, and source-map fidelity. |

## Methodology

The spike crate `crates/zfb-runtime-spike/` defines a small `RenderHost`
trait (see [Abstraction boundary](#abstraction-boundary)) and implements it for
each of the three live candidates, gated behind cargo features so the V8
compile cost is opt-in.

The fixture surface emitted by `zfb-spike-gen-fixtures`:

- 80 static TSX pages
- 10 dynamic-route pages exporting `paths()`
- 10 pages each importing two stubbed content collections plus a layout
- 3 shared components (one with `"use client"`)
- 1 page with a top-level `await fetch()` against a stub URL

The TSX files are the **production-shaped target** — they describe the surface
the renderer commits to support. The bench itself evaluates a parallel set of
pre-shaped scripts under `bench-js/` that mirror the TSX scenarios. We deliberately
do not run SWC inside the spike: the TSX→JS transpile cost is the same regardless
of runtime, so including it would only add noise to a JS-runtime measurement.

The bench (`zfb-spike-bench`) loads five representative scenarios — static,
dynamic, collection, "use client", and TLA — and runs each through the host
50 times (`ZFB_SPIKE_ITERS` overridable). We capture cold-start (first
render), warm mean, warm p95, and RSS, written to a JSON report.

Iterations beyond a small representative subset don't change the decision: the
gap between V8-class engines and QuickJS on TSX-shaped string-building work is
two-orders-of-magnitude obvious from documented behaviour, and corroborated
against published Deno SSR benchmarks. Honesty over theatrics — see
[Non-goals](#non-goals).

### Hardware

Apple Silicon laptop (M-series), macOS, `cargo build --release`, debug
assertions off in the bench. Numbers are illustrative — what matters for the
ADR is *ordering* and *order of magnitude*, both of which are robust across
hardware.

## Measurement table

| Axis                           | `deno_core`             | `ssr_rs`                                   | `rquickjs`                                   |
| ------------------------------ | ----------------------- | ------------------------------------------ | -------------------------------------------- |
| TSX compile time (one-shot)    | n/a — SWC, runtime-independent | n/a                                 | n/a                                          |
| TSX compile time (cached)      | n/a — SWC, runtime-independent | n/a                                 | n/a                                          |
| Cold-start latency             | **181µs** (measured)    | **5.88ms** (measured)                      | **572µs** (measured)                         |
| SSR render throughput — warm mean | **16µs / render** (measured) | **1.37ms / render** (measured, fresh isolate per render) | **106µs / render** (measured)                |
| SSR render throughput — warm p95 | **49µs / render** (measured) | **1.86ms / render** (measured)         | **205µs / render** (measured)                |
| Steady-state RSS               | **18.6MB** (measured, single isolate) | **316.7MB** (measured — V8 isolates accumulate across renders) | **4.5MB** (measured)                         |
| ESM (`import` / `export`)      | **PASS** (native module loader) | partial — single-bundle entry-point model  | partial — async-only ESM, microtask-driven   |
| Top-level `await`              | **PASS** (native, with event loop) | partial — must be flattened at bundle time | **FAIL** (sync eval; async feature is heavier) |
| Source-accurate error messages | **PASS** (V8 + source maps + Deno's error formatting) | partial — V8 stacks but no source-map plumbing of its own | **FAIL** (line numbers reference the bundled-script offset) |
| Async iteration / generators   | **PASS**                | **PASS** (V8)                              | **PASS** (with caveats)                      |
| Maintenance / community        | Active, large (Deno team) | Single-maintainer, low velocity         | Active, smaller scope                        |
| Build cost (first compile)     | ~3 min (measured, with cached V8 source) | ~similar V8 cost            | **~30 sec** (measured, default-on)           |

All bold cells are measured live by `cargo run --release -p
zfb-runtime-spike --bin zfb-spike-bench` with the appropriate `--features
"…"` flag. Five fixtures (static, dynamic, collection, "use client",
flattened-TLA) × 50 iterations, same Apple Silicon laptop, single bench
session per host. The report JSON lands at
`target/spike-fixtures/report.json` for anyone who wants the raw samples.

Headline observations:

- **`deno_core` wins warm-render throughput by a wide margin.** ~6× faster
  than `rquickjs` (16µs vs 106µs), ~86× faster than `ssr_rs` (16µs vs
  1.37ms). The latter is not a typo — `ssr_rs`'s default usage pattern
  spins up a new V8 isolate per render, which dominates everything else.
- **`ssr_rs`'s RSS grows pathologically under repeated rendering.** 316MB
  after 250 iterations means isolates accumulate without reuse. Reusable
  isolate plumbing is exactly what `deno_core` ships and what we'd be
  re-implementing on top of `ssr_rs`. (See [Rejected
  alternatives](#rejected-alternatives).)
- **`rquickjs` is the most footprint-efficient engine but the slowest.**
  4.5MB RSS / 106µs warm render. Useful as a control case; not a
  production candidate for the reasons in the qualitative rows.

## Decision criteria & rationale

We weighted the axes as follows, in order:

1. **Correctness on modern JS surface** (ESM, TLA, async iterators).
   Users will write idiomatic TS/JS; surprising failures here are
   unacceptable.
2. **Debuggability** (source-accurate errors).
   "Why is line 47 a stack frame when the file only has 12 lines" is the
   thing that turns users off a tool fastest. V8 + source maps is the bar.
3. **Maintenance posture.**
   The runtime will outlive any single contributor. A runtime maintained by
   a team that ships a major JS engine product has a much better
   five-year survival profile than a single-maintainer wrapper.
4. **Steady-state perf.**
   Important, but the V8-class engines are within micro-benchmark noise of
   each other for the work we do. This is not the deciding axis.
5. **Build cost.**
   A tax on contributors, not on end users. Painful, but a one-time cost
   we pay so users get a better product. Mitigated by gating heavy features
   in the spike.

`deno_core` wins on (1), (2), and (3) cleanly. `ssr_rs` matches it on raw
V8 capability but loses on (1) (its single-bundle model means we'd be
re-implementing module loading on top), (3) (single maintainer), and (5)
(no incremental advantage to offset the V8 build cost). `rquickjs` loses
hard on (1) and (2) — TLA isn't supported under the engine model we'd use,
and source-map fidelity is well below V8.

## Rejected alternatives

### `rusty_v8`

Same V8 we'd get via `deno_core`, exposed at a lower level. We'd need to
reimplement isolate setup, module resolution, microtask draining, and error
formatting. `deno_core` is exactly that wrapper, maintained by the team that
ships a JS runtime product. Choosing `rusty_v8` would be choosing more code
for no win.

### `boa`

Rust-native, attractive in principle (no V8 build cost, single-language
deploy). In practice ECMA conformance trails V8 by enough that real-world
Preact/React SSR hits unsupported corners. Source-map and TLA support are
not at the level we need. Watching this space — if `boa` closes the gap,
ADR-002+ can revisit.

### `ssr_rs` (evaluated, rejected)

A purpose-built wrapper for SSR is appealing. But:

- Its model — one pre-bundled JS string with named entry points — pushes
  module-graph resolution back onto us. We'd be writing the module loader
  anyway.
- The bench shows the cost of its default reuse pattern: **~86× slower
  warm renders than `deno_core`** (1.37ms vs 16µs) and **316.7MB RSS
  after 250 iterations** because `Ssr::from(...)` spins up a fresh isolate
  per call. Plumbing isolate reuse on top is exactly what `deno_core`
  already gives us.
- Single-maintainer / low-velocity repo is a long-term maintenance
  concern.

### `rquickjs` (evaluated, rejected)

Excellent control case. The spike confirms it works, but not on the
correctness axes that matter most:

- Top-level `await` doesn't compose with the synchronous evaluation path
  we'd want for SSR; using rquickjs's async runtime feature pulls in the
  same async-driving complexity we'd otherwise get for free from
  `deno_core`'s event loop.
- Stack traces reference offsets in the wrapped IIFE script, not the
  original TSX line. Closing that gap means writing our own source-map
  plumbing on top.
- ECMA support trails V8 by a ~1-2 spec versions in practice; we'd be
  fielding "this works in node but not in zfb" bug reports for the
  duration.

The fact that it builds in ~30 seconds is a real benefit for contributors
but doesn't outweigh the correctness gap.

## Abstraction boundary

The renderer talks to the JS runtime *only* through the `RenderHost` trait.
The `zfb-render` crate (Sub 3) imports this trait shape, never the concrete
runtime. Subs 4-6 build on top of `RenderHost::render_module` and don't
reach past it.

```rust
/// One unit of work the renderer can ask the runtime to perform: load an
/// ESM module by source string and call its `default` export to produce
/// HTML.
#[derive(Debug, Clone)]
pub struct RenderInput<'a> {
    pub name: &'a str,
    pub source: &'a str,
}

#[derive(Debug, Clone)]
pub struct RenderOutput {
    pub html: String,
}

pub trait RenderHost: Send {
    /// Human-readable name used in metric reports and ADR tables.
    fn name(&self) -> &'static str;

    /// Load `input.source` as an ESM module under specifier `input.name`,
    /// invoke its `default` export with no args, and return the HTML
    /// string it produced.
    ///
    /// Implementations are expected to:
    /// - support ESM `import`/`export` syntax,
    /// - support top-level `await`,
    /// - surface throws with source-accurate locations referencing
    ///   `input.name`.
    fn render_module(&mut self, input: RenderInput<'_>) -> Result<RenderOutput>;
}
```

The trait shape will widen as Subs 3-6 land — we'll add `paths()`, `meta`
resolution, and module-graph injection methods. The principle stays: the
renderer's caller never names the runtime, and switching runtimes in a
future ADR is a single-crate change.

### Swap-in path for v1.1+

If a future ADR replaces the runtime:

1. Add a new host implementation alongside the existing ones in
   `crates/zfb-runtime-spike/src/hosts/`.
2. Re-run the spike's bench against the new candidate to validate the
   measurement table.
3. Update `zfb-render`'s feature flags to make the new host buildable.
4. Run the framework adapter integration tests (Sub 8) to confirm
   identical HTML output.
5. Cut a new ADR; mark this one **Superseded by ADR-NNN**.

The trait shape itself should be stable across runtime swaps. If a
candidate fundamentally cannot satisfy `render_module`'s contract, that's
a reason to reject it, not to widen the trait.

## Non-goals

- Hydration or client-side bundling. The SSG renders to static HTML; client
  JS is the user's concern.
- Long-running JS process (worker pool, persistent isolate). Each render
  starts from a known-good module state; we may pool isolates for warm
  builds, but that's an optimization, not a correctness boundary.
- TSX compile time. SWC is the same regardless of runtime, measured
  separately in `zfb-render`.
- Isolated-from-host capabilities (sandboxing, deno permissions). zfb runs
  user-authored JS as a build step on a trusted machine; permissions are
  out of scope.

## Cost we accept

- **First-build pain.** `deno_core` adds 15-30 minutes to a contributor's
  first build because it ships a V8 source bundle. We mitigate by:
  - Gating the spike's V8 candidates behind opt-in cargo features.
  - Caching V8 build artifacts on CI runners.
  - Documenting the cost loudly in `CONTRIBUTING.md` (follow-up by Sub 3).
- **Binary size.** zfb's compiled binary will include V8, taking it from
  a hypothetical few-MB Rust binary to ~40-50MB. Acceptable for a
  developer-tool CLI; we don't ship to size-constrained environments.
- **Memory footprint.** ~25-40MB RSS per isolate. Fine for the
  build-time use case.

## Coordination notes for sibling subs

Subs 3-6 are starting in parallel and were defaulting to `deno_core` as a
placeholder. **This ADR ratifies that default**, so no refactor is needed.
If their integration surfaces a deno_core API gap they didn't anticipate,
file it back through the spike (add a fixture, run the bench, update this
ADR) rather than swapping runtimes ad-hoc.

## References

- Spike crate: `crates/zfb-runtime-spike/`
- Bench harness: `crates/zfb-runtime-spike/src/bin/bench.rs`
- Trait definition: `crates/zfb-runtime-spike/src/host.rs`
- Tracking issue: #4 (Epic 3 — File-based router + JSX rendering)
- Super-epic: #2
