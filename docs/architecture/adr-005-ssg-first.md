# ADR-005: SSG-first via miniflare subprocess + Hono-style adapter pattern

- **Status:** Accepted
- **Date:** 2026-04-28
- **Owners:** Epic 6 (zfb-ssg-render — SSG-first execution model)
- **Supersedes:** [ADR-001](./adr-001-js-runtime.md) (deno_core / V8-in-binary
  JS runtime).

## Decision (one sentence)

**Build-time TSX→HTML rendering runs through `miniflare` (workerd) as a
short-lived npm subprocess driven by the `@takazudo/zfb-runtime` package; the
Rust CLI does not embed a JS runtime, and the same Hono-style page router
handles both build-time SSG and runtime SSR.**

## Context

ADR-001 ratified `deno_core` as zfb's embedded JS runtime, behind the
`RenderHost` trait in `zfb-render`. That decision was correct for the
question it answered (which V8-class engine has the best correctness +
debuggability + maintenance posture for embedded SSR), but the question has
moved.

Three things changed since ADR-001:

1. **Deployment target consolidated on Cloudflare Workers (workerd).** zfb's
   runtime SSR story — for routes that opt out of static prerendering or
   need request-scoped data — targets Cloudflare Workers. Running build-time
   SSG in a *different* JS runtime than the production SSR runtime creates
   quiet behavioural drift: a Web API that miniflare/workerd ships but
   deno_core does not (or vice versa) becomes a "works on my machine, fails
   on prod" trap. The whole point of choosing a JS runtime once was to stop
   that class of bug.
2. **The deno_core binary footprint stopped being a tax we want to pay.**
   Bundling V8 makes the zfb CLI a ~40-50 MB binary and adds 25-40 MB RSS
   per isolate. For a build-time tool that runs to completion and exits,
   paying that cost on every contributor's first build (15-30 min compile)
   and every CI run is a poor trade against subprocess startup, which is
   measured in tens of milliseconds for miniflare.
3. **The router emerged as the right abstraction, not the runtime.** What
   `zfb-render` actually needs from "the JS side" is not raw module
   evaluation — it is "given a request shape, produce HTML". That is the
   Hono adapter pattern: one router, many entry adapters (build-time
   crawler, miniflare worker, Cloudflare Workers production). The runtime
   choice falls out of the adapter choice.

ADR-001's `RenderHost` trait survives this change. It was always the
abstraction seam, and the comments in that ADR explicitly anticipated swap-in
paths for v1.1+. ADR-005 takes that path.

## Decision details

### Build-time path (SSG)

For every TSX page that prerenders to static HTML:

1. The Rust orchestrator (T6 — build-time render orchestration) collects
   the route table and per-page props from `zfb-router` / `zfb-content`.
2. It spawns a single miniflare worker as an npm subprocess, loading the
   bundle that `@takazudo/zfb-runtime` exposes (the bundler step lands in
   T3). miniflare runs workerd locally — same JS runtime as Cloudflare
   Workers in production.
3. The Rust side hands routes + props into the worker over the IPC boundary
   exposed by `RenderHost` (the trait stays; the concrete impl is a
   subprocess client, not a deno_core isolate).
4. The worker invokes the user's page router (constructed via
   `createPageRouter` from `@takazudo/zfb-runtime`) with a synthetic
   `Request` per route, collects the `Response` (HTML + headers), and
   returns it across the boundary.
5. After all pages are rendered, the Rust orchestrator writes plain HTML
   files to `dist/`. The miniflare subprocess exits.

The shipped artifact is plain static HTML. No JS runtime is embedded; no JS
runtime is required to host the output. SSG users can deploy the output to
any static host without further runtime concerns.

### Runtime path (SSR for `prerender = false` routes)

For routes that opt out of static prerendering (per T5), the *same*
`createPageRouter` instance is the entry point of a Cloudflare Worker. The
build emits a worker bundle; Cloudflare Workers (workerd) executes it on
request. There is no second router and no second adapter — the build-time
SSG path and the runtime SSR path differ only in which adapter wraps the
router.

### Adapter pattern (Hono-style)

`@takazudo/zfb-runtime` exposes a router constructor and the same router
satisfies multiple entry adapters:

- `createPageRouter(routes)` — builds the router from a route table.
- The same router is invoked by:
  - the build-time miniflare adapter (per-route synthetic `Request`,
    collected `Response`, written to disk);
  - the production Cloudflare Workers adapter (worker `fetch` event handler
    delegates to the router);
  - a future `zfb dev` adapter (long-lived miniflare server with file-watch
    integration).

The pattern follows Hono's adapter model: one routing core, multiple
runtime-specific entry shells.

### API surface contract for `@takazudo/zfb-runtime` (input to T6)

The build orchestrator (T6, wave 2) reads this section as the contract for
what to wire up against. The package is expected to export at minimum:

- `createPageRouter(options)` — returns a router object that exposes a
  `fetch(request, env, ctx)` method matching the Cloudflare Workers
  `ExportedHandler` shape. Used identically by build-time and runtime
  adapters.
- A documented route registration shape (file-based route → handler /
  TSX module reference) compatible with the route table emitted by
  `zfb-router`.
- A bundle entry that can be loaded by miniflare without additional
  Cloudflare bindings, plus a worker entry that can be deployed to
  Cloudflare Workers as-is.
- An IPC contract for the build-time path: the orchestrator sends a list
  of routes + per-route props; the worker returns HTML per route. The
  exact wire format (newline-delimited JSON over stdio vs HTTP loopback
  vs miniflare's RPC surface) is T6's call but should be one of those
  three so the worker code stays runtime-portable.

Anything beyond that surface — middleware shape, error pages, streaming,
custom 404 handling — is the package's internal concern and out of scope for
this ADR.

## Consequences

**Positive.**

- Build-time and runtime use the *same* JS engine (workerd via miniflare /
  workerd in production). No runtime drift between SSG and SSR.
- The Rust binary stays small. No embedded V8, no 15-30 min first-compile
  on contributor machines, no 40-50 MB CLI binary.
- One router for both SSG and SSR. The production codepath gets exercised
  on every build, not only at deploy time.
- Hono-style adapter pattern is a known, documented, well-understood
  abstraction. Users moving from frameworks that use it will recognise it.

**Negative — costs we accept.**

- Build now requires Node.js + the `@takazudo/zfb-runtime` package
  installed; the Rust binary alone is no longer self-sufficient. For a
  framework whose user content is JS/TS this is a near-zero cost (Node is
  already on the box) but it is a cost.
- Subprocess startup adds latency to small builds. miniflare cold start
  is in the tens of milliseconds — fine for builds with more than one
  page. We measure this in T6.
- Crash diagnostics now cross a process boundary. Stack traces from inside
  the worker need to be surfaced through the IPC channel without
  truncation. T6 owns the error-shape contract.

**Neutral.**

- The `RenderHost` trait stays. The renderer code in `zfb-render` does not
  know whether the host is a deno_core isolate or a subprocess client. The
  surface that goes away is `DenoCoreHost`, the `deno_core_host` cargo
  feature, and the `zfb-runtime-spike` crate (whose entire purpose was to
  back ADR-001).

## Alternatives considered

### Keep `deno_core` (the ADR-001 decision)

Rejected. Loses runtime parity with the deployment target (Cloudflare
Workers) and pays a binary-size + first-build-time tax for a feature
(in-process JS hosting) we do not actually want at build time. ADR-001's
benchmarks remain valid for the question they answered; the question has
moved.

### Embed workerd directly (FFI / native bindings)

Rejected. workerd is C++ and does not expose a stable embedding API to the
same standard `deno_core` does. Re-embedding it would buy us back exactly
the binary-size problem we want to escape, with worse maintenance posture.

### Static-only SSG, no SSR adapter

Rejected. zfb has explicit support for `prerender = false` routes (T5).
Removing the runtime SSR path would simplify the build but eliminate a
documented feature. Carrying both paths in one router is the design.

### Per-page `node` subprocess instead of miniflare

Rejected. A bare Node subprocess does not give us workerd parity, and Node's
process startup is heavier than miniflare's (which keeps a single workerd
isolate hot for the duration of the build). The whole point of choosing
miniflare is "same JS runtime as production".

## Supersedes

ADR-001 (deno_core / V8-in-binary). The retirement work in this same change:

- removes `DenoCoreHost` and the `deno_core_host` feature flag from
  `zfb-render`;
- deletes the `zfb-runtime-spike` crate (its purpose was to back ADR-001's
  measurement table);
- preserves the `RenderHost` trait and `ModuleHandle` types, which remain
  the abstraction seam used by tests today and by the miniflare subprocess
  client landing in T6.

## References

- ADR-001 (superseded): `docs/architecture/adr-001-js-runtime.md`
- Tracking issue (Epic): #52
- T6 (downstream): build-time render orchestration via miniflare subprocess.
- T2 (downstream): `@takazudo/zfb-runtime` Hono-style page router.
