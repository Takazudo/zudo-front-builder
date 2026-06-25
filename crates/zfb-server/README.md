# zfb-server

The dev-mode HTTP server for `zudo-front-builder`: axum + SSE live-reload + CSS hot-swap.

`zfb-server` runs an [`axum`](https://docs.rs/axum) server that serves in-memory rendered HTML from
[`zfb-build`](../zfb-build)'s rebuild loop, built `dist/assets/` files, and per-request on-disk
fallbacks for `dist/` and `public/`. Every served HTML response has a small live-reload
`<script>` injected before `</body>`, which opens an SSE connection to `/__zfb/reload` and
listens for `page`, `css`, and `islands` events.

## Modules

| Module | Purpose |
| -------------------- | -------------------------------------------------------------------- |
| `embed` | `Server` / `ServerBuilder` / `ServerHandle` — embed-as-library API |
| `embed_handlers` | `EmbedHandler` / `EmbedHandlerSet` — Rust-side request handlers |
| `inject` | Byte-level live-reload `<script>` injector |
| `injected_routes` | `InjectedRouteSet` — pattern registry + request-time matcher for plugin-owned routes |
| `livereload` | SSE bridge: `ReloadEvent`, `outcome_to_events`, `sse_response` |
| `middleware` | Tower middleware for request extensions |
| `plugin_middleware` | `DevMiddlewareDispatcher` — plugin dev-middleware dispatch |
| `routes` | Axum router: `build_router`, `AppState`, `PageCache` |
| `ssr` | `SsrDispatcher` / `SsrRouteSet` — request-time SSR dispatch |

## Public API

Top-level re-exports from `lib.rs`:

```rust,ignore
use zfb_server::{
    // Entry points
    serve, serve_with_listener, ServeOpts,
    // Embed API
    Server, ServerBuilder, ServerHandle, ServerMode,
    // Live-reload
    outcome_to_events, IslandsBundleInfo, ReloadEvent, ReloadTx,
    // Routes / cache
    build_router, AppState, PageCache, CachedPage, DEV_404_BODY,
    content_type_for_extension, resolve_content_type,
    // Plugin middleware
    DevMiddlewareDispatcher, DevMiddlewareSet, PluginRegistration,
    PluginRequest, PluginResponse, PluginResponseEncoding,
    // Embed handlers
    EmbedHandler, EmbedHandlerFn, EmbedHandlerFuture, EmbedHandlerSet, RouteParams,
    // Injected routes (InjectedRoute is re-exported from zfb-build)
    InjectedRoute, InjectedRouteSet, pattern_matches,
    // SSR
    SsrDispatcher, SsrRequest, SsrResponse, SsrRouteRecord, SsrRouteSet, SsrRoutesHandle,
    // Inject helpers
    inject_livereload, inject_livereload_into_tree, LIVERELOAD_TAG,
    // Bundle URL handles
    IslandsBundleUrl, CssBundleUrl,
};
```

### Entry points

#### `serve` / `serve_with_listener`

```rust,ignore
pub async fn serve<S>(opts: ServeOpts, shutdown: S) -> anyhow::Result<()>
where
    S: Future<Output = ()> + Send + 'static;

pub async fn serve_with_listener<S>(
    opts: ServeOpts,
    listener: TcpListener,
    shutdown: S,
) -> anyhow::Result<()>
where
    S: Future<Output = ()> + Send + 'static;
```

`serve` binds `opts.addr` itself; `serve_with_listener` accepts a pre-bound listener (useful
in integration tests that bind `:0` and read the OS-chosen port via `TcpListener::local_addr`).
Pass `std::future::pending()` as `shutdown` to run until the process exits.

### `ServeOpts`

All paths must be absolute. Key fields:

| Field | Role |
| -------------------- | -------------------------------------------------------- |
| `addr` | Bind address (default `127.0.0.1:3000`) |
| `dist_root` | Built assets directory (`/assets/*` route root) |
| `html_root` | Dev-only HTML root (separate from `dist_root` in `zfb dev`) |
| `public_root` | Static files at site root (`public/logo.svg` → `/logo.svg`) |
| `pages` | `PageCache` populated by the orchestrator render loop |
| `broadcast` | `ReloadTx` — broadcast sender for SSE live-reload |
| `plugins` | Dev-middleware registrations from user plugins |
| `injected_routes` | Post-precedence `InjectedRouteSet` from plugins' `injectRoute` hooks; consulted by `lazy_render_adapter` to render static and dynamic injected routes in dev |
| `ssr_routes` | `SsrRoutesHandle` for `prerender = false` pages |
| `base` | `zfb.config.ts` `base` value (normalised internally) |
| `trailing_slash` | `zfb.config.ts` `trailingSlash` value |
| `mode` | `ServerMode::Dev` / `Preview` / `Embed` |
| `islands_bundle_url` | `Arc<RwLock<Option<String>>>` — current islands bundle URL |
| `css_bundle_url` | `Arc<RwLock<Option<String>>>` — current CSS bundle URL |

### Route map

| Route | Handler |
| ------------------------------------- | --------------------------------------------- |
| `GET /assets/*path` | Static files from `<dist_root>/assets/` |
| `GET /__zfb/livereload.js` | Bundled live-reload JS (dev only) |
| `GET /__zfb/reload` | SSE event stream (dev only) |
| `GET /` and `GET /*path` | Page cache → `dist/` → `public/` → 404 |

When `base: "/foo/"` is configured the entire table mounts under `/foo`.

### Live-reload SSE events

Three event types flow over `/__zfb/reload`:

| Event | Browser action | Payload |
| ---------- | ---------------------------------------- | ------- |
| `page` | `location.reload()` | none |
| `css` | Bump `<link>` query strings | none |
| `islands` | `import(bundleUrl)` + re-hydrate | `{ component, bundleUrl }` JSON |

`outcome_to_events(&BuildOutcome)` translates a build tick into this list:
pages written → `page`, CSS changed → `css`, islands bundle changed → one `islands` per
component. CSS or islands-only changes do **not** emit `page` so the browser hot-swaps
without losing client-side state.

### Embed-as-library API

`ServerBuilder` wraps the server for in-process Rust hosts (Tauri, an axum app, a CLI tool):

```rust,ignore
let handle = Server::builder()
    .config_path("/path/to/zfb.config.ts")
    .mode(ServerMode::Embed)
    .bind("127.0.0.1:0".parse()?)
    .build()
    .serve_in_thread()?;

let addr = handle.addr();
// … later …
handle.shutdown();
handle.join();
```

`serve_in_thread` spins up a dedicated OS thread with a `current_thread` tokio runtime so
the caller does not need an existing async context. The method budget on `Server` +
`ServerBuilder` is capped at 10 (per [research/346-embed-as-library-api.md](../../research/346-embed-as-library-api.md) §3.2).

## Design notes

### Dev-only caveat

This crate always injects the live-reload script, hard-codes `Cache-Control: no-store` on
HTML, and exposes `/__zfb/*`. Production builds emit static files served by an edge runtime
and must not pull in `zfb-server`.

### Feature gate

`zfb-server` carries the `embed_v8` feature to propagate the V8 gate to `zfb-build`. Its
own default features are off so workspace-wide feature unification does not force V8 on for
all consumers. The top-level `zfb` crate is the single place that decides V8 inclusion.

### How it plugs into `zfb-build`

The bin crate owns:

1. A [`zfb_build::BuildOrchestrator`](../zfb-build) (the rebuild loop).
2. A `tokio::sync::broadcast` channel of `ReloadEvent`s.
3. A `PageCache` of rendered HTML keyed by URL path.
4. This crate's `serve` task.

The bin crate wires the orchestrator's `on_outcome` callback to call `outcome_to_events` and
send the result into the broadcast channel. The server itself only reads the cache — it never
renders pages.

## Tests

```sh
cargo test -p zfb-server
```

Module-level unit tests live in `src/livereload.rs` (event-mapping rules and SSE payload
contract), `src/routes.rs`, and `src/inject.rs`. Integration tests in `tests/` boot a real
axum server on an ephemeral port and exercise full request paths including SSR dispatch and
watcher interplay.
