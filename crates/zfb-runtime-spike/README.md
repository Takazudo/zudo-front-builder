# zfb-runtime-spike

Bounded spike that backs **ADR-001 (JS runtime selection)**. Not part of the
production build — `[publish = false]` and excluded from any release pipeline.

The spike defines a tiny `RenderHost` trait and provides one host
implementation per candidate runtime. Each candidate is gated behind a cargo
feature so contributors only pay the (very real) V8 compile cost when they
explicitly want to run that candidate.

## Candidates

| feature   | crate          | notes                                                                  |
| --------- | -------------- | ---------------------------------------------------------------------- |
| `quickjs` | `rquickjs`     | non-V8 control. Compiles in seconds. Default-on.                       |
| `deno`    | `deno_core`    | V8 via Deno's reusable core. First build pulls V8 (15-30 min).         |
| `ssr-rs`  | `ssr_rs`       | thin V8 wrapper aimed at Preact/React SSR. Same V8 cost.               |

`rusty_v8` and `boa` are deliberately not included — see ADR-001 for rationale.

## Generate fixtures

```sh
cargo run --release -p zfb-runtime-spike --bin zfb-spike-gen-fixtures
```

This writes:

- `target/spike-fixtures/pages/static/page-NN.tsx` — 80 static TSX pages
- `target/spike-fixtures/pages/dynamic/[slug-NN].tsx` — 10 dynamic pages
- `target/spike-fixtures/pages/collections/post-NN.tsx` — 10 collection-pulling pages
- `target/spike-fixtures/components/shared-N.tsx` — 3 shared components, one with `"use client"`
- `target/spike-fixtures/pages/tla/late.tsx` — top-level await page
- `target/spike-fixtures/bench-js/*.mjs` — pre-shaped ESM that the bench actually evaluates

The TSX files exist as a **production-shaped target** — they describe the
fixture surface ADR-001 commits the renderer to support. The `bench-js/`
modules exist because the spike measures the **JS runtime**, not the SWC
transpile step. The transpile cost is the same regardless of which runtime is
chosen; running it inside the spike would only add noise.

## Run the bench

```sh
# Default: rquickjs only.
cargo run --release -p zfb-runtime-spike --bin zfb-spike-bench

# Add deno_core (15-30 min first build).
cargo run --release -p zfb-runtime-spike --bin zfb-spike-bench \
  --no-default-features --features deno

# All three.
cargo run --release -p zfb-runtime-spike --bin zfb-spike-bench \
  --no-default-features --features all-hosts
```

Environment knobs:

- `ZFB_SPIKE_ITERS` — warm-loop iterations per fixture (default 50).
- `ZFB_SPIKE_OUT`   — fixtures directory (default `target/spike-fixtures`).
- `ZFB_SPIKE_REPORT` — JSON report path (default `<out>/report.json`).

The bench prints a summary table and writes a JSON report with cold-start,
warm mean, p95, and RSS for each host that built and ran.

## What this spike does NOT measure

- TSX → JS transpile time. SWC is the same regardless of runtime; we measure
  it once in `zfb-render` proper, not here.
- Module-graph resolution under real `import` statements. The spike's
  bench-js modules are self-contained; ADR-001 calls this out and notes how
  the production renderer will exercise import resolution in `zfb-render`'s
  own integration tests.
- Hydration. Client-side bundling is out of scope for the SSG.

These omissions are intentional and documented in ADR-001 §"Methodology".
