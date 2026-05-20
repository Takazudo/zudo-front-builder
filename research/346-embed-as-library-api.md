# Research: Embed-as-library API for in-process Tauri integration (#346)

## 1. Question

Can `zfb-server` expose a public Rust crate API that lets a Tauri host
run zfb's server in-process — on a tokio thread inside the Tauri
process — instead of spawning the `zfb` binary as a child sidecar
(Mode D in the desktop-deployment composition matrix)?

In particular:

- Is `crates/zfb-server` already library-shaped, or does it need to
  graduate out of a CLI binary?
- What is a minimum public builder surface (under 10 methods) that
  reads cleanly from a Tauri `setup` callback?
- How should a `prerender = false` route handler receive per-request
  Tauri context (IPC handle, FS capabilities)? Options to weigh: tokio
  task-local, axum `Extension`, builder-provided context closure.
- Does CCResDoc's current `ccresdoc-server` crate become a thin layer
  on top of an embedded zfb server, or stay as-is?

## 2. What was tried

Read-only audit of the relevant crates (no edits to `crates/*` per
scope guardrail). Concrete actions:

- `gh issue view 346` — pulled the issue body and "next concrete
  step" list.
- Read `crates/zfb-server/Cargo.toml` — confirmed `[lib]` only; **no
  `[[bin]]` target, no `src/main.rs`**. The issue's "audit
  src/main.rs (or lib.rs)" only matches `src/lib.rs`.
- Read `crates/zfb-server/src/lib.rs` end-to-end (235 lines). Both
  public entry points — `serve(opts, shutdown)` and
  `serve_with_listener(opts, listener, shutdown)` — already accept a
  caller-supplied shutdown future and (in the latter case) a
  caller-bound `TcpListener`.
- Skimmed `crates/zfb-server/src/routes.rs` (2283 lines) — the
  parts that are unconditionally "dev-shaped" are: the per-response
  live-reload `<script>` injection in `inject_livereload_with_prefix`
  (called from `page_response_bytes`), the `Cache-Control: no-store`
  header on `/__zfb/livereload.js`, and the `/__zfb/reload` SSE
  endpoint. The page-cache → dist → public fallback chain, the
  `base` prefix mounting, the `ServeDir` for `/assets/*`, and the
  plugin dev-middleware dispatcher are all generic enough to keep in a
  production-shaped embed.
- Read `crates/zfb-server/src/livereload.rs` lines 1–100 — the only
  external coupling is `outcome_to_events(BuildOutcome)`, which is an
  opt-in helper. Nothing in the serve loop itself requires
  `zfb-build`.
- Read `crates/zfb-server/src/plugin_middleware.rs` lines 1–120 —
  the server already takes a trait object (`DevMiddlewareDispatcher`)
  for plugin dispatch. No hard dep on `zfb-build::PluginHost`.
- Read `crates/zfb-server/tests/integration.rs` lines 1–100 —
  third-party tokio code (the test harness, a `Harness::start()` that
  is not part of the bin crate) already embeds the server via
  `serve_with_listener` from an ephemeral port. **External embedding
  works today; the missing piece is ergonomics, not architecture.**
- Read `crates/zfb/src/commands/dev.rs` lines 1–407 — confirmed the
  bin crate's responsibilities and which of them belong on the
  "embedder" vs the "library":
  - bin-owned (don't move into `zfb-server`): config loading, watcher
    + dependency graph, dev renderer boot (`zfb_build::renderer::start`),
    on-disk graph persistence, plugin host spawn, Ctrl-C wiring.
  - already library-shaped in `zfb-server`: the `ServeOpts` struct
    (10 fields), `serve` / `serve_with_listener`, page cache,
    broadcast channel, plugin-middleware dispatcher.
- Read `crates/zfb/src/commands/preview.rs` lines 1–360 — confirmed
  that `zfb preview` does NOT reuse `zfb-server`; it stands up its own
  axum router from scratch. The reason: preview is intentionally
  prod-shaped (no livereload, no `__zfb/*`). This is a tell that
  today's `zfb-server` is dev-shaped enough that the preview path
  preferred a fresh router over toggling behaviour in `ServeOpts`.
- Searched `docs/src/content/docs/architecture/build-engine.mdx` and
  `docs/src/content/docs/guides/desktop-deployment.mdx` — confirmed
  the Mode A/B/C/D composition matrix and that Mode D is documented
  as "does not work today; no public API."
- Searched `docs/src/content/docs/guides/ssr-and-cloudflare-bindings.mdx`
  — `prerender = false` is a route-level export today and routes that
  set it are SSR-handled by Cloudflare Workers post-deploy. The dev
  server does NOT currently execute those handlers in-process.
- Built and ran a 100-line spike at `__inbox/346-zfb-embed-spike/`
  that exercises today's API. Result: passes `cargo test` (one
  test, embedding succeeds on an OS-assigned port). See §3 below.

## 3. Evidence

### 3.1 Audit: what is dev-shaped vs. embeddable today

| Concern | Status today | Verdict for Mode D |
|---|---|---|
| `[lib]`-only crate | yes, no `[[bin]]` / `main.rs` | already library-shaped |
| `serve` / `serve_with_listener` accept shutdown future and pre-bound listener | yes | embeddable today |
| `ServeOpts` is `Clone`, owns no globals | yes | safe to construct from a Tauri `setup` callback |
| Page cache, plugin middleware, injected routes — pluggable via `Option<…>` fields | yes | composes cleanly |
| Live-reload `<script>` injection on every HTML response | **unconditional** | needs a `mode: ServerMode { Dev / Preview / Embed }` toggle |
| `Cache-Control: no-store` on `__zfb/livereload.js` | unconditional, but only on the dev-only route | gated behind the same `Mode` |
| `/__zfb/reload` SSE endpoint always mounted | unconditional | gated behind `Mode::Dev` |
| Coupling to `zfb-build` | only via `outcome_to_events(BuildOutcome)` (opt-in helper) | already loose — no refactor required for Mode D |
| Production preview reuses `zfb-server` | **no — preview.rs builds its own router** | Mode D could replace this duplication once a `Preview` mode lands |

**Headline finding:** `zfb-server` is already library-shaped. The
issue body's worry that the crate is "production-shaped vs. dev-only"
overstates the gap — the crate is dev-*flavoured* (livereload script
injection, SSE endpoint), not dev-*coupled*. A Tauri host can call
`serve_with_listener` from its own tokio runtime today; the spike in
§3.4 demonstrates this. What's missing is (a) a builder shape that's
nicer to write from `setup`, (b) a flag to turn off the live-reload
flavour, and (c) an extension point for SSR (`prerender = false`)
handlers that need Tauri context.

This reframes the implementation cost from "major refactor" to
"polish + extension points + dev/prod toggle." Worth flagging in
estimates that fork off this research.

### 3.2 Proposed public builder API (9 methods)

Method count target was "under 10." The sketch below counts 9 methods
on the `Server` + `ServerBuilder` types combined: 1 entry on `Server`
(`builder`), 6 on `ServerBuilder` (`config_path`, `mode`, `bind`,
`with_request_extension`, `with_ssr_handler`, `build`), and 2
terminals on `Server` (`serve_in_thread`, `serve`). `ServerHandle`
ops (`addr`, `shutdown`, `join`) are excluded from the budget — they
belong to a separate type returned at runtime.

```rust
use zfb_server::{Server, ServerHandle, ServerMode};

// (1) Server::builder() — entry point. Returns a fresh ServerBuilder
//     with no required fields set.
let server = Server::builder()
    // (2) .config_path(path) — load zfb.config.{json,ts} from this
    //     path. Drives dist_root, public_root, base, trailing_slash.
    //     Mutually exclusive with the lower-level (3a-c).
    .config_path("./zfb.config.json")
    // (3) .mode(ServerMode::Embed) — Dev | Preview | Embed. Controls
    //     livereload script injection, /__zfb/reload, and
    //     Cache-Control on /__zfb/livereload.js. Default: Dev.
    .mode(ServerMode::Embed)
    // (4) .bind(addr) — SocketAddr to bind. Pass 127.0.0.1:0 for an
    //     OS-assigned port (read it back from ServerHandle::addr()).
    .bind("127.0.0.1:0".parse()?)
    // (5) .with_request_extension(value) — clones into axum's
    //     per-request extensions on every page handler invocation.
    //     The recommended Tauri-context passthrough (see §3.3).
    .with_request_extension(tauri_handle.clone())
    // (6) .with_ssr_handler(matcher, handler) — register an axum
    //     handler for `prerender = false` routes. Matches by URL
    //     pattern; the handler receives axum extractors including
    //     the request extensions above. Multiple registrations
    //     allowed; checked between page-cache lookup and dist/public
    //     fallback.
    .with_ssr_handler("/api/*path", api_routes::handler)
    // (7) .build() — validates required fields, returns Result<Server>.
    .build()?;

// (8) Server::serve_in_thread(self) — spawns the axum loop on the
//     current tokio runtime; returns a ServerHandle non-blockingly.
//     Use this from a Tauri `setup` callback that already runs on
//     tokio.
let handle: ServerHandle = server.serve_in_thread()?;

// (9) Server::serve(self).await — async variant for embedders that
//     want to await the server future themselves (e.g. CLI bin
//     crates). Equivalent to today's serve_with_listener.

println!("zfb embedded at {}", handle.addr());
// later:
handle.shutdown().await?;
```

Supporting public types:

- `pub struct Server` — built artefact, consumes self on serve.
- `pub struct ServerBuilder` — the type method (1) returns.
- `pub struct ServerHandle { addr(), shutdown(), join() }` — the
  three handle ops are not counted toward the "under 10 methods"
  budget because they're on a *different* type. If the reviewer
  pushes back, the easiest cut is to drop (9) `.serve(self)` since
  `.serve_in_thread()` covers the embed path and the bin crate can
  call into the lower-level `serve_with_listener` directly.
- `pub enum ServerMode { Dev, Preview, Embed }` — naming matches
  the existing `zfb dev` / `zfb preview` commands and reserves
  `Embed` for the Mode D path. Internally a thin alias for "no
  livereload injection, no `/__zfb/*` routes."
- `pub struct PageCache` — already exists; the builder retains a
  hook (`.with_page_cache(cache)`) for embedders that populate the
  cache externally (e.g. CCResDoc, which manages its own rebuild
  loop). This hook is **optional** and is not one of the 9 counted
  methods.

Compression knobs if 9 is still too many:

- Merge (4) `.bind(addr)` into `.mode(ServerMode::Embed { bind })`
  — clean for embed but ugly for dev/preview.
- Drop (9) `.serve(self).await` and keep only `.serve_in_thread()`
  — embedders only ever want non-blocking; `zfb dev` can keep using
  the low-level `serve_with_listener`.
- Defer (5) `.with_request_extension` to a follow-up — but then the
  Tauri-context use case doesn't compile in v1, defeating the point.

### 3.3 Per-request Tauri context passthrough

Two distinct surfaces are in play; pinning these prevents API drift:

**Surface (i) — Rust-side SSR handler.** The embedder registers an
`axum`-compatible handler via `.with_ssr_handler(pattern, handler)`.
The handler runs in the same tokio runtime as Tauri, has direct
access to `AppHandle`, can call Tauri commands, read `$HOME/.claude/`
synchronously, etc. No IPC hop.

**Surface (ii) — JS-side SSR handler running in the embedded V8
isolate.** Today's `prerender = false` Cloudflare adapter path. The
dev server does NOT execute these in-process; Cloudflare does after
deploy. For Mode D this would require a per-request scoped native
binding in the V8 host. **Out of scope for v1** — Mode D's win is
the Rust-handler path; the JS-handler path is a follow-up.

For Surface (i), the three options the issue calls out:

| Option | Pros | Cons | Verdict |
|---|---|---|---|
| **axum `Extension`** via `Router::layer(Extension(ctx))` or `with_request_extension` | Idiomatic axum. Composable. Handler reads via `Extension(ctx): Extension<TauriCtx>` extractor. Type-checked at handler-fn signature. Already supported by every axum 0.8 example. | Each extension is keyed by type; collisions are caught at runtime, not compile time. | **Pick this.** |
| Tokio task-local | Works across non-axum code (e.g. inside the V8 host's native bindings — useful for Surface (ii) later). | Implicit dataflow — the next dev who reads the handler can't tell where `TauriCtx` comes from. Easy to leak across `tokio::spawn` boundaries. | Reserve for Surface (ii) follow-up. |
| Builder-provided context closure `Fn(Request) -> Ctx` | Lets the embedder synthesise per-request context. | Doesn't compose with axum's extractor model — every handler needs to call the closure explicitly. | Reject. |

**Recommendation:** v1 ships `with_request_extension<T: Clone + Send +
Sync + 'static>(value: T)` which inserts a `tower::ServiceBuilder ::
layer(Extension(value))` on the page-handler route. SSR handlers
read it via the standard `axum::extract::Extension<T>` extractor. The
matching design note in CCResDoc-side embedding (#349) can reference
"SSR handlers take `Extension<AppHandle>`."

When Surface (ii) lands (V8-side SSR), add a parallel
`with_request_taskscope<T>(value: T)` that pushes a tokio task-local
for the duration of the V8 invocation. Same data, different
mechanism — kept separate so Surface (i) users don't pay the
task-local cost.

### 3.4 Spike — compile validation against today's API

Spike lives at `__inbox/346-zfb-embed-spike/` (gitignored, won't ship
in the PR). 100-line `src/lib.rs` plus a `Cargo.toml` that depends on
the workspace's `zfb-server` via relative path and excludes itself
from the workspace with an empty `[workspace]` table.

Per the advisor's guidance, the spike does **option (a)**: validate
that today's `ServeOpts` + `serve_with_listener` can be embedded
inside a third-party tokio runtime. It does NOT prototype the
proposed `Server::builder()` API because that would require editing
`crates/zfb-server/` (forbidden by the file-scope guardrail).

Public surface of the spike:

- `embed_zfb_server(project_root, dist_root, public_root, bind)`
  returns an `EmbeddedHandle { addr, pages, broadcast,
  shutdown_tx, join }` — an analog of the proposed `ServerHandle`.
- `smoke_test_blocking()` — drives the above end-to-end in a
  multi-threaded tokio runtime, captures the bound port.

Build result:

```
$ cd __inbox/346-zfb-embed-spike && cargo build
   ... (workspace deps already cached)
   Compiling zfb-embed-spike v0.0.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 00s

$ cargo test --quiet
running 1 test
.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
```

**Empirical conclusion:** today's `zfb-server` can already be
embedded into a foreign tokio runtime, bound to an ephemeral port,
and shut down cleanly via a oneshot signal. The proposed builder API
is ergonomic polish on top of an already-working substrate, not a
rescue of an unworkable one.

### 3.5 CCResDoc decision (principle-based)

This research worktree does NOT have CCResDoc source access — `ls
../` shows only the parallel research worktrees (r344–r349) and not
the CCResDoc repository. The recommendation is therefore
principle-based and conditional, not a verdict on CCResDoc's actual
routes.

**Principle:** Mode D embedding is most valuable for the
"Tauri-host-as-prod-server" case. If `ccresdoc-server`'s public
shape is essentially "axum router with bespoke routes, no zfb
file-based routing," then keeping it as-is and *not* migrating to
the embedded zfb server is the right call — there's no win from
forcing zfb's `pages/`-shaped routing onto a project that doesn't
follow that convention.

If, conversely, CCResDoc has zfb-shaped `pages/` and uses
`ccresdoc-server` only because there was no public embed API,
migrating to `zfb-server::Server::builder()` (with
`with_ssr_handler` for the Tauri-specific routes) deletes a fair
chunk of duplicated route-mount code.

The sibling investigation #349 has direct access to CCResDoc and can
make the call. From this side, the API sketch in §3.2 is the
contract: assume `.with_ssr_handler(matcher, handler)` and
`.with_request_extension(tauri_handle)` exist; #349 may reference
those signatures verbatim.

## 4. Conclusion

**Embeddability verdict — high confidence:** `zfb-server` is already
library-shaped and already embeddable. The spike at
`__inbox/346-zfb-embed-spike/` compiles and binds against today's
public API with no upstream changes. The blocker the issue worries
about ("crate is dev-only, needs production refactor") is not real —
the dev-flavoured surfaces (livereload, SSE) are confined to a
handful of routes that a `ServerMode` flag can gate cleanly.

**Builder API recommendation — medium confidence on shape, high
confidence on direction:** The 9-method sketch in §3.2 is the right
mental model; the exact spelling will move once the implementation
lands. Key commitments worth keeping fixed regardless:

- `Server::builder() -> ServerBuilder` entry point.
- `ServerMode { Dev, Preview, Embed }` flag, defaulting to `Dev` so
  existing `zfb dev` callers keep working.
- `with_request_extension<T>(value)` for Tauri context passthrough
  (Surface (i), axum `Extension`-based).
- `with_ssr_handler(pattern, handler)` for `prerender = false`
  Rust-side routes.
- `ServerHandle { addr(), shutdown(), join() }` returned from
  `serve_in_thread()`.

**Implementation cost — lower than the issue implied:** the bulk of
the work is renaming, adding a `Mode` enum, layering one
`Extension(value)` on the page route, and adding one `Router::route`
registration per `with_ssr_handler` call. No restructuring of the
route table itself.

## 5. Follow-ups

- **V8-side SSR per-request context (Surface ii).** Not in v1.
  Requires a tokio task-local + a native binding in `zfb-render`'s V8
  host that reads from it. Worth its own issue once Surface (i) has
  shipped.
- **Preview-mode replacement.** `zfb preview` today rebuilds its
  own router from scratch (`crates/zfb/src/commands/preview.rs`).
  Once `ServerMode::Preview` lands, the preview command can collapse
  to `Server::builder().mode(Preview).config_path(...).build()?...`
  — a noticeable LOC reduction. Worth tracking as a separate
  cleanup issue, not blocking on Mode D.
- **`ServeOpts` migration.** The current `ServeOpts` struct stays
  internal once `ServerBuilder` lands; one option is to keep
  `serve_with_listener(opts, listener, shutdown)` as a `#[doc(hidden)]`
  power-user escape hatch for the bin crate and tests, with all docs
  pointing at `Server::builder()`.
- **Plugin-middleware in Embed mode.** The proposed sketch
  silently disables plugin dev-middleware in `ServerMode::Embed`
  (because plugin host spawn is dev-only). Confirm with a test
  whether any embedder actually wants plugin middleware in prod-shaped
  embedding — if yes, expose `.with_plugins(set)` explicitly.
- **CCResDoc migration verdict.** Defer to #349. The contract
  surface this doc commits to is the §3.2 builder API; #349's
  recommendation about whether CCResDoc migrates depends on that
  side's route audit.

## 6. Scope exceptions

None. All edits stayed inside:

- `research/346-embed-as-library-api.md` (this file — primary
  deliverable).
- `__inbox/346-zfb-embed-spike/` (spike crate; `__inbox/` is
  gitignored so this is durable on disk but not in the PR).

No edits to `crates/zfb-server/` or any other workspace crate. The
spike's `Cargo.toml` explicitly opts itself out of the workspace via
an empty `[workspace]` table so the workspace root manifest stays
untouched.
