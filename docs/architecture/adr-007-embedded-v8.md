# ADR-007: Embedded V8 (`deno_core`) replaces miniflare subprocess for build-time SSG

- **Status:** Accepted
- **Date:** 2026-05-04
- **Owners:** Epic #160 (Embed V8 — replace miniflare subprocess with embedded deno_core)
- **Supersedes:** [ADR-005](./adr-005-ssg-first.md) (miniflare subprocess + Hono-style adapter pattern)

## Decision (one sentence)

**Build-time TSX→HTML rendering runs through an in-process V8 isolate via
`deno_core`, behind the existing `RenderHost` trait; the Node.js runtime is no
longer required on end-user machines or in Tauri-bundled distributions.**

## Context

ADR-005 superseded ADR-001's `deno_core` choice on two grounds:

1. Running build-time SSG through a different JS runtime than the production
   Cloudflare Workers target (workerd) invites quiet behavioural drift.
2. Embedding V8 adds a ~40-50 MB CLI binary and a 15-30 minute first-build
   compile cost on contributor machines — a tax that is hard to justify for a
   tool that delegates to a subprocess anyway.

Both concerns were real when ADR-005 was written. Two things have changed since
then.

**The Tauri distribution goal.** zfb is targeting a Tauri-bundled desktop
distribution: users install a `.app` / `.exe` / `.AppImage` and run `zfb
build` directly, with no Node.js installed. miniflare is a Node package; Node
is the only remaining reason a user machine needs a JS runtime installed to
_run_ `zfb build`. Removing that requirement is a hard goal for the Tauri
path.

**The bundle-contract insight.** ADR-005's runtime-parity argument — "run the
same engine as workerd at build time" — rests on a premise that no longer
applies: zfb's production deployment contract is the _bundle shape_, not the
build-time engine. The bundle `export default { fetch }` is what Cloudflare
Workers loads; whatever engine executed it at build time is irrelevant to what
workerd does at request time. The workerd-shape bundle was always the stable
contract. ADR-005 correctly preserved that contract; this ADR extends the
argument: one bundle shape, multiple runtimes that consume it.

**WIP timing.** zfb is mid-scaffolding (Phase A epics still in progress).
Committing to the right build-time host now is far cheaper than retrofitting
after the full stack lands.

The binary-size and first-build-time costs ADR-001 and ADR-005 cited as
reasons to avoid `deno_core` are real costs that this ADR accepts and
documents. They are the price of dropping the Node requirement for end users.
The trade is explicit.

## Decision details

### Build-time path (SSG)

The miniflare subprocess path (ADR-005: spawn Node + miniflare-bootstrap.mjs,
communicate over HTTP loopback) is replaced by an in-process `deno_core`
isolate:

1. The Rust orchestrator (`crates/zfb-build`) collects the route table and
   per-page props from `zfb-router` / `zfb-content`.
2. It creates an in-process V8 isolate via `deno_core`, loading the same
   workerd-shape bundle that `@takazudo/zfb-runtime` builds with esbuild. No
   subprocess is spawned.
3. The Rust side calls into the isolate through the `RenderHost` trait (the
   trait is unchanged; the concrete impl swaps). Per-route props are passed as
   JSON; the isolate invokes the bundle's `fetch` entry with a synthetic
   `Request`, collects the `Response` (HTML), and returns it.
4. After all pages are rendered, the orchestrator writes plain HTML to
   `dist/`. The isolate is torn down.

The shipped artifact is plain static HTML. No JS runtime is embedded in the
_output_; no JS runtime is required to _host_ the output.

### Production path (SSR for `prerender = false` routes)

**Unchanged.** The bundle's `export default { fetch }` entry is deployed to
Cloudflare Workers exactly as before. workerd executes it on request. The
production path is not aware of which build-time engine produced the bundle.

### Architecture principle

> One contract (workerd-shape bundle), multiple implementations:
> `deno_core` for build, workerd for production.

### What changes

- `crates/zfb-build/src/renderer.rs`: swap `Backend::SpawnMiniflare` for
  `Backend::EmbeddedV8`.
- `crates/zfb-render/`: add a new `DenoCoreHost` implementation of the
  `RenderHost` trait (Sub 2 — #162).
- `packages/miniflare-bootstrap.mjs` and the associated Node npm scripts:
  removed in Sub 7 (#167) cleanup wave.
- Root `package.json`: remove `miniflare` dependency in Sub 7.

### What stays unchanged

- The workerd-shape bundle contract (`export default { fetch }`).
  `crates/zfb-build/src/bundler.rs` is unchanged in shape.
- `packages/zfb-runtime` (Hono router) — the API the bundle relies on at
  build time and at production serve time is the same.
- `packages/zfb-adapter-cloudflare` — completely unaffected.
- The `RenderHost` trait (see [Abstraction boundary](#abstraction-boundary)).

### `deno_core` extension set

The in-process host requires a set of `deno_core` extension crates to cover
the Web API surface `@takazudo/zfb-runtime` relies on:

| Extension crate | Purpose |
| --------------- | ------- |
| `deno_fetch`    | `fetch()` API — Hono's routing core calls `new Request(...)` at build time |
| `deno_web`      | `TextEncoder` / `TextDecoder`, `ReadableStream`, `Event` |
| `deno_url`      | `URL`, `URLSearchParams` |
| `deno_console`  | `console.log` / `.error` (build-time diagnostics) |

These four plus `deno_core` itself are sufficient for `@takazudo/zfb-runtime`'s
current dependency surface. Versions are pinned in `Cargo.toml` (see
[compatibilityDate migration](#compatibilitydate-migration)).

### `node:*` polyfill caveat

miniflare-bootstrap today runs under workerd with
`compatibilityFlags: ["nodejs_compat", "enable_nodejs_fs_module"]`. This means
any user code that contains a top-level `import "node:fs"` (or any other
`node:*` specifier covered by `nodejs_compat`) can _resolve_ at module-load
time even when the import is never actually executed. Typical example: Astro-era
content helpers like `loadCategoryMeta` wrap a `node:fs` call behind a runtime
guard but leave the top-level import unconditional.

Under `deno_core`, `node:*` specifiers are not resolved natively. Without
explicit wiring, a top-level `import "node:fs"` throws a module-resolution
error before any user code runs.

**Chosen strategy: runtime stubs registered as a `deno_core` extension.**

A single `op_node_compat` extension is registered alongside the Web API
extensions. It intercepts every `node:*` specifier in the module loader and
returns a stub module whose every named export is a function that throws at
call time:

```
throw new Error(
  `${specifier} is not available under the zfb SSG runtime. ` +
  "Guard your import with a build-time check or move the call to Rust."
);
```

The stub resolves at module-load time, so top-level `import "node:fs"` passes
the module graph check. The error only fires if the call actually executes at
render time.

**v1 stub coverage** (`node:*` specifiers in actual use across the user-facing
surface and Astro-migration helpers):

- `node:fs`
- `node:fs/promises`
- `node:path`
- `node:url`
- `node:buffer`

Sub 2 (#162) expands the list as gaps surface during integration. The strategy
commits to the full `node:*` namespace — any unknown `node:*` specifier falls
back to the same stub template — so gaps only need a new entry in the stub
registry, not a structural change.

**User-facing limitation (documented in user guide).** Any render-time call to
a stubbed `node:*` export is a hard error. The migration path:

1. Identify the call site (the error message names the module and member).
2. Either guard with `if (typeof process !== 'undefined')` and confirm the
   branch is never taken under SSG, or move the logic to build-time Rust via
   the content bridge.

**Rejected alternatives.**

- **`deno_node` compat layer.** The Deno project ships a full Node.js
  compatibility layer that translates most of the Node standard library on top
  of `deno_core`. Rejected: the layer brings in ~150k lines of JS and dozens of
  Rust ops covering file system, process, crypto, streams, and more. The
  maintenance cost and binary-size impact are disproportionate to the actual
  usage — we need load-time resolution for a handful of specifiers, not runtime
  fidelity for the Node standard library.
- **Bundle-time esbuild strip.** Configure esbuild to strip `node:*` imports
  from the bundle at build time (mark them `external` + resolve to empty). This
  is too fragile: pnpm-linked packages can resolve to different on-disk paths
  across machines; side-effect-only imports that `nodejs_compat` makes harmless
  can disappear differently; and the behaviour diverges from the workerd
  production target where `nodejs_compat` is still active. The stub strategy
  keeps the module graph intact and surfaces any real call-time use loudly.

### `compatibilityDate` migration

miniflare-bootstrap.mjs today pins:

```js
compatibilityDate: "2025-01-01"
```

This anchors workerd's runtime semantics to a snapshot so zfb's JS surface
does not drift as workerd ships new flag defaults across CI workers.

`deno_core` has no equivalent concept. Its built-in surface (Web APIs,
extensions) is pinned by the crate version, not a date string.

**Chosen replacement: pin specific crate versions in `Cargo.toml`.**

```toml
[dependencies]
deno_core  = "=0.x.y"   # pinned
deno_fetch = "=0.a.b"   # pinned
deno_web   = "=0.c.d"   # pinned
deno_url   = "=0.e.f"   # pinned
deno_console = "=0.g.h" # pinned
```

The exact versions are chosen at Sub 2 implementation time and committed to the
lockfile. Upgrading the V8 host surface requires an explicit version bump in
`Cargo.toml` — same intentionality as bumping `compatibilityDate`.

**Known behavioural delta.** `deno_fetch` implements the Fetch specification
against its own TLS / redirect / credential handling; it is not workerd's
`fetch`. Known differences:

- workerd's `fetch` respects Cloudflare-specific header semantics (e.g.
  `CF-Connecting-IP`); `deno_fetch` does not. This is irrelevant at build
  time because no real HTTP requests are made during SSG — Hono routes are
  called with synthetic `Request` objects, not live network traffic.
- workerd's cache API (`caches.default`) is not present in `deno_core`. User
  code that reaches for `caches` at module-load time will get an undefined
  reference. This is accepted for v1; if a real consumer hits it, the op_node
  pattern extends naturally to a `caches` stub.

There is no plan to re-implement a `compatibilityDate` abstraction on top of
`deno_core`. The Cargo version pin is the full story.

### Source-map fidelity

`deno_core` surfaces JS exceptions as
`<specifier>:<line>:<col>: <message>` — the standard V8 error format.
The existing `crates/zfb-render/src/sourcemap.rs` decoder takes
`(generated_line, generated_col)` pairs and looks up the original source
location from the esbuild-emitted source map. It is format-agnostic: the
caller is responsible for parsing the frame string into `(line, col)` before
calling `decode_position`.

`deno_core` stack frames use the same `<file>:<line>:<col>` convention that
`sourcemap.rs` was designed around (the miniflare subprocess forwarded frames
in the same format). No structural change to the decoder is expected. If the
frame parser in the diagnostics renderer needs minor adaption to `deno_core`'s
exact error format, that work is in scope of Sub 2 (#162). The decoder itself
(`decode_position`) is confirmed format-agnostic and does not need to change.

## Abstraction boundary

The `RenderHost` trait in `crates/zfb-render/src/render_host.rs` is unchanged.
It exposes three async operations — `execute_module`, `call_default`,
`get_export` — and the renderer never names the concrete host.

ADR-001's [Swap-in path for
v1.1+](./adr-001-js-runtime.md#swap-in-path-for-v11) explicitly anticipated
this kind of revision:

> 3. Update `zfb-render`'s feature flags to make the new host buildable.
> 4. Run the framework adapter integration tests (Sub 8) to confirm identical
>    HTML output.
> 5. Cut a new ADR; mark this one **Superseded by ADR-NNN**.

This ADR is that ADR-NNN. Steps 3-5 are the content of the Embed V8 epic.

## In scope / out of scope

### In scope

- `crates/zfb-render` — new `DenoCoreHost` implementation (Sub 2).
- `crates/zfb-build` — swap `Backend::SpawnMiniflare` for `Backend::EmbeddedV8`
  (Sub 4 — #164).
- Root `package.json` — remove `miniflare` dev dependency (Sub 7 — #167).
- Docstrings in `render_host.rs`, `sourcemap.rs`, `renderer.rs` that currently
  refer to miniflare as the active backend (Sub 7).
- This ADR and cross-reference edits in ADR-001, ADR-003, ADR-004, ADR-005.

### Out of scope

- Bundler shape (`crates/zfb-build/src/bundler.rs`) — unchanged.
- Cloudflare adapter (`packages/zfb-adapter-cloudflare`) — unchanged.
- Hono runtime API (`packages/zfb-runtime`) — unchanged.
- Incremental-orchestrator work (`zfb-build`'s watch / rebuild scheduling) —
  a separate investment captured as a follow-on.
- Tauri integration — this epic delivers the build-time architecture that makes
  Tauri tractable; it does not ship a Tauri app.

## Consequences

### Positive

- **Node.js no longer required to run `zfb build`.** End users and
  Tauri-bundled distributions install one binary. Node is still required for
  repo dev tooling (prettier, lefthook, fetch scripts, esbuild managed by pnpm)
  but contributors building user sites don't need it installed on the machine
  that runs `zfb build`.
- **Per-build subprocess startup latency is eliminated.** miniflare's
  cold-start overhead (tens of milliseconds per build, measured in T6 under
  ADR-005) goes away. For projects with many pages the saving is real, though
  modest.
- **One fewer external process.** No more IPC channel, no more port-binding
  race, no more process-exit cleanup on Ctrl-C.

### Negative — costs we accept

- **First-build compile time.** `deno_core` adds 15-30 minutes to a
  contributor's first `cargo build` because it ships a V8 source bundle
  (~100 MB when unpacked). This is the same cost ADR-001 accepted. Sub 3 (#163)
  lands the CI mitigation (pre-built V8 artifact cache on CI runners).
  `CONTRIBUTING.md` documents the cost loudly.
- **CLI binary growth: ~30-40 MB.** The zfb binary grows from a few MB to
  ~40-50 MB once V8 is statically linked. Acceptable for a developer-tool CLI;
  zfb does not ship to size-constrained environments.
- **dev-loop `reload()` latency may regress.** miniflare's hot-reload
  respawn was measured at ~50ms. Destroying and recreating a V8 isolate between
  builds is heavier. This is accepted for v1; module-re-evaluation as an
  optimisation (keep the isolate warm across file-watch cycles, only
  re-evaluate changed modules) is captured as a follow-on.

### Neutral

- The `RenderHost` trait survives unchanged. No caller code in `zfb-render`
  changes; only the concrete host implementation swaps.
- `compatibilityDate` as a concept disappears. Its functional role — pinning
  the JS surface so CI doesn't drift — is taken over by explicit Cargo version
  pins. The mechanism is more transparent: a `Cargo.toml` diff is visible in
  PRs; a miniflare bootstrap string hidden in a `.mjs` file was not.
- The `__zfb.content` global (ADR-004 content bridge) and the `getCollection`
  / `getEntry` helpers are wired through the same `globalThis` injection path,
  now against `deno_core`'s runtime API instead of workerd's. No TypeScript
  surface change.

## Alternatives considered

### Keep miniflare (the ADR-005 decision)

The natural baseline. Rejected because it permanently requires Node on every
machine that runs `zfb build`. For the Tauri distribution target that is a
hard blocker. The build-time runtime-parity benefit miniflare offered is
real but outweighed by the distribution constraint.

### Use `deno_core` without the Web API extensions (bare V8 only)

Wire only `deno_core` itself, no `deno_fetch` / `deno_web` / `deno_url`.
Rejected. Hono's routing core calls `new Request(...)` and `new URL(...)` at
bundle-initialization time; without the Web API extensions the bundle fails to
load. The extension set documented above is the minimum viable surface.

### Embed workerd directly

Workerd does not expose a stable Rust embedding API. Attempting to embed it
as a native library would buy back the binary-size problem with worse
maintenance posture and no upstream support.

### Ship two binaries (lean + V8)

Offer a `zfb-lean` binary without V8 that still delegates to a miniflare
subprocess, alongside a `zfb` binary that embeds V8. Rejected because it
fragments the tool: users have to choose, documentation has to branch, CI
matrix doubles. The complexity cost is not worth preserving a mode that
requires Node when the whole motivation is removing that dependency.

### `wasm_bindgen` + V8 WASM build

Run V8 in a WebAssembly sandbox inside the Rust process. Rejected: V8's WASM
build is not designed for the embedding use case; startup and JIT performance
regress substantially compared to native.

## References

- Sub 2 (V8 host implementation): #162
- Sub 4 (renderer integration / build path swap): #164
- Sub 7 (miniflare cleanup + docstring updates): #167
- Source conversation transcript: `/mnt/c/Users/takaz/Dropbox/screenshots/20260504_183426-conversation.md`
- Big-plan log: `/home/takazudo/cclogs/zfb/20260504_184222-big-plan-embed-v8.md`
- ADR-001 (superseded by ADR-005, then ADR-007 re-adopts deno_core; cross-ref updated): `docs/architecture/adr-001-js-runtime.md`
- ADR-005 (superseded by this ADR): `docs/architecture/adr-005-ssg-first.md`
- `RenderHost` trait: `crates/zfb-render/src/render_host.rs`
- Source-map decoder: `crates/zfb-render/src/sourcemap.rs`
- Epic: #160
