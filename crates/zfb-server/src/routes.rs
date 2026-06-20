//! Axum routes for the dev server.
//!
//! ## Route map
//!
//! - `GET /assets/*path` — static files from `<dist_root>/assets/`,
//!   served via [`tower_http::services::ServeDir`].
//! - `GET /__zfb/livereload.js` — bundled JS that opens an SSE
//!   connection back to this server. Always served with
//!   `Cache-Control: no-store`.
//! - `GET /__zfb/reload` — SSE event stream. See
//!   [`crate::livereload::sse_response`].
//! - `GET /` and `GET /*path` — render HTML out of the in-memory page
//!   cache, then fall back to `<dist_root>/...` and finally to
//!   `<public_root>/...` on disk before returning the dev 404. The
//!   `public/` directory has NO URL prefix — `public/logo.svg` is
//!   reachable at `/logo.svg`, matching the production layout
//!   `zfb build` produces (`copy_public_dir` copies straight into
//!   `dist/`).
//!
//! ## Page key / static-file resolution
//!
//! For a request to `/blog/foo` we look up the page cache in this order:
//!
//! 1. `/blog/foo`
//! 2. `/blog/foo/index.html`
//! 3. `/blog/foo/` (trailing slash — useful when the renderer keys by
//!    directory-style path)
//!
//! For a request to `/` we look up `/` and then `/index.html`. First
//! hit wins. If the cache misses, we then try `<dist_root>/<path>` and
//! `<dist_root>/<path>/index.html` on disk, then `<public_root>/<path>`
//! as a verbatim static-file read. Only after all three layers miss do
//! we return [`DEV_404_BODY`].
//!
//! Precedence (highest first): plugin dev-middleware → page cache →
//! dist directory → public directory → 404. A `pages/foo.tsx` route
//! therefore always wins over a same-named `public/foo` file.
//!
//! All HTML responses (including 404) go through
//! [`crate::inject::inject_livereload_with_prefix`] before being
//! returned, so every served page wires itself up to the live-reload
//! SSE stream — with the right URL when the dev server runs under a
//! `base` prefix.
//!
//! ## `base` prefix mounting (issue #229)
//!
//! When the project's `zfb.config.ts` declares `base: "/foo/"`, the
//! whole route table moves under `/foo`:
//!
//! - `GET /foo/` and `GET /foo/<route>` serve the rendered page (with
//!   the same `dist/` + `public/` disk fallback chain described above,
//!   so `public/logo.svg` is reachable at `/foo/logo.svg`),
//! - `GET /foo/assets/<file>` serves built static assets from
//!   `<dist_root>/assets/`,
//! - `GET /foo/__zfb/livereload.js` and `GET /foo/__zfb/reload` serve
//!   live-reload (and the injected `<script src>` matches),
//! - plugin-registered dev-middleware paths are auto-prefixed too
//!   (a plugin that calls `ctx.register("/api/echo")` is reached at
//!   `GET /foo/api/echo`; the plugin handler still receives
//!   `/api/echo` without the `base` prefix in `req.url`).
//!
//! Bare `GET /` redirects to `GET /<base>/` so a developer who
//! navigated to the root sees the home page rather than a confusing
//! 404. Other unprefixed paths fall through to a 404 body that hints
//! at the configured base. The redirect is only emitted when the
//! requested path is exactly `/` to avoid the `/foo/` → `/foo/` loop
//! the issue's review explicitly called out.
//!
//! When `base` is `None` or `"/"` the route table is identical to the
//! pre-`base` build byte-for-byte.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Component, PathBuf};
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::DefaultBodyLimit;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, Extensions, HeaderMap, HeaderValue, Method, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get};
use axum::Router;
use tokio::sync::RwLock;
use tower_http::trace::TraceLayer;
use zfb_types::escape_html;

use crate::assets_containment::ContainedAssetsService;
use crate::embed_handlers::EmbedHandlerSet;
use crate::inject::inject_livereload_with_prefix;
use crate::livereload::{sse_response, ReloadTx};
use crate::plugin_middleware::{
    DevMiddlewareSet, PluginDispatchOutcome, PluginRegistration, PluginRequest,
    PluginResponseEncoding,
};
use crate::ssr::{SsrRequest, SsrRouteSet};

/// HTML body returned when a page is not in the cache.
///
/// This is intentionally a dev-mode "did you forget to add a route?"
/// affordance — production builds emit static files and never hit this
/// path. It still gets the live-reload script injected so the tab will
/// auto-refresh once the missing page lands.
pub const DEV_404_BODY: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>zfb dev — 404</title></head><body><h1>404 — page not in cache</h1><p>The dev server has no rendered HTML for this URL. If you just added the page, the rebuild may still be in flight.</p></body></html>";

/// One cached page entry: the rendered body plus an optional explicit
/// `Content-Type`. When `content_type` is `None`, the server derives
/// one from the cache key's file extension via
/// [`content_type_for_extension`].
///
/// The body is stored as raw bytes (`Vec<u8>`) so binary responses
/// (images, fonts, WASM, …) can flow through the cache without
/// requiring valid UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedPage {
    /// Body to send back to the browser. Stored as raw bytes so binary
    /// responses (images, fonts, WASM, …) are supported without
    /// requiring UTF-8.
    pub body: Vec<u8>,
    /// Optional explicit `Content-Type` (typically supplied by a page's
    /// `export const contentType = "…"` frontmatter via Sub 1's
    /// extractor). `None` means "derive from the URL/extension".
    pub content_type: Option<String>,
}

impl CachedPage {
    /// Convenience: build an HTML cache entry from a `String` (or
    /// `&str`) with no explicit content-type override (the server will
    /// derive one).
    pub fn html(body: impl Into<String>) -> Self {
        Self {
            body: body.into().into_bytes(),
            content_type: None,
        }
    }

    /// Build a cache entry from raw bytes with no explicit content-type
    /// override. Use this for binary responses (images, fonts, WASM, …).
    pub fn bytes(body: impl Into<Vec<u8>>) -> Self {
        Self {
            body: body.into(),
            content_type: None,
        }
    }
}

/// In-memory cache of rendered pages keyed by URL path.
///
/// Keys are the leading-slash URL path the browser asked for, e.g.
/// `/`, `/blog/foo`, `/blog/foo/index.html`, `/sitemap.xml`. The bin
/// crate populates this from the orchestrator's render outputs.
///
/// Wrapped in an `Arc<RwLock<...>>` so route handlers can read
/// concurrently while the bin crate's rebuild loop holds a write
/// briefly to swap in fresh entries.
///
/// ## Non-HTML pages (Sub 49)
///
/// Each entry is a [`CachedPage`] carrying both the rendered body and
/// an optional `content_type` override. Pages that opt into non-HTML
/// output (e.g. `pages/sitemap.xml.tsx` or a feed page that sets
/// `export const contentType = "application/rss+xml"`) live in this
/// same cache with the appropriate URL key — the dev server reads
/// `content_type` to set the response header rather than always
/// returning `text/html`.
#[derive(Clone, Default)]
pub struct PageCache {
    inner: Arc<RwLock<HashMap<String, CachedPage>>>,
}

impl PageCache {
    /// Construct an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace an HTML entry. `key` should be the
    /// leading-slash URL path the browser will use (e.g. `/blog/foo`).
    /// Equivalent to [`insert_with_content_type`](Self::insert_with_content_type)
    /// with `content_type = None`.
    pub async fn insert(&self, key: impl Into<String>, body: impl Into<String>) {
        self.inner
            .write()
            .await
            .insert(key.into(), CachedPage::html(body));
    }

    /// Insert or replace an entry, optionally overriding the
    /// `Content-Type` the dev server uses for this URL. Pass `None`
    /// for `content_type` to let the server derive it from the URL's
    /// file extension.
    ///
    /// `body` is accepted as a `String` (or `&str`); for binary bodies
    /// use [`insert_bytes_with_content_type`](Self::insert_bytes_with_content_type).
    pub async fn insert_with_content_type(
        &self,
        key: impl Into<String>,
        body: impl Into<String>,
        content_type: Option<String>,
    ) {
        self.inner.write().await.insert(
            key.into(),
            CachedPage {
                body: body.into().into_bytes(),
                content_type,
            },
        );
    }

    /// Insert or replace an entry with a raw-bytes body, optionally
    /// overriding the `Content-Type`. Use this for binary responses
    /// (images, fonts, WASM, …) where the body is not valid UTF-8.
    pub async fn insert_bytes_with_content_type(
        &self,
        key: impl Into<String>,
        body: impl Into<Vec<u8>>,
        content_type: Option<String>,
    ) {
        self.inner.write().await.insert(
            key.into(),
            CachedPage {
                body: body.into(),
                content_type,
            },
        );
    }

    /// Replace the entire cache atomically with the contents of
    /// `entries`.
    pub async fn replace_all<I, K, V>(&self, entries: I)
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let new_map: HashMap<String, CachedPage> = entries
            .into_iter()
            .map(|(k, v)| (k.into(), CachedPage::html(v)))
            .collect();
        *self.inner.write().await = new_map;
    }

    /// Remove all entries whose key is in `keys`.
    ///
    /// Intended for invalidating stale routes when the dev pipeline prunes
    /// a globally-vanished output path (issue #804). Each `key` should
    /// match the leading-slash URL the entry was inserted under (e.g.
    /// `/blog/foo`). Unknown keys are silently ignored.
    pub async fn remove<I, K>(&self, keys: I)
    where
        I: IntoIterator<Item = K>,
        K: AsRef<str>,
    {
        let mut map = self.inner.write().await;
        for key in keys {
            map.remove(key.as_ref());
        }
    }

    /// Look up `key` and return the entry if present.
    pub async fn get(&self, key: &str) -> Option<CachedPage> {
        self.inner.read().await.get(key).cloned()
    }
}

/// Convert a URL path's trailing extension into a default
/// `Content-Type`.
///
/// The returned string is the value to plug into the response's
/// `Content-Type` header. The catch-all is `application/octet-stream`
/// so the browser doesn't sniff an unknown body as HTML and execute
/// scripts inside it. Pages that need an exotic extension should set
/// `export const contentType = "…"` in their frontmatter and rely on
/// the [`CachedPage::content_type`] override path instead.
///
/// Uses the [`mime_guess`] crate for the actual MIME lookup so the
/// table stays up-to-date without manual maintenance. A small set of
/// overrides is applied on top for extensions where `mime_guess` would
/// omit a required `charset` parameter or return a non-canonical
/// string for our use-case:
///
/// - `html`/`htm`, `txt`, `css`, `js`/`mjs`/`cjs` all get an explicit
///   `; charset=utf-8` suffix appended.
/// - `map` is canonicalised to `application/json` (source maps).
/// - `webmanifest` → `application/manifest+json`.
/// - `rss` → `application/rss+xml`.
/// - `atom` → `application/atom+xml`.
/// - `wasm` → `application/wasm` (mime_guess may return
///   `application/wasm` or miss it on older databases).
pub fn content_type_for_extension(extension: &str) -> String {
    // Hard-coded overrides take precedence — these are extensions where
    // mime_guess does not include a charset suffix we need, returns a
    // non-canonical type, or is absent from the database.
    let ext = extension.to_ascii_lowercase();
    match ext.as_str() {
        // Text types that need explicit charset.
        "html" | "htm" => return "text/html; charset=utf-8".to_string(),
        "txt" => return "text/plain; charset=utf-8".to_string(),
        "css" => return "text/css; charset=utf-8".to_string(),
        "js" | "mjs" | "cjs" => return "application/javascript; charset=utf-8".to_string(),
        // Generic XML: mime_guess returns "text/xml" but RFC 3023 /
        // RFC 7303 prefers "application/xml" for XML not intended as
        // a browser-rendered document. Keep the more correct type.
        "xml" => return "application/xml".to_string(),
        // Specialised XML subtypes not carried by the mime database.
        "rss" => return "application/rss+xml".to_string(),
        "atom" => return "application/atom+xml".to_string(),
        // Canonical JSON variants.
        "map" => return "application/json".to_string(),
        "webmanifest" => return "application/manifest+json".to_string(),
        // WASM (some older mime databases miss this).
        "wasm" => return "application/wasm".to_string(),
        _ => {}
    }

    // Delegate everything else to mime_guess. It handles images, fonts,
    // video/audio, PDF, and hundreds of other types without any
    // maintenance burden on our side.
    mime_guess::from_ext(&ext)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string()
}

/// Pick the `Content-Type` for a page-cache entry, applying the
/// documented precedence:
///
/// 1. The entry's `content_type` override, if `Some`,
/// 2. else [`content_type_for_extension`] applied to `url_path`'s
///    trailing extension,
/// 3. else `application/octet-stream` (the catch-all of
///    [`content_type_for_extension`]).
///
/// `url_path` is the URL the page was served at (with or without a
/// leading slash); only its trailing component matters for the
/// extension lookup.
pub fn resolve_content_type(entry: &CachedPage, url_path: &str) -> String {
    if let Some(ct) = &entry.content_type {
        return ct.clone();
    }
    let basename = url_path.rsplit('/').next().unwrap_or(url_path);
    let ext = basename.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    if ext.is_empty() {
        // Pages served at extensionless URLs (`/about`, `/`) are HTML
        // by convention. `content_type_for_extension`'s fallback is
        // `application/octet-stream` (safer for unknown asset types),
        // so we hard-code the HTML default here instead of leaning on
        // the helper's fallback.
        return "text/html; charset=utf-8".to_string();
    }
    content_type_for_extension(ext)
}

/// Shared state for the route handlers.
#[derive(Clone)]
pub struct AppState {
    /// Server mode — gates Dev-only behaviour at request time
    /// (live-reload script injection, `/__zfb/reload` SSE endpoint,
    /// `/__zfb/livereload.js` script endpoint, default
    /// `Cache-Control: no-store` on HTML). Defaults to
    /// [`crate::ServerMode::Dev`] for parity with the historical
    /// behaviour of `serve_with_listener` callers (the `zfb dev` and
    /// integration-test paths). The embed builder threads its own
    /// `mode` value through verbatim.
    pub mode: crate::ServerMode,
    /// Page cache that renderers populate.
    pub pages: PageCache,
    /// Live-reload broadcast sender — cloned per SSE subscription.
    pub broadcast: ReloadTx,
    /// Optional plugin dev-middleware set. `None` when no user plugins
    /// declared a `devMiddleware` hook.
    pub plugins: Option<DevMiddlewareSet>,
    /// Optional plugin-injected synthetic-route set (#255). `None`
    /// when no plugin called `injectRoute` from the new `setup`
    /// hook. The dev router checks this on every page-cache miss
    /// to surface the matched entrypoint (full evaluation through
    /// the page renderer lands in a follow-up).
    pub injected_routes: Option<crate::injected_routes::InjectedRouteSet>,
    /// Request-time SSR routes (issue #367 / Gap 1, live update issue #807).
    /// `None` when the project has no `prerender = false` pages. When `Some`,
    /// the page handler dispatches matched URLs through this set BEFORE
    /// consulting the page cache — see the precedence contract on
    /// [`crate::ssr`] for the full ordering.
    ///
    /// Wrapped in `Arc<RwLock<Option<SsrRouteSet>>>` so the per-tick
    /// renderer reload can swap in a fresh route set mid-session.
    pub ssr_routes: Option<crate::ssr::SsrRoutesHandle>,
    /// Embed-API handler set populated by
    /// [`crate::ServerBuilder::with_ssr_handler`] (#372). `None` in the
    /// `zfb dev` / `zfb preview` paths; `Some` when a Rust host
    /// registered one or more handlers. Dispatched in [`serve_page`]
    /// after plugin dev-middleware but before the SSR dispatcher, so a
    /// handler claiming the same path as a runtime-rendered page wins.
    pub embed_handlers: Option<EmbedHandlerSet>,
    /// Build output directory. Used for `/assets/*` serving (mounted at
    /// `<dist_root>/assets/`). The HTML page-cache disk fallback lives
    /// on [`html_root`](Self::html_root) instead so dev's per-route
    /// renders can target a separate directory from the production
    /// `outDir` (issue #534).
    pub dist_root: std::path::PathBuf,
    /// On-disk page root used as the page-cache fallback in
    /// `serve_page`. `<html_root>/<path>/index.html` and
    /// `<html_root>/<path>` are probed when the in-memory cache misses
    /// (issue #534). For preview / embed callers this is the same as
    /// [`dist_root`](Self::dist_root); for `zfb dev` this points at a
    /// dev-only directory so dev no longer overwrites the production
    /// build output `pnpm preview` later serves.
    pub html_root: std::path::PathBuf,
    /// Project public-static directory. Used as a final on-disk fallback
    /// after the page cache and dist directory miss, so that user files
    /// in `public/` are reachable at the site root (e.g.
    /// `public/logo.svg` → `/logo.svg`). Matches the production layout
    /// that `zfb build` produces by copying `public/*` straight into
    /// `dist/`.
    pub public_root: std::path::PathBuf,
    /// Canonical mount prefix from `zfb.config.ts`'s `base` field, as
    /// returned by [`zfb_types::dev_mount_prefix`]. `None` means the
    /// dev server mounts everything at root (the no-base case);
    /// `Some("/foo")` (leading slash, no trailing slash) means the
    /// page handlers serve content under `/foo/...` and the injected
    /// live-reload script URL points at `/foo/__zfb/livereload.js`.
    pub base_prefix: Option<String>,
    /// Mirror of `zfb.config.ts`'s `trailingSlash` field. When true,
    /// the in-flight base-rewrite pass (sub #234 /
    /// zudolab/zudo-doc#1579) appends `/` to extensionless absolute
    /// hrefs after prefixing, so dev preview matches the canonical
    /// trailing-slash URL shape that `zfb build` emits to disk.
    pub trailing_slash: bool,

    /// Optional shared handle to the current dev-mode islands bundle URL
    /// (issue #377). When `Some` and the inner lock holds `Some(url)`,
    /// every served HTML response in [`crate::ServerMode::Dev`] mode
    /// has a `<script type="module" src="<url>"></script>` spliced into
    /// `<head>` via [`zfb_build::head_inject::inject_prod_head_assets`].
    /// `None` (outer) or `None` inside the lock both fall back to "no
    /// injection" — projects without `"use client"` components must
    /// not ship a script tag pointing at a non-existent bundle.
    ///
    /// Gated to Dev mode at the response-shaping site: Preview / Embed
    /// callers never inject even if they accidentally pass a non-`None`
    /// handle, so the production-shaped response contract is preserved.
    pub islands_bundle_url: Option<crate::IslandsBundleUrl>,

    /// Optional shared handle to the current dev-mode CSS bundle URL
    /// (issue #494 / #498). When `Some` and the inner lock holds `Some(url)`,
    /// every served HTML response in [`crate::ServerMode::Dev`] mode
    /// has a `<link rel="stylesheet" href="<url>">` spliced into
    /// `<head>` via [`zfb_build::head_inject::inject_prod_head_assets`].
    /// `None` (outer) or `None` inside the lock both fall back to "no
    /// injection" — projects with Tailwind disabled must not ship a
    /// link tag pointing at a non-existent file.
    ///
    /// Gated to Dev mode at the response-shaping site: Preview / Embed
    /// callers never inject even if they accidentally pass a non-`None`
    /// handle.
    pub css_bundle_url: Option<crate::CssBundleUrl>,

    /// Host-header / Origin allowlist state (issue #931 / #919). Built
    /// from the listener's actual bound address at serve time —
    /// loopback binds carry a disabled validator (allow-everything), so
    /// the default `localhost` setup sees zero behaviour change.
    /// [`build_router`] applies the Host-header layer from this field
    /// (covering BOTH the no-base and base-prefixed construction
    /// branches); [`serve_page`] consults it for the Origin check on
    /// non-GET requests reaching plugin/embed/SSR dispatch.
    pub host_validation: crate::host_validation::HostValidation,

    /// Optional render-on-request hook (issue #1020).
    ///
    /// When `Some` and `mode == ServerMode::Dev`, `serve_page` awaits
    /// this hook on every GET/HEAD request **before** the in-memory page
    /// cache lookup. The hook's job is to ensure `html_root` is fresh;
    /// after it returns the normal `PageCache → html_root → public_root`
    /// waterfall continues unchanged.
    ///
    /// `None` disables the hook (Preview/Embed/tests without a hook); all
    /// existing legs are byte-identical. Snapshotted under a short read
    /// lock and released before the `await` — same discipline as
    /// `ssr_routes`.
    pub render_on_request_hook: Option<crate::render_hook::RenderOnRequestHandle>,
}

/// Build the axum router for the dev server.
///
/// `state.dist_root` is the build output directory (used for
/// `/assets/*` and as the first on-disk fallback for page misses).
/// `state.public_root` is the project's static assets directory; it
/// is consulted as a per-request on-disk fallback inside [`serve_page`]
/// (no top-level `/public/*` mount — files there appear at the site
/// root, mirroring `zfb build`'s `public/` → `dist/` copy).
///
/// ## Method policy (issue #230)
///
/// Built-in routes keep their existing GET-only semantics:
///
/// - `<base>/__zfb/livereload.js`, `<base>/__zfb/reload`,
///   `<base>/assets/*` are all registered with `get(...)` or via
///   [`ServeDir`] (which is GET/HEAD-only). A non-GET request to any
///   of these surfaces gets a `405 Method Not Allowed` from axum /
///   tower-http directly — those routes are dev-server infrastructure,
///   not user code, and CSRF posture for them must stay tight.
/// - The page-renderer mounts accept ALL methods so user-registered
///   `devMiddleware` handlers can serve `POST`, `PUT`, `DELETE`,
///   `PATCH`, etc. on prefixes they claim. The handler itself in
///   [`serve_page`] still returns `405 Allow: GET, HEAD` if a non-GET
///   request slips through to the page-cache fallback (i.e. no plugin
///   claimed the URL or a plugin returned `Passthrough`). The
///   `public/` on-disk fallback inside `serve_page` is also reached
///   only on the GET/HEAD path, so `POST /favicon.ico` 405s the same
///   way it always did.
///
/// ## Base prefix mounting (issue #229)
///
/// When `state.base_prefix` is `Some(prefix)`, every route is
/// registered at `<prefix>/<route>` directly (livereload, SSE,
/// `/assets/*`, page cache, plugin middleware). A bare `GET /` request
/// is redirected to `<prefix>/`, and any other unprefixed path falls
/// through to a 404 with a one-line hint at the configured base. See
/// the module docs for the complete contract. The `public/` on-disk
/// fallback inside `serve_page` is reached after the page-cache miss,
/// so `public/logo.svg` is served at `<prefix>/logo.svg` just like the
/// production build emits.
///
/// Routes are registered with explicit prefix substitution rather than
/// [`Router::nest`] because the latter has subtle 0.8.x trailing-slash
/// quirks when the inner router contains both a literal `/` route and
/// a `/{*path}` catch-all (the very shape we use). Manual prefix
/// registration is verbose but uniquely well-defined.
pub fn build_router(state: AppState) -> Router {
    let host_validation = state.host_validation.clone();
    let prefix = state.base_prefix.clone();

    let router = match prefix {
        // No base prefix configured — keep the byte-for-byte
        // pre-`base` route table at the root.
        None => build_core_router(state, ""),
        Some(prefix) => {
            // Prefix is canonical: leading slash, no trailing slash (e.g.
            // "/foo"). Build the core route table with the prefix folded into
            // every path, then add the bare `/` redirect and the
            // outside-base 404 fallback.
            let redirect_target = format!("{prefix}/");
            let prefix_for_404 = prefix.clone();
            let mode_for_404 = state.mode;

            build_core_router(state, &prefix)
                // Bare `/` lands the developer on the home page — but only `/`
                // exactly, never `<prefix>/...`, because the prefixed routes
                // above already catch those and a redirect there would loop.
                // The Uri is extracted so any query string on `/?x=1` is carried
                // through to `<prefix>/?x=1` rather than silently dropped.
                .route(
                    "/",
                    get(move |uri: Uri| {
                        let target = redirect_target.clone();
                        async move {
                            let target = match uri.query() {
                                Some(q) if !q.is_empty() => format!("{target}?{q}"),
                                _ => target,
                            };
                            Redirect::to(&target).into_response()
                        }
                    }),
                )
                // Any other unprefixed path (e.g. an HTML link that forgot the
                // base, or a stale browser cache) gets a 404 with a one-line
                // hint at the configured base. The body is HTML so the
                // live-reload script can pick it up — the 404 disappears once
                // the developer follows the hint.
                .fallback(get(move |uri: Uri| {
                    let prefix = prefix_for_404.clone();
                    async move { unprefixed_404_response(&prefix, uri.path(), mode_for_404) }
                }))
        }
    };

    // Issue #931: the Host-header allowlist layer is applied HERE,
    // after the two construction branches merge, so the no-base AND the
    // base-prefixed route tables are both protected — applying it
    // inside one branch only would pass the localhost smoke test while
    // leaving base-prefixed deployments exposed. No-op (router returned
    // unchanged) when the validator is not enforcing (loopback bind).
    // TraceLayer wraps outermost so rejected requests still get traced.
    crate::host_validation::apply_host_validation_layer(router, host_validation)
        // 2 MiB cap: generous enough for any legitimate dev-middleware
        // POST payload, prevents unbounded memory buffering on the page
        // routes that extract `body: Bytes` with no size guard.
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
}

/// Assemble the core route table — every route prefixed with `prefix`
/// (use `""` for the no-base case so paths stay at the root).
///
/// Plugin dispatch + cache lookup expect UNPREFIXED paths (issue #229
/// contract: a plugin that calls `ctx.register("/api/echo")` is reached
/// at `<prefix>/api/echo` but the handler still sees `req.url =
/// /api/echo`). The page handlers below strip the prefix from the
/// captured wildcard / URI before forwarding into [`serve_page`].
fn build_core_router(state: AppState, prefix: &str) -> Router {
    let assets_dir = state.dist_root.join("assets");
    // Wrap ServeDir with symlink containment: any request whose resolved
    // FS path escapes `assets_dir` (including via symlinks) is rejected
    // with 404 before ServeDir sees it.  See `assets_containment` module.
    let assets_service = ContainedAssetsService::new(assets_dir);

    let livereload_path = format!("{prefix}/__zfb/livereload.js");
    let sse_path = format!("{prefix}/__zfb/reload");
    let assets_mount = format!("{prefix}/assets");
    let root_path = format!("{prefix}/");
    let wild_path = format!("{prefix}/{{*path}}");

    // Live-reload surface (`/__zfb/livereload.js` SSE script,
    // `/__zfb/reload` SSE stream) is Dev-only. In Preview/Embed modes
    // these endpoints stay unmounted so an embedder doesn't accidentally
    // expose dev-server infrastructure on a production-shaped server.
    // The HTML body never injects the script in those modes either —
    // see [`page_response_bytes`] for the response-shaping side of the
    // gate.
    let is_dev = matches!(state.mode, crate::ServerMode::Dev);

    let mut router = Router::new();
    if is_dev {
        router = router
            .route(&livereload_path, get(livereload_js))
            .route(&sse_path, get(sse_handler));
    }
    router
        .nest_service(&assets_mount, assets_service)
        // `any` (vs `get`) on the page-renderer mounts so user-registered
        // devMiddleware handlers can serve every HTTP method (zfb#230).
        // `serve_page` enforces GET/HEAD-only for the page-cache fallback
        // when no plugin claims the URL.
        .route(&root_path, any(page_root))
        .route(&wild_path, any(page_handler))
        .with_state(state)
}

/// Build the dev 404 served when a request lands outside the configured
/// `base` prefix. Mirrors [`DEV_404_BODY`] but appends a one-line hint
/// pointing at the prefix so the developer notices the typo / forgotten
/// base instead of staring at a blank "page not in cache" body.
///
/// The hint is HTML-escaped — `prefix` and `path` come from request
/// data and could otherwise smuggle markup into the response.
/// Read the current dev-mode islands bundle URL from the shared
/// state handle, if any. Returns the locked string by clone so the
/// caller can hold the result across the response build without
/// keeping the read lock alive. A poisoned lock (writer panicked) is
/// recovered into a non-poisoned guard rather than re-panicking — a
/// dev-server crash on a stale lock would be the worst possible UX
/// for a feature whose whole purpose is to make islands "just work"
/// in dev.
fn current_islands_bundle_url(handle: &Option<crate::IslandsBundleUrl>) -> Option<String> {
    let arc = handle.as_ref()?;
    let guard = arc.read().unwrap_or_else(|p| {
        tracing::warn!(
            site = "AppState.islands_bundle_url",
            "rwlock poisoned, recovered"
        );
        p.into_inner()
    });
    guard.clone()
}

/// Read the current dev-mode CSS bundle URL from the shared state
/// handle, if any. Mirrors [`current_islands_bundle_url`] for CSS.
/// Returns the locked string by clone so the caller can hold the result
/// across the response build without keeping the read lock alive. A
/// poisoned lock is recovered rather than re-panicking.
fn current_css_bundle_url(handle: &Option<crate::CssBundleUrl>) -> Option<String> {
    let arc = handle.as_ref()?;
    let guard = arc.read().unwrap_or_else(|p| {
        tracing::warn!(
            site = "AppState.css_bundle_url",
            "rwlock poisoned, recovered"
        );
        p.into_inner()
    });
    guard.clone()
}

fn unprefixed_404_response(prefix: &str, path: &str, mode: crate::ServerMode) -> Response {
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>zfb dev — 404 (base mismatch)</title></head><body><h1>404 — outside configured base</h1><p>This dev server is mounted under <code>{}</code> (from <code>base</code> in <code>zfb.config.ts</code>). The path <code>{}</code> is not under that prefix. Try <a href=\"{}/\">{}/</a> instead.</p></body></html>",
        escape_html(prefix),
        escape_html(path),
        escape_html(prefix),
        escape_html(prefix),
    );
    page_response_bytes(
        StatusCode::NOT_FOUND,
        body.into_bytes(),
        "text/html; charset=utf-8",
        // Inject the live-reload script with the prefix so the
        // 404 page silently upgrades when the developer fixes the URL
        // and the matching page becomes reachable. `page_response_bytes`
        // will silently drop the injection when `mode != Dev`.
        true,
        prefix,
        // Static 404 body — its `href="{prefix}/"` already ends in `/`,
        // so the trailing-slash post-process is a no-op either way.
        false,
        mode,
        // The unprefixed 404 page is a static body with no `<head>`
        // anchor, so the islands splicer would no-op anyway. Pass None
        // to keep the surface tight — this handler has no AppState in
        // scope.
        None,
        // Same reasoning as islands: no AppState in scope, pass None.
        None,
    )
}

/// Handler for `GET /__zfb/livereload.js`. Returns the bundled JS with
/// `Cache-Control: no-store` so dev tabs always refetch the latest
/// build of the script when reloaded.
pub async fn livereload_js() -> impl IntoResponse {
    let body = include_str!("livereload.js");
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/javascript; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        body,
    )
}

/// Handler for `GET /__zfb/reload`. Opens an SSE stream for this
/// connection.
pub async fn sse_handler(State(state): State<AppState>) -> impl IntoResponse {
    sse_response(&state.broadcast)
}

/// Handler for `/` — serve the root page or dispatch to a plugin
/// dev-middleware that claims the root prefix. Accepts every HTTP
/// method (see [`build_router`] for the method policy); GET/HEAD
/// fall through to the page cache, other methods 405 unless a plugin
/// handles them.
pub async fn page_root(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
    extensions: Extensions,
    body: Bytes,
) -> Response {
    serve_page(&state, "/", &uri, method, headers, extensions, body).await
}

/// Handler for `/*path` — serve any other rendered page or dispatch
/// to a plugin dev-middleware. See [`page_root`] for the method
/// policy.
pub async fn page_handler(
    State(state): State<AppState>,
    AxumPath(path): AxumPath<String>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
    extensions: Extensions,
    body: Bytes,
) -> Response {
    serve_page(&state, &path, &uri, method, headers, extensions, body).await
}

async fn serve_page(
    state: &AppState,
    raw_path: &str,
    uri: &Uri,
    method: Method,
    headers: HeaderMap,
    extensions: Extensions,
    body: Bytes,
) -> Response {
    // Strip any leading slash from the captured wildcard so we can
    // build canonical lookup keys ourselves.
    let trimmed = raw_path.trim_start_matches('/');
    // Mount-prefix-aware livereload injection (issue #229). The
    // `<script src>` we splice into HTML responses must point at the
    // prefixed URL the dev server actually serves the JS at, otherwise
    // the browser would request `/__zfb/livereload.js` (unprefixed) and
    // hit the dev 404. Empty when no `base` is configured.
    let lr_prefix = state.base_prefix.as_deref().unwrap_or("");

    // The page-cache fallback below only handles GET/HEAD. Track this
    // so a non-GET request that does not match any plugin (or hits a
    // plugin that returns Passthrough) is rejected with `405 Allow:
    // GET, HEAD` instead of falling through to the cache logic.
    let is_get_like = matches!(method, Method::GET | Method::HEAD);

    // Sub 3 / #108 — plugin dev-middleware takes priority over the
    // page cache so a plugin can override a URL that also has a
    // `pages/foo.tsx`. Handlers may signal "passthrough" if they
    // decide not to handle the request, in which case we fall back
    // to the cache lookup below. Forward the full request URI
    // (including query string) so the plugin can implement
    // `?since=<timestamp>` etc.
    //
    // Issue #230: every HTTP method (including POST/PUT/DELETE/PATCH)
    // is forwarded; the plugin handler decides whether to accept or
    // reject the method. Built-in routes (`/__zfb/...`, `/assets/*`)
    // are unaffected — those are routed by axum directly and stay
    // GET-only. The `public/` on-disk fallback further down only
    // runs on the GET/HEAD path, so `POST /favicon.ico` still 405s.
    //
    // Issue #229 contract: plugins register UNPREFIXED paths
    // (`ctx.register("/api/echo")`); the dev server registers the
    // route at `<base_prefix>/api/echo` but strips the prefix before
    // invoking the handler. `trimmed` already drops the prefix because
    // the route pattern includes it (the wild `{*path}` only captures
    // what comes after); for `uri.path_and_query()` we strip the
    // prefix manually so the plugin sees the unprefixed URL.
    if let Some(set) = state.plugins.as_ref() {
        let path_only = format!("/{trimmed}");
        if let Some(reg) = set.find_match(&path_only) {
            // Issue #931: cross-origin non-GET requests must not reach
            // plugin handlers when the server is LAN-exposed.
            if let Some(resp) = origin_rejection(state, &method, &headers) {
                return resp;
            }
            // Path + optional query, with the dev server's mount
            // prefix stripped so the plugin handler sees the URL
            // shape it registered.
            let full = strip_prefix_from_full_uri(uri, state.base_prefix.as_deref())
                .unwrap_or_else(|| path_only.clone());
            let plugin_headers = headermap_to_string_map(&headers);
            let plugin_body = body_bytes_to_utf8_string(&body);
            match dispatch_plugin(
                set,
                reg,
                &full,
                method.as_str(),
                plugin_headers,
                plugin_body,
                state.mode,
            )
            .await
            {
                PluginDispatchAttempt::Responded(resp) => return resp,
                PluginDispatchAttempt::Passthrough => {}
                PluginDispatchAttempt::Errored(resp) => return resp,
            }
        }
    }

    // Issue #372 — Rust-side handlers registered through the embed API
    // (`ServerBuilder::with_ssr_handler`). Dispatched after plugin
    // dev-middleware (plugins always win) but before request-time SSR,
    // so a Rust handler claiming a path that also has a JS
    // runtime-rendered page short-circuits the JS dispatch. The handler
    // signature is HTTP-shaped and domain-agnostic — see
    // [`crate::embed_handlers`] for the dispatch contract.
    if let Some(set) = state.embed_handlers.as_ref() {
        let path_only = format!("/{trimmed}");
        if let Some((handler, params)) = set.find_match(&path_only) {
            // Issue #931: same Origin gate as the plugin leg above.
            if let Some(resp) = origin_rejection(state, &method, &headers) {
                return resp;
            }
            return dispatch_embed_handler(
                handler,
                params,
                uri,
                &method,
                &headers,
                &extensions,
                body.clone(),
                state.base_prefix.as_deref(),
                state.mode,
            )
            .await;
        }
    }

    // Issue #367 (Gap 1) — request-time SSR for `prerender = false`
    // pages. Slots in between plugin dev-middleware and the page-cache
    // fallback so plugins keep their override capability, but pages
    // that opted out of prerender always reach the V8 host instead of
    // falling through to a stale dist snapshot. Like the plugin layer
    // we accept every HTTP method here — the page's `fetch` handler
    // decides whether to allow `POST`/`PUT`/etc. (mirroring Cloudflare).
    //
    // Issue #807: `ssr_routes` is now a live handle (`Arc<RwLock<...>>`).
    // Read a snapshot of the current route set under a short-lived read
    // lock — the lock is released before any I/O so the writer (per-tick
    // reload) is never blocked by in-flight requests.
    if let Some(handle) = state.ssr_routes.as_ref() {
        let set_snapshot: Option<crate::ssr::SsrRouteSet> =
            handle.read().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(set) = set_snapshot {
            let path_only = format!("/{trimmed}");
            if set.find_match(&path_only).is_some() {
                // Issue #931: same Origin gate as the plugin leg above.
                if let Some(resp) = origin_rejection(state, &method, &headers) {
                    return resp;
                }
                // Strip the dev server's mount prefix from the URL before
                // dispatching so the SSR handler sees the same shape
                // Cloudflare delivers in production. Without this fix a
                // request to `/<base>/dynamic?x=1` would reach the V8 host
                // as `/<base>/dynamic?x=1`, diverging from prod where the
                // adapter serves the route at `/dynamic?x=1`.
                let full = strip_prefix_from_full_uri(uri, state.base_prefix.as_deref())
                    .unwrap_or_else(|| path_only.clone());
                return dispatch_ssr(
                    &set,
                    &full,
                    &method,
                    &headers,
                    &body,
                    lr_prefix,
                    state.trailing_slash,
                    state.mode,
                    current_islands_bundle_url(&state.islands_bundle_url).as_deref(),
                    current_css_bundle_url(&state.css_bundle_url).as_deref(),
                )
                .await;
            }
        }
    }

    // No plugin handled this request. The dev page-cache fallback is
    // GET/HEAD-only — anything else 405s here so a misrouted POST does
    // not silently get treated as a page lookup.
    if !is_get_like {
        return method_not_allowed_get_head();
    }

    // Issue #1020 — render-on-request hook. Dev + GET/HEAD only (gated
    // above). The hook makes `html_root` fresh as a side effect; after
    // it returns the request falls through to the existing PageCache →
    // html_root → public_root waterfall unchanged.
    //
    // Threading discipline mirrors the SSR leg (routes.rs:867-874):
    // snapshot the inner `Arc` under a short read lock, release the lock
    // before `await`ing so a concurrent session reload is never blocked
    // by an in-flight render.
    //
    // Error containment: the hook is spawned as a separate task so that
    // a panic in the hook is caught by tokio and surfaced as a `JoinError`
    // rather than unwinding the request handler. The handler logs the
    // error and falls through to the existing disk/cache legs — the
    // contract is best-effort: the hook either makes the disk fresh or
    // does not, and the server serves whatever is there.
    if matches!(state.mode, crate::ServerMode::Dev) {
        if let Some(handle) = state.render_on_request_hook.as_ref() {
            let hook_snapshot: Option<std::sync::Arc<dyn crate::render_hook::RenderOnRequestHook>> =
                handle.read().unwrap_or_else(|p| p.into_inner()).clone();
            if let Some(hook) = hook_snapshot {
                // Use the prefix-stripped path so the hook sees the same
                // URL shape the production adapter delivers (`/blog/hello`,
                // not `/<base>/blog/hello`).
                let url_path = strip_prefix_from_full_uri(uri, state.base_prefix.as_deref())
                    .map(|s| {
                        // strip_prefix_from_full_uri returns path-and-query;
                        // drop the query string for the hook.
                        s.split_once('?').map(|(p, _)| p.to_string()).unwrap_or(s)
                    })
                    .unwrap_or_else(|| format!("/{trimmed}"));
                // Spawn so a panic in the hook doesn't unwind this handler
                // task — the JoinError is caught and logged; we fall through
                // to the existing legs either way.
                let join = tokio::spawn(async move {
                    hook.render_if_stale(&url_path).await;
                });
                if let Err(e) = join.await {
                    tracing::warn!(
                        url_path = %trimmed,
                        error = %e,
                        "render-on-request hook failed (continuing with fallback legs)",
                    );
                }
            }
        }
    }

    let candidates = lookup_keys(trimmed);
    for key in &candidates {
        if let Some(entry) = state.pages.get(key).await {
            // Pick the Content-Type per the precedence rule:
            //   override on the cache entry > URL extension default
            //   > text/html. The cache key wins over `raw_path`
            //   because the matched key reflects what the renderer
            //   actually emitted (e.g. `/sitemap.xml`).
            let content_type = resolve_content_type(&entry, key);
            let is_html = content_type.to_ascii_lowercase().starts_with("text/html");
            return page_response_bytes(
                StatusCode::OK,
                entry.body,
                &content_type,
                is_html,
                lr_prefix,
                state.trailing_slash,
                state.mode,
                current_islands_bundle_url(&state.islands_bundle_url).as_deref(),
                current_css_bundle_url(&state.css_bundle_url).as_deref(),
            );
        }
    }

    // #255 — plugin-injected synthetic routes. After the page cache
    // miss but before the dist / public fallbacks we consult the
    // injected-route registry. A hit means a plugin claimed this URL
    // pattern via `injectRoute(pattern, entrypoint)`. The registry
    // plumbing is the deliverable for #255; full evaluation of the
    // matched entrypoint through the page renderer is a follow-up.
    // Today we emit a structured log so the user can confirm the
    // injection landed end-to-end, and fall through to the existing
    // dist / public fallback (which will 404 if no other file claims
    // the URL — exactly the same shape as before, plus the log).
    let path_for_inject = format!("/{trimmed}");
    if let Some(set) = state.injected_routes.as_ref() {
        if let Some(rec) = set.find_match(&path_for_inject) {
            // Use the same tracing target the dev middleware uses so
            // plugin diagnostics cluster together in dev output.
            tracing::info!(
                target: "zfb_plugin",
                plugin = %rec.plugin,
                pattern = %rec.pattern,
                entrypoint = %rec.entrypoint.display(),
                url = %path_for_inject,
                "injectRoute matched (renderer integration is a follow-up)",
            );
        }
    }

    // In-memory cache miss: fall back to reading rendered HTML from
    // disk. Today the dev pipeline doesn't actively populate
    // `state.pages` for HTML — the dev server serves nearly every page
    // via this disk path, with the renderer writing to `html_root` on
    // every watcher tick. Issue #534: `html_root` is a separate
    // directory from `dist_root` so dev's writes never overwrite the
    // production output. For preview / embed callers `html_root` and
    // `dist_root` point at the same directory, so behaviour is
    // unchanged there.
    if let Some(bytes) = read_from_dist(&state.html_root, trimmed).await {
        // Mirror the cached-path content-type derivation. Hardcoding
        // `text/html` here used to splice a livereload `<script>` tag
        // into XML feeds (`/sitemap.xml`, `/atom.xml`) and serve them
        // with the wrong Content-Type whenever the in-memory cache was
        // cold — breaking feed readers and XML parsers.
        let basename = trimmed.rsplit('/').next().unwrap_or(trimmed);
        let ext = basename.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        let content_type = if ext.is_empty() {
            "text/html; charset=utf-8".to_string()
        } else {
            content_type_for_extension(ext)
        };
        let is_html = content_type.to_ascii_lowercase().starts_with("text/html");
        return page_response_bytes(
            StatusCode::OK,
            bytes,
            &content_type,
            is_html,
            lr_prefix,
            state.trailing_slash,
            state.mode,
            current_islands_bundle_url(&state.islands_bundle_url).as_deref(),
            current_css_bundle_url(&state.css_bundle_url).as_deref(),
        );
    }

    // Final on-disk fallback: project `public/` directory. The
    // production build copies `public/*` straight into `dist/` (see
    // `crates/zfb/src/commands/build.rs::copy_public_dir`), so files
    // there must also appear at the site root in dev — otherwise
    // `<img src="/logo.svg">` referencing `public/logo.svg` 404s in
    // dev but works after `zfb build`, which is the bug this lookup
    // is designed to prevent. Page cache and dist take precedence
    // (above), so a same-named `pages/foo.tsx` route always wins over
    // `public/foo`.
    if !trimmed.is_empty() {
        if let Some(bytes) = read_from_public(&state.public_root, trimmed).await {
            let basename = trimmed.rsplit('/').next().unwrap_or(trimmed);
            let ext = basename.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
            let content_type = if ext.is_empty() {
                "application/octet-stream".to_string()
            } else {
                content_type_for_extension(ext)
            };
            let is_html = content_type.to_ascii_lowercase().starts_with("text/html");
            return page_response_bytes(
                StatusCode::OK,
                bytes,
                &content_type,
                is_html,
                lr_prefix,
                state.trailing_slash,
                state.mode,
                current_islands_bundle_url(&state.islands_bundle_url).as_deref(),
                current_css_bundle_url(&state.css_bundle_url).as_deref(),
            );
        }
    }

    // 404 is always the dev HTML body so the page is replaced once
    // a real one lands. The live-reload script gets injected.
    page_response_bytes(
        StatusCode::NOT_FOUND,
        DEV_404_BODY.as_bytes().to_vec(),
        "text/html; charset=utf-8",
        true,
        lr_prefix,
        state.trailing_slash,
        state.mode,
        current_islands_bundle_url(&state.islands_bundle_url).as_deref(),
        current_css_bundle_url(&state.css_bundle_url).as_deref(),
    )
}

/// Strip the dev server's mount prefix from `uri`'s path-and-query
/// shape so plugin handlers see the unprefixed URL they registered.
///
/// Returns `None` when the URI has no path-and-query component (which
/// would be very unusual — axum always populates this on incoming
/// requests). When `prefix` is `None` (no base configured) the path
/// is returned unchanged.
///
/// Path components must be a strict prefix (`/foo` followed by `/` or
/// end-of-string) — `/foobar` is NOT considered prefixed by `/foo` and
/// is returned unchanged. This mirrors the boundary semantics in the
/// build-time link rewriter (zfb#228).
fn strip_prefix_from_full_uri(uri: &Uri, prefix: Option<&str>) -> Option<String> {
    let pq = uri.path_and_query()?;
    let raw = pq.as_str();
    let prefix = match prefix {
        Some(p) if !p.is_empty() => p,
        _ => return Some(raw.to_string()),
    };
    if !raw.starts_with(prefix) {
        return Some(raw.to_string());
    }
    let rest = &raw[prefix.len()..];
    if rest.is_empty() {
        return Some("/".to_string());
    }
    let first_byte = rest.as_bytes()[0];
    if first_byte == b'/' || first_byte == b'?' || first_byte == b'#' {
        // Strict-prefix boundary: `/foo` followed by another path
        // segment, a query, or a fragment is genuinely under the
        // prefix. `/foobar` (no boundary char) is NOT.
        if first_byte == b'?' || first_byte == b'#' {
            return Some(format!("/{rest}"));
        }
        return Some(rest.to_string());
    }
    Some(raw.to_string())
}

/// Resolve `path` (following symlinks) and require the result to live
/// inside `root` (also symlink-resolved). Returns the canonical path on
/// success — callers MUST read the returned canonical path, not the
/// original: re-reading the original would reopen a check-then-use
/// window where a symlink swapped between check and read escapes the
/// root (TOCTOU).
///
/// Returning `None` on any canonicalize error (e.g. the file does not
/// exist yet) is intentional: callers treat a failed containment check
/// as not-found, so a missing symlink target is a safe 404.
///
/// Async (#903): this runs on every request-path disk fallback, so the
/// canonicalize syscalls go through `tokio::fs` (which offloads to the
/// blocking pool) instead of blocking the request worker directly.
async fn resolve_within_root(
    path: &std::path::Path,
    root: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let canonical_root = tokio::fs::canonicalize(root).await.ok()?;
    let canonical_path = tokio::fs::canonicalize(path).await.ok()?;
    canonical_path
        .starts_with(&canonical_root)
        .then_some(canonical_path)
}

/// Try to read a page from the dist directory on disk.
///
/// Probes `<dist_root>/<trimmed>/index.html` and then
/// `<dist_root>/<trimmed>` (for pages served at their exact path like
/// `sitemap.xml`). Returns `None` on any read failure.
///
/// `trimmed` comes from a percent-decoded URL, so we reject any path
/// that contains `..`, NUL, backslash, or absolute components before
/// joining onto `dist_root`. Without this gate, a request like
/// `/%2e%2e/...` would let a local browser tab read files outside dist.
///
/// After joining, we also canonicalize the resolved path and verify it
/// still lives inside `dist_root` — a symlink planted inside dist that
/// points outside would otherwise be followed silently.
async fn read_from_dist(dist_root: &std::path::Path, trimmed: &str) -> Option<Vec<u8>> {
    if !is_safe_url_path(trimmed) {
        return None;
    }
    let candidates: [PathBuf; 2] = [
        dist_root.join(trimmed).join("index.html"),
        dist_root.join(trimmed),
    ];
    for path in &candidates {
        let Some(resolved) = resolve_within_root(path, dist_root).await else {
            continue;
        };
        // Read the canonical path returned by resolve_within_root, not
        // the original joined path — re-reading the original would reopen
        // the check-then-use window.  Residual race: a directory component
        // swapped for a symlink between canonicalize and open can still
        // redirect the open; this is accepted (see assets_containment.rs
        // module doc for the reference model and full rationale).
        if let Ok(bytes) = tokio::fs::read(&resolved).await {
            return Some(bytes);
        }
    }
    None
}

/// Try to read a file from the project's `public/` directory on disk.
///
/// Unlike [`read_from_dist`] this does NOT probe an `index.html`
/// candidate — `public/` is a verbatim static-file mirror, so a
/// request to `/foo` resolves to `<public_root>/foo` only. Returns
/// `None` on any read failure (including the file simply not existing).
///
/// `trimmed` comes from a percent-decoded URL, so we reject any path
/// that contains `..`, NUL, backslash, or absolute components before
/// joining onto `public_root` — same threat model as [`read_from_dist`].
/// Empty `trimmed` (a bare `/` request) is also rejected because we
/// never want to serve the directory itself.
///
/// After joining, we also canonicalize the resolved path and verify it
/// still lives inside `public_root` — a symlink planted inside public/
/// that points outside would otherwise be followed silently.
async fn read_from_public(public_root: &std::path::Path, trimmed: &str) -> Option<Vec<u8>> {
    if trimmed.is_empty() || !is_safe_url_path(trimmed) {
        return None;
    }
    let path = public_root.join(trimmed);
    let resolved = resolve_within_root(&path, public_root).await?;
    // Reject directory reads explicitly — reading a directory returns an
    // EISDIR error on Unix and would surface as a None here anyway, but
    // on Windows the behaviour is platform-dependent. Being explicit also
    // documents the intent. Checked on the canonical path the read uses.
    let is_dir = tokio::fs::metadata(&resolved)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false);
    if is_dir {
        return None;
    }
    // Read the canonical path returned by resolve_within_root, not
    // the original joined path — re-reading the original would reopen
    // the check-then-use window.  Residual race: a directory component
    // swapped for a symlink between canonicalize and open can still
    // redirect the open; this is accepted (see assets_containment.rs
    // module doc for the reference model and full rationale).
    tokio::fs::read(&resolved).await.ok()
}

/// Reject URL paths that would escape the dist root once joined.
///
/// Mirrors `commands/preview.rs::is_safe_path` in the `zfb` crate; see
/// that copy for the rationale.
fn is_safe_url_path(url_path: &str) -> bool {
    let stripped = url_path.trim_start_matches('/');
    if stripped.is_empty() {
        return true;
    }
    if stripped.contains('\0') {
        return false;
    }
    let p = std::path::Path::new(stripped);
    for comp in p.components() {
        match comp {
            Component::Normal(part) => {
                if let Some(s) = part.to_str() {
                    if s.contains('\\') {
                        return false;
                    }
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => return false,
        }
    }
    true
}

/// Result of a plugin dev-middleware dispatch attempt. The dev server
/// folds `Passthrough` into the regular page-cache lookup; everything
/// else short-circuits with a Response.
enum PluginDispatchAttempt {
    Responded(Response),
    Passthrough,
    Errored(Response),
}

/// Build a [`PluginRequest`] for the given URL/method/headers/body and
/// dispatch it.
///
/// Issue #230: the request method, headers, and body are forwarded
/// verbatim so plugin handlers can implement non-GET endpoints (form
/// submissions, save actions, sidecar API proxies). Non-UTF-8 request
/// bodies are dropped here — dev-middleware bodies are line-protocol
/// JSON over stdio to the plugin host, and the wire shape only
/// supports UTF-8 strings. Binary uploads are a separate extension.
async fn dispatch_plugin(
    set: &DevMiddlewareSet,
    reg: &PluginRegistration,
    url_path: &str,
    method: &str,
    headers: HashMap<String, String>,
    body: Option<String>,
    mode: crate::ServerMode,
) -> PluginDispatchAttempt {
    let req = PluginRequest {
        method: method.to_string(),
        url: url_path.to_string(),
        headers,
        body,
    };
    match set.dispatcher.dispatch(&reg.handler_id, req).await {
        Ok(PluginDispatchOutcome::Response(resp)) => {
            // Decode body and build an axum response. The plugin host
            // pre-validates the status code; clamp out-of-range to 500
            // so a misbehaving handler can't synthesise an invalid
            // response.
            let status =
                StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let body_bytes: Vec<u8> = match resp.body_encoding {
                // Plugin handlers may opt into binary bodies via
                // `bodyEncoding: "base64"`. Use the standard crate
                // rather than rolling our own decoder so all the edge
                // cases (padding, whitespace, URL-safe alphabet) are
                // handled correctly.
                PluginResponseEncoding::Base64 => {
                    use base64::Engine as _;
                    match base64::engine::general_purpose::STANDARD.decode(resp.body.as_bytes()) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            let msg = format!(
                                "plugin `{}` returned an invalid base64 body: {}",
                                reg.plugin, e
                            );
                            return PluginDispatchAttempt::Errored(plugin_error_response(
                                &msg, mode,
                            ));
                        }
                    }
                }
                PluginResponseEncoding::Utf8 => resp.body.into_bytes(),
            };
            let mut builder = Response::builder().status(status);
            for (k, v) in resp.headers {
                // The body is reconstructed Rust-side (base64 decode /
                // into_bytes), so any Content-Length / Transfer-Encoding the
                // plugin returned is stale; Connection is hop-by-hop. Drop
                // them and let hyper recompute framing (matches dispatch_ssr).
                let lower = k.to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "content-length" | "transfer-encoding" | "connection"
                ) {
                    continue;
                }
                if let Ok(value) = HeaderValue::try_from(v) {
                    builder = builder.header(k, value);
                }
            }
            // Cache busting matches the rest of the dev server — plugin
            // responses are dev-only artefacts; never let a browser
            // cache a stale plugin emission across reloads.
            builder = builder.header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            match builder.body(axum::body::Body::from(body_bytes)) {
                Ok(resp) => PluginDispatchAttempt::Responded(resp),
                Err(e) => PluginDispatchAttempt::Errored(plugin_error_response(
                    &format!("failed to build response from plugin `{}`: {e}", reg.plugin,),
                    mode,
                )),
            }
        }
        Ok(PluginDispatchOutcome::Passthrough) => PluginDispatchAttempt::Passthrough,
        Err(err) => PluginDispatchAttempt::Errored(plugin_error_response(
            &format!(
                "plugin `{}` dev-middleware failed: {}",
                err.plugin, err.message,
            ),
            mode,
        )),
    }
}

/// Dispatch one request through the SSR layer (issue #367 / Gap 1).
///
/// `path_only` is the URL path without the dev-server mount prefix —
/// the V8 host receives URLs in their CF-adapter shape, not in their
/// dev-server-mounted shape. Headers/body are forwarded verbatim so a
/// `prerender = false` page can implement non-GET endpoints exactly
/// the way it would in production.
///
/// The response is built via [`page_response_bytes`] so HTML bodies
/// gain the live-reload `<script>` automatically. Non-HTML content
/// types (`application/json`, RSS, etc.) skip injection — mirrors the
/// page-cache content-type sniffing in [`serve_page`].
/// `url_path` must be the path-and-query with the dev server's mount
/// prefix (issue #229) already stripped. Production Cloudflare routes
/// never see the dev prefix, so dev parity requires the same shape
/// here.
#[allow(clippy::too_many_arguments)]
async fn dispatch_ssr(
    set: &SsrRouteSet,
    url_path: &str,
    method: &Method,
    headers: &HeaderMap,
    body: &Bytes,
    lr_prefix: &str,
    add_trailing_slash: bool,
    mode: crate::ServerMode,
    islands_bundle_url: Option<&str>,
    css_bundle_url: Option<&str>,
) -> Response {
    let mut req_headers: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            req_headers.insert(name.as_str().to_string(), v.to_string());
        }
    }
    let req = SsrRequest {
        method: method.as_str().to_string(),
        url_path: url_path.to_string(),
        headers: req_headers,
        body: body.to_vec(),
    };
    let resp = match set.dispatcher.dispatch(req).await {
        Ok(r) => r,
        Err(e) => {
            return ssr_error_response(url_path, &e.message, mode);
        }
    };
    // Pick the content-type from the SSR response headers (case-
    // insensitive) and default to HTML when the handler didn't set
    // one — most prerender=false pages return HTML.
    let content_type = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "text/html; charset=utf-8".to_string());
    let is_html = content_type.to_ascii_lowercase().starts_with("text/html");
    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut out = page_response_bytes(
        status,
        resp.body,
        &content_type,
        is_html,
        lr_prefix,
        add_trailing_slash,
        mode,
        islands_bundle_url,
        css_bundle_url,
    );
    // Merge any extra headers the SSR handler set (Set-Cookie, Vary,
    // …). Skip headers `page_response_bytes` already wrote
    // (`content-type`, `cache-control`) so a handler that explicitly
    // sets `cache-control: public,max-age=60` doesn't silently lose to
    // our no-store default — instead the SSR header wins.
    for (k, v) in resp.headers.iter() {
        let lower = k.to_ascii_lowercase();
        // `page_response_bytes` rewrites the HTML body (livereload script,
        // doctype, head asset injection, base-prefix link rewrite), so any
        // Content-Length / Transfer-Encoding the handler returned is now
        // stale, and Connection is hop-by-hop. Drop them and let hyper
        // recompute framing from the rewritten body. (content-type is set
        // by `page_response_bytes`.)
        if matches!(
            lower.as_str(),
            "content-type" | "content-length" | "transfer-encoding" | "connection"
        ) {
            continue;
        }
        if let Ok(name) = header::HeaderName::try_from(k.as_str()) {
            if let Ok(value) = HeaderValue::try_from(v) {
                // For Cache-Control we let the handler override the
                // default no-store; for all other headers we insert
                // (replace). Multi-valued headers like Set-Cookie
                // would need append semantics, but the BTreeMap shape
                // upstream already collapses duplicates.
                out.headers_mut().insert(name, value);
            }
        }
    }
    out
}

/// Dispatch one request through a Rust-side embed handler registered
/// via [`crate::ServerBuilder::with_ssr_handler`] (issue #372).
///
/// The handler receives an [`axum::http::Request<Body>`] reconstructed
/// from the captured method, URL, headers, and body, plus the
/// pattern's captured params. The handler's response goes through
/// `IntoResponse` (already applied at registration time by
/// [`crate::embed_handlers::erase_handler`]), so handlers can return
/// strings, tuples, or full responses without further conversion at
/// the dispatch site.
///
/// The URL the handler sees has the dev-server mount prefix stripped —
/// matching the URL shape a production deployment would observe —
/// because handlers are expected to encode their patterns in the
/// no-prefix shape.
#[allow(clippy::too_many_arguments)]
async fn dispatch_embed_handler(
    handler: crate::embed_handlers::EmbedHandlerFn,
    params: crate::embed_handlers::RouteParams,
    uri: &Uri,
    method: &Method,
    headers: &HeaderMap,
    extensions: &Extensions,
    body: Bytes,
    base_prefix: Option<&str>,
    mode: crate::ServerMode,
) -> Response {
    // Rebuild the inbound request so the handler sees a plain
    // `http::Request<Body>` — no axum-specific extractors required.
    // The URL is path-and-query with the dev server's mount prefix
    // stripped (mirrors the SSR dispatch shape).
    //
    // Per-request `Extensions` injected by
    // [`crate::middleware::apply_request_extension_layer`] are
    // forwarded verbatim so the handler can read host-supplied values
    // via `req.extensions().get::<T>()`.
    let stripped =
        strip_prefix_from_full_uri(uri, base_prefix).unwrap_or_else(|| uri.path().to_string());

    let mut builder = Request::builder().method(method.clone()).uri(&stripped);
    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
    }
    let mut req = match builder.body(Body::from(body)) {
        Ok(r) => r,
        Err(e) => {
            return embed_handler_error_response(
                &format!("failed to rebuild request for embed handler: {e}"),
                mode,
            );
        }
    };
    *req.extensions_mut() = extensions.clone();

    handler(req, params).await
}

/// Build the HTML 5xx response served when reconstructing the request
/// for an embed handler fails. Should be unreachable in practice — the
/// inbound request was already a valid axum request — but the explicit
/// fallback avoids a panic if some future header/value combination
/// trips the `http::Request::builder` checks.
fn embed_handler_error_response(message: &str, mode: crate::ServerMode) -> Response {
    // Dev mode: verbose body with the error detail. Preview/Embed: generic body
    // only; full detail is logged server-side so clients never see internal info.
    let body = if matches!(mode, crate::ServerMode::Dev) {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>zfb dev \u{2014} handler error</title></head><body><h1>Handler dispatch error</h1><pre>{}</pre></body></html>",
            escape_html(message),
        )
    } else {
        tracing::error!(message, "embed handler dispatch error");
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Internal Server Error</title></head><body><h1>Internal Server Error</h1></body></html>".to_string()
    };
    let mut resp = (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        body,
    )
        .into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// Build the HTML 5xx response served when [`SsrDispatcher::dispatch`]
/// returns an error. Surfaces the underlying message so the developer
/// sees the V8 stack trace instead of an empty 500.
fn ssr_error_response(url_path: &str, message: &str, mode: crate::ServerMode) -> Response {
    // Dev mode: verbose body with path + V8 detail. Preview/Embed: generic body
    // only; full detail is logged server-side so clients never see internal info.
    let body = if matches!(mode, crate::ServerMode::Dev) {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>zfb dev \u{2014} ssr error</title></head><body><h1>SSR error at <code>{}</code></h1><pre>{}</pre></body></html>",
            escape_html(url_path),
            escape_html(message),
        )
    } else {
        tracing::error!(url_path, message, "SSR dispatch error");
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Internal Server Error</title></head><body><h1>Internal Server Error</h1></body></html>".to_string()
    };
    let mut resp = (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        body,
    )
        .into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// Origin gate for non-GET/HEAD requests reaching the dynamic dispatch
/// surfaces — plugin dev-middleware, embed handlers, request-time SSR
/// (issue #931 / #919). Returns `Some(403)` when the server is
/// LAN-exposed (host validation enforced) and either:
///
/// - the `Origin` header is absent on a non-GET request (fail closed —
///   browsers always send `Origin` on cross-origin non-GET requests, so
///   absence implies a non-browser LAN client bypassing CORS), or
/// - the request carries an `Origin` whose host fails the same allowlist
///   the Host-header layer uses.
///
/// Returns `None` (allow) when:
///
/// - the method is GET/HEAD (safe methods rely on the Host check), or
/// - the server is bound to loopback (default — zero behaviour change).
///
/// Static read paths (`/assets`, dist/public fallbacks, livereload) are
/// exempt by construction — this helper is only invoked at the three
/// dynamic-dispatch sites inside [`serve_page`].
fn origin_rejection(state: &AppState, method: &Method, headers: &HeaderMap) -> Option<Response> {
    if matches!(*method, Method::GET | Method::HEAD) {
        return None;
    }
    let validation = &state.host_validation;
    if !validation.is_enforced() {
        return None;
    }
    let Some(value) = headers.get(header::ORIGIN) else {
        // When enforcement is on, a non-GET request that omits the Origin
        // header cannot be a browser cross-origin request (browsers always
        // send it). Fail closed: return 403 so non-browser LAN clients
        // cannot bypass the CSRF guard by dropping the header.
        return Some(crate::host_validation::missing_origin_forbidden_response(
            state.mode,
        ));
    };
    // Present-but-unreadable (non-ASCII) and disallowed origins both
    // fail closed.
    let allowed = value
        .to_str()
        .map(|origin| validation.origin_allowed(origin))
        .unwrap_or(false);
    if allowed {
        return None;
    }
    let shown = value.to_str().unwrap_or("<non-ASCII>");
    Some(crate::host_validation::origin_forbidden_response(
        shown, state.mode,
    ))
}

/// Build the `405 Method Not Allowed` response returned when a non-GET
/// request reaches the page-cache fallback (i.e. no plugin claimed the
/// URL or a plugin returned `Passthrough`). Mirrors what axum used to
/// emit for `get(...)` routes before issue #230 broadened the page
/// mounts to every HTTP method.
fn method_not_allowed_get_head() -> Response {
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header(header::ALLOW, HeaderValue::from_static("GET, HEAD"))
        .header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))
        .body(axum::body::Body::empty())
        .expect("static 405 response builds")
}

/// Convert an axum [`HeaderMap`] into the flat string map shape the
/// plugin host wire protocol expects. Header values that are not valid
/// UTF-8 are dropped — the JS-side handler receives a string-keyed
/// object and cannot represent arbitrary bytes. Multi-valued headers
/// keep the last seen value (the protocol does not currently model
/// repeated headers; see the dev-middleware contract in
/// `crates/zfb/js/plugin-host.mjs`).
fn headermap_to_string_map(headers: &HeaderMap) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(headers.len());
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            out.insert(name.as_str().to_string(), v.to_string());
        }
    }
    out
}

/// Convert an inbound request body (already drained into [`Bytes`]) to
/// the `Option<String>` shape the plugin host wire protocol expects.
/// Empty bodies become `None`; non-UTF-8 bodies are dropped (see the
/// note in [`dispatch_plugin`] about binary uploads being a separate
/// extension).
fn body_bytes_to_utf8_string(body: &Bytes) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    std::str::from_utf8(body).ok().map(|s| s.to_string())
}

fn plugin_error_response(message: &str, mode: crate::ServerMode) -> Response {
    // Dev mode: verbose body with the plugin error detail. Preview/Embed: generic
    // body only; full detail is logged server-side so clients never see internal info.
    let body = if matches!(mode, crate::ServerMode::Dev) {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>zfb dev — plugin error</title></head><body><h1>Plugin dev-middleware error</h1><pre>{}</pre></body></html>",
            escape_html(message),
        )
    } else {
        tracing::error!(message, "plugin dev-middleware error");
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Internal Server Error</title></head><body><h1>Internal Server Error</h1></body></html>".to_string()
    };
    let mut resp = (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        body,
    )
        .into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// Generate the lookup-key candidates for a given URL path.
///
/// `path` is the path WITHOUT a leading slash (the wildcard capture).
/// Empty string means "the root".
///
/// We return up to three candidates in priority order; the first hit
/// wins. The variants cover:
///
/// - exact match (`/blog/foo` → `/blog/foo`)
/// - directory-style with `index.html` (`/blog/foo` → `/blog/foo/index.html`)
/// - directory-style with trailing slash (`/blog/foo` → `/blog/foo/`)
fn lookup_keys(path: &str) -> Vec<String> {
    if path.is_empty() {
        return vec!["/".to_string(), "/index.html".to_string()];
    }
    // Normalise any trailing slash(es) once so `foo`, `foo/`, and
    // `foo//` all collapse to the same candidates.
    let stripped = path.trim_end_matches('/');
    if stripped.is_empty() {
        // The whole path was slashes — treat as root.
        return vec!["/".to_string(), "/index.html".to_string()];
    }
    vec![
        format!("/{stripped}"),
        format!("/{stripped}/index.html"),
        format!("/{stripped}/"),
    ]
}

/// Build a page response from a raw-bytes body with a custom
/// `Content-Type`, injecting the live-reload tag only for HTML
/// responses (Sub 49: non-HTML pages like RSS feeds or sitemaps must
/// not be polluted with a script tag — they'd fail XML parsers).
///
/// When `inject_reload` is `true` the body bytes are interpreted as a
/// UTF-8 HTML document (a reasonable assumption for dev-mode HTML).
/// Non-UTF-8 bytes in an HTML body are served as-is without injection
/// rather than panicking — a graceful degradation for the unlikely
/// case of a malformed page reaching the dev cache.
///
/// `base_prefix` is the dev server's mount prefix (issue #229) — empty
/// for the no-base case, or `"/foo"` when `base: "/foo/"` is set —
/// folded into the `<script src>` URL so the browser fetches the
/// live-reload JS at the prefixed path the dev server actually serves
/// it at.
///
/// `add_trailing_slash` mirrors `zfb.config.ts`'s `trailingSlash` field
/// so the dev-mode rewrite shape matches the canonical build-mode
/// shape (sub #234 / zudolab/zudo-doc#1579).
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_response_bytes(
    status: StatusCode,
    body: Vec<u8>,
    content_type: &str,
    inject_reload: bool,
    base_prefix: &str,
    add_trailing_slash: bool,
    mode: crate::ServerMode,
    islands_bundle_url: Option<&str>,
    css_bundle_url: Option<&str>,
) -> Response {
    // Live-reload script injection and the default `Cache-Control: no-
    // store` shaping are Dev-only — Preview and Embed callers want
    // production-shaped HTML responses (no SSE script, no aggressive
    // cache busting that would defeat a host's own caching policy).
    let is_dev = matches!(mode, crate::ServerMode::Dev);
    let inject_reload = inject_reload && is_dev;
    // Issue #377: dev-mode initial-load injection of the islands
    // `<script type="module">` tag. Gated to Dev mode so Preview/Embed
    // callers never ship the unhashed `/assets/islands.js` URL — that
    // would defeat production hashing for embedders running this server
    // off a built `dist/`. Also gated to HTML responses (`inject_reload`
    // doubles as the is-HTML signal — both flips are set by the same
    // call-site logic). When the bundle URL is empty/whitespace it is
    // treated as absent (the bin crate seeds the handle with `None`
    // when no `"use client"` islands exist).
    let islands_script_url = if is_dev && inject_reload {
        islands_bundle_url.map(str::trim).filter(|u| !u.is_empty())
    } else {
        None
    };
    // Issue #494 / #498: dev-mode injection of the CSS `<link>` tag.
    // Same gate as islands — Dev mode only, HTML responses only.
    let css_link_url = if is_dev && inject_reload {
        css_bundle_url.map(str::trim).filter(|u| !u.is_empty())
    } else {
        None
    };
    let body_out: Vec<u8> = if inject_reload {
        // HTML should always be valid UTF-8; fall back to raw bytes on
        // the rare occasion it isn't so we don't panic in dev mode.
        match std::str::from_utf8(&body) {
            Ok(html) => {
                // Issue #228 + #229: when the dev server is mounted under a
                // `base` prefix, user-authored root-absolute `<a href>` /
                // `<form action>` in the cached HTML must be rewritten so
                // navigation under the prefix doesn't 404. Production
                // builds run the same pass on disk; the dev server runs it
                // in-flight per response. Empty prefix is a no-op (the
                // shared `compute_prefixed` short-circuits via idempotency)
                // so this only changes behaviour when a base IS set.
                let rewritten = if base_prefix.is_empty() {
                    Cow::Borrowed(html)
                } else {
                    match zfb_build::link_base_rewrite::rewrite_links_in_html(
                        html,
                        base_prefix,
                        add_trailing_slash,
                    ) {
                        Ok(s) => Cow::Owned(s),
                        // Graceful degradation in dev mode: lol_html should
                        // not fail on the renderer's well-formed HTML, but
                        // if something pathological reaches us, serve the
                        // original bytes rather than 500ing the page.
                        Err(_) => Cow::Borrowed(html),
                    }
                };
                // Splice the CSS `<link>` and islands `<script type="module">`
                // tags into `<head>` (so livereload's `</body>`-anchored tag
                // still trails the rest of the body markup). The shared helper
                // is idempotent and a passthrough for bodies that have no
                // `</head>`. CSS is injected before islands so the stylesheet
                // loads first on initial page render.
                let with_head_assets = {
                    let assets = zfb_build::head_inject::ProdHeadAssets {
                        css_url: css_link_url.map(str::to_owned),
                        island_module_urls: islands_script_url
                            .map(|u| vec![u.to_string()])
                            .unwrap_or_default(),
                    };
                    if assets.is_empty() {
                        rewritten
                    } else {
                        Cow::Owned(
                            zfb_build::head_inject::inject_prod_head_assets(&rewritten, &assets)
                                .into_owned(),
                        )
                    }
                };
                inject_livereload_with_prefix(&with_head_assets, base_prefix).into_bytes()
            }
            Err(_) => body,
        }
    } else {
        body
    };

    // Issue #530: HTML5 doctype prepend for dev/preview SSR responses.
    // Mirrors the same guard in `crates/zfb-build/src/renderer.rs::render_one_inner`.
    // Gated on `text/html` (via the same split on `;` the renderer uses) so
    // non-HTML routes (XML, JSON, plain-text) are never touched. The helper
    // `needs_html5_doctype` further skips bodies that already declare a
    // doctype (case-insensitive, BOM-aware), preventing double-prepend.
    let is_html_content_type = content_type
        .split(';')
        .next()
        .map(|m| m.trim().eq_ignore_ascii_case("text/html"))
        .unwrap_or(false);
    let body_out: Vec<u8> = if is_html_content_type {
        match std::str::from_utf8(&body_out) {
            Ok(text) if zfb_build::head_inject::needs_html5_doctype(text) => {
                format!("{}{text}", zfb_build::head_inject::HTML5_DOCTYPE_PREFIX).into_bytes()
            }
            _ => body_out,
        }
    } else {
        body_out
    };

    // The Content-Type may come from a user frontmatter override and
    // therefore can't be statically validated; fall back to a safe
    // default if the value contains characters HTTP rejects.
    let ct_header = HeaderValue::try_from(content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("text/html; charset=utf-8"));

    let mut resp = (status, [(header::CONTENT_TYPE, ct_header)], body_out).into_response();
    if is_dev {
        // `no-store` is a dev-server-only default — Preview / Embed
        // callers want browsers (and their own host caches) to respect
        // whatever cache policy the underlying handler set (or the
        // production CDN downstream will set).
        resp.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inject::LIVERELOAD_TAG;
    use crate::livereload::ReloadEvent;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let dist = std::env::temp_dir().join("zfb-test-dist");
        AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.clone(),
            // Tests don't exercise the dev/build split — alias to dist.
            html_root: dist,
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: None,
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        }
    }

    fn test_state_with_base(prefix: &str) -> AppState {
        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let dist = std::env::temp_dir().join("zfb-test-dist");
        AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.clone(),
            html_root: dist,
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: Some(prefix.to_string()),
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: None,
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        }
    }

    fn test_router(state: AppState) -> Router {
        // The dist/public roots on the state are bogus paths that
        // don't exist on disk, so the dist + public on-disk fallbacks
        // inside `serve_page` simply return None — which is fine: these
        // tests exercise the routing logic, not asset I/O. Tests that
        // need real on-disk fixtures override `dist_root` / `public_root`
        // on the AppState before calling `test_router`.
        build_router(state)
    }

    async fn body_string(resp: Response) -> String {
        let bytes = to_bytes(resp.into_body(), 1_000_000).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// Issue #534 regression — on a cache miss the page handler must read
    /// HTML from `state.html_root`, NOT from `state.dist_root`. In `zfb
    /// dev` the two point at different directories: dev writes per-route
    /// HTML to `<project>/.zfb-build/dev-pages/`, the production output
    /// stays in `<project>/dist/`. Wiring this fallback to `dist_root`
    /// (the historical bug) would mean dev served the most recent
    /// `pnpm build` output instead of the dev pipeline's edits.
    ///
    /// Falsifiability: flipping the read in `serve_page` from
    /// `&state.html_root` back to `&state.dist_root` causes the
    /// `assert!(body.contains("dev-page"))` to fail — the request would
    /// either 404 (no file at the dist_root path) or serve the
    /// `prod-build` body from the other tempdir.
    #[tokio::test]
    async fn disk_fallback_reads_html_root_not_dist_root() {
        use tempfile::TempDir;
        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);

        // Two separate on-disk roots. `dist_root` mimics what an
        // earlier `pnpm build` left behind; `html_root` is dev's
        // own per-route output dir.
        let dist_dir = TempDir::new().expect("dist tempdir");
        let html_dir = TempDir::new().expect("html tempdir");

        // The disk fallback probes `<root>/<trimmed>/index.html`
        // first, so create the file under that shape on the dev side.
        let html_page_dir = html_dir.path().join("blog");
        std::fs::create_dir_all(&html_page_dir).expect("mk html subdir");
        std::fs::write(
            html_page_dir.join("index.html"),
            "<html><body>dev-page-from-html_root</body></html>",
        )
        .expect("write dev html");

        // Put a different page at the dist_root path so a wrong wiring
        // would serve THIS body and fail the assertion below.
        let dist_page_dir = dist_dir.path().join("blog");
        std::fs::create_dir_all(&dist_page_dir).expect("mk dist subdir");
        std::fs::write(
            dist_page_dir.join("index.html"),
            "<html><body>prod-build-leftover-from-dist_root</body></html>",
        )
        .expect("write dist html");

        let state = AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist_dir.path().to_path_buf(),
            html_root: html_dir.path().to_path_buf(),
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: None,
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        };
        // Cache miss — the fallback must read from html_root.
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/blog/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(
            body.contains("dev-page-from-html_root"),
            "disk fallback must read from html_root; got body:\n{body}",
        );
        assert!(
            !body.contains("prod-build-leftover-from-dist_root"),
            "disk fallback must NOT read from dist_root; got body:\n{body}",
        );
    }

    #[tokio::test]
    async fn serves_page_from_cache() {
        let state = test_state();
        state
            .pages
            .insert("/", "<html><body><h1>home</h1></body></html>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("<h1>home</h1>"));
        assert!(body.contains(LIVERELOAD_TAG));
    }

    #[tokio::test]
    async fn serves_nested_page_from_cache() {
        let state = test_state();
        state
            .pages
            .insert("/blog/foo", "<html><body>foo</body></html>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/blog/foo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("foo"));
        assert!(body.contains(LIVERELOAD_TAG));
    }

    #[tokio::test]
    async fn falls_back_to_index_html_key() {
        let state = test_state();
        state
            .pages
            .insert("/blog/foo/index.html", "<html><body>indexed</body></html>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/blog/foo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_string(resp).await.contains("indexed"));
    }

    #[tokio::test]
    async fn returns_dev_404_for_unknown_path() {
        let state = test_state();
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/nope/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_string(resp).await;
        assert!(body.contains("404"));
        assert!(body.contains(LIVERELOAD_TAG));
    }

    #[tokio::test]
    async fn livereload_js_endpoint_serves_script() {
        let state = test_state();
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/__zfb/livereload.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            ct.starts_with("application/javascript"),
            "unexpected content-type: {ct}"
        );

        let cc = resp
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(cc, "no-store");

        let body = body_string(resp).await;
        assert!(body.contains("EventSource"));
        // The unprefixed literal survives only as the no-currentScript
        // fallback (issue #1027): the stream URL is derived from the
        // script tag's own src so a `base`-prefixed dev server connects
        // to <base>/__zfb/reload instead of 404ing on the bare path.
        assert!(body.contains("/__zfb/reload"));
        assert!(
            body.contains("document.currentScript"),
            "served livereload.js must derive the SSE stream URL from its \
             own script src (base-prefix awareness)"
        );
    }

    #[tokio::test]
    async fn html_responses_are_cache_busted() {
        let state = test_state();
        state
            .pages
            .insert("/x", "<html><body>x</body></html>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(Request::builder().uri("/x").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let cc = resp
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(cc, "no-store");
    }

    #[test]
    fn lookup_keys_root() {
        assert_eq!(
            lookup_keys(""),
            vec!["/".to_string(), "/index.html".to_string()]
        );
    }

    #[test]
    fn lookup_keys_simple_path() {
        let out = lookup_keys("blog/foo");
        assert_eq!(out[0], "/blog/foo");
        assert_eq!(out[1], "/blog/foo/index.html");
        assert_eq!(out[2], "/blog/foo/");
    }

    #[test]
    fn lookup_keys_trailing_slash_request() {
        let out = lookup_keys("blog/foo/");
        assert_eq!(out[0], "/blog/foo");
        assert_eq!(out[1], "/blog/foo/index.html");
        assert_eq!(out[2], "/blog/foo/");
    }

    // ---- non-HTML page content-type (Sub 49) -----------------------------

    #[test]
    fn content_type_for_extension_known() {
        assert_eq!(
            content_type_for_extension("html"),
            "text/html; charset=utf-8"
        );
        assert_eq!(content_type_for_extension("xml"), "application/xml");
        assert_eq!(content_type_for_extension("rss"), "application/rss+xml");
        assert_eq!(content_type_for_extension("json"), "application/json");
        assert_eq!(
            content_type_for_extension("txt"),
            "text/plain; charset=utf-8"
        );
    }

    #[test]
    fn content_type_for_extension_unknown_falls_back_to_octet_stream() {
        // Unknown extensions get a generic binary content-type so a
        // browser doesn't sniff them as HTML and execute scripts.
        assert_eq!(
            content_type_for_extension("weird"),
            "application/octet-stream",
        );
    }

    #[test]
    fn content_type_for_extension_known_binary_types() {
        assert_eq!(content_type_for_extension("pdf"), "application/pdf");
        assert_eq!(content_type_for_extension("png"), "image/png");
        assert_eq!(content_type_for_extension("jpg"), "image/jpeg");
        assert_eq!(content_type_for_extension("svg"), "image/svg+xml");
        assert_eq!(content_type_for_extension("wasm"), "application/wasm");
    }

    #[test]
    fn resolve_content_type_uses_override_first() {
        let entry = CachedPage {
            body: b"<rss/>".to_vec(),
            content_type: Some("application/rss+xml".into()),
        };
        // URL says `.xml`, but the override (`application/rss+xml`)
        // wins per the precedence rule.
        assert_eq!(
            resolve_content_type(&entry, "/feed.xml"),
            "application/rss+xml"
        );
    }

    #[test]
    fn resolve_content_type_falls_back_to_url_extension() {
        let entry = CachedPage {
            body: b"<urlset/>".to_vec(),
            content_type: None,
        };
        assert_eq!(
            resolve_content_type(&entry, "/sitemap.xml"),
            "application/xml"
        );
        assert_eq!(
            resolve_content_type(&entry, "/llms.txt"),
            "text/plain; charset=utf-8"
        );
    }

    #[test]
    fn resolve_content_type_html_default_for_extensionless() {
        let entry = CachedPage {
            body: b"<p>x</p>".to_vec(),
            content_type: None,
        };
        // No extension on the URL → HTML default.
        assert_eq!(
            resolve_content_type(&entry, "/about"),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            resolve_content_type(&entry, "/"),
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn serves_xml_with_correct_content_type_and_no_livereload() {
        let state = test_state();
        // sitemap.xml.tsx → cache key /sitemap.xml; URL-extension
        // derivation produces application/xml.
        state
            .pages
            .insert("/sitemap.xml", "<urlset></urlset>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/sitemap.xml")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "application/xml");

        // Crucial: non-HTML responses must NOT have the live-reload
        // <script> injected — it would corrupt XML parsers.
        let body = body_string(resp).await;
        assert!(body.contains("<urlset>"));
        assert!(!body.contains(LIVERELOAD_TAG));
    }

    #[tokio::test]
    async fn frontmatter_content_type_override_wins_over_url_extension() {
        let state = test_state();
        // Page is at `/feed.xml` but its frontmatter sets
        // contentType = "application/rss+xml". The override beats
        // the URL-extension derivation.
        state
            .pages
            .insert_with_content_type(
                "/feed.xml",
                "<rss></rss>",
                Some("application/rss+xml".into()),
            )
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/feed.xml")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "application/rss+xml");
    }

    #[tokio::test]
    async fn html_pages_still_get_livereload_injected() {
        let state = test_state();
        state
            .pages
            .insert("/about", "<html><body>x</body></html>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/about")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains(LIVERELOAD_TAG));
    }

    // -------------------------------------------------------------------
    // Issue #494 / #498 — dev-mode CSS `<link>` injection
    // -------------------------------------------------------------------
    //
    // Mirror of the islands-injection acceptance criterion: the page
    // handler must splice a `<link rel="stylesheet">` into `<head>` when
    // `AppState.css_bundle_url` carries a URL.  Two cases:
    //   1. `css_bundle_url == Some(url)` → link tag present in `<head>`.
    //   2. `css_bundle_url == None` → no link tag injected.
    // A combined case also verifies that CSS and islands co-exist when
    // both handles are seeded.

    fn make_css_bundle_url(url: &str) -> crate::CssBundleUrl {
        Arc::new(std::sync::RwLock::new(Some(url.to_string())))
    }

    fn make_islands_bundle_url(url: &str) -> crate::IslandsBundleUrl {
        Arc::new(std::sync::RwLock::new(Some(url.to_string())))
    }

    #[tokio::test]
    async fn css_link_injected_into_head_when_handle_seeded() {
        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let dist = std::env::temp_dir().join("zfb-test-dist");
        let state = AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.clone(),
            html_root: dist,
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: Some(make_css_bundle_url("/assets/styles.css")),
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        };
        // HTML must include <head></head> so inject_prod_head_assets has an anchor.
        state
            .pages
            .insert("/", "<html><head></head><body><p>hello</p></body></html>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(
            body.contains("<link rel=\"stylesheet\" href=\"/assets/styles.css\">"),
            "expected css link tag in response body; got:\n{body}",
        );
        // Livereload script must also still be present.
        assert!(body.contains(LIVERELOAD_TAG));
    }

    #[tokio::test]
    async fn css_link_absent_when_handle_is_none() {
        let state = test_state();
        state
            .pages
            .insert("/", "<html><head></head><body><p>hello</p></body></html>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(
            !body.contains("stylesheet"),
            "expected no stylesheet link when css_bundle_url is None; got:\n{body}",
        );
    }

    #[tokio::test]
    async fn css_and_islands_both_injected_when_both_handles_seeded() {
        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let dist = std::env::temp_dir().join("zfb-test-dist");
        let state = AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.clone(),
            html_root: dist,
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: Some(make_islands_bundle_url("/assets/islands.js")),
            css_bundle_url: Some(make_css_bundle_url("/assets/styles.css")),
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        };
        state
            .pages
            .insert("/", "<html><head></head><body><p>hello</p></body></html>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(
            body.contains("<link rel=\"stylesheet\" href=\"/assets/styles.css\">"),
            "expected css link tag; got:\n{body}",
        );
        assert!(
            body.contains("<script type=\"module\" src=\"/assets/islands.js\">"),
            "expected islands script tag; got:\n{body}",
        );
        assert!(body.contains(LIVERELOAD_TAG));
    }

    #[tokio::test]
    async fn css_link_not_injected_in_preview_mode() {
        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let dist = std::env::temp_dir().join("zfb-test-dist");
        let state = AppState {
            mode: crate::ServerMode::Preview,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.clone(),
            html_root: dist,
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: Some(make_css_bundle_url("/assets/styles.css")),
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        };
        state
            .pages
            .insert("/", "<html><head></head><body><p>hello</p></body></html>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(
            !body.contains("stylesheet"),
            "expected no stylesheet link in Preview mode; got:\n{body}",
        );
    }

    // -------------------------------------------------------------------
    // Issue #230 — devMiddleware accepts every HTTP method
    // -------------------------------------------------------------------
    //
    // The legacy router only accepted GET/HEAD on the page-renderer
    // mounts, which meant a plugin's `/api/echo` POST handler never
    // ran — axum returned 405 before the plugin layer was reached. The
    // tests below exercise the new policy:
    //
    // - User-registered devMiddleware paths receive every method, with
    //   `req.method`, `req.headers`, and `req.body` propagated.
    // - Built-in routes (`/__zfb/livereload.js`, `/__zfb/reload`) keep
    //   their GET-only semantics so a stray POST cannot reach
    //   dev-server infrastructure.
    // - Non-GET requests that hit the page-cache fallback (no plugin
    //   match, or plugin returned `Passthrough`) get a deterministic
    //   `405 Allow: GET, HEAD` instead of a confused 404.

    /// Recording dispatcher: stores the last [`PluginRequest`] it saw
    /// and returns whatever the test asks for.
    struct RecordingDispatcher {
        last: tokio::sync::Mutex<Option<PluginRequest>>,
        outcome: PluginDispatchOutcome,
    }

    #[async_trait::async_trait]
    impl crate::plugin_middleware::DevMiddlewareDispatcher for RecordingDispatcher {
        async fn dispatch(
            &self,
            _id: &str,
            request: PluginRequest,
        ) -> Result<PluginDispatchOutcome, crate::plugin_middleware::PluginDispatchError> {
            *self.last.lock().await = Some(request);
            Ok(self.outcome.clone())
        }
    }

    fn echo_response() -> PluginDispatchOutcome {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        PluginDispatchOutcome::Response(crate::plugin_middleware::PluginResponse {
            status: 200,
            headers,
            body: "{\"ok\":true}".to_string(),
            body_encoding: PluginResponseEncoding::Utf8,
        })
    }

    fn state_with_dispatcher(dispatcher: Arc<RecordingDispatcher>, path: &str) -> AppState {
        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let set = DevMiddlewareSet {
            registrations: Arc::new(vec![PluginRegistration {
                path: path.to_string(),
                handler_id: "h1".to_string(),
                plugin: "test".to_string(),
            }]),
            dispatcher: dispatcher.clone()
                as Arc<dyn crate::plugin_middleware::DevMiddlewareDispatcher>,
        };
        let dist = std::env::temp_dir().join("zfb-test-dist");
        AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: Some(set),
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.clone(),
            html_root: dist,
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: None,
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        }
    }

    #[tokio::test]
    async fn plugin_handler_receives_post_request() {
        let dispatcher = Arc::new(RecordingDispatcher {
            last: tokio::sync::Mutex::new(None),
            outcome: echo_response(),
        });
        let state = state_with_dispatcher(dispatcher.clone(), "/api/echo");
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/echo")
                    .header("content-type", "application/json")
                    .body(Body::from("{\"x\":1}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let captured = dispatcher
            .last
            .lock()
            .await
            .clone()
            .expect("plugin should have been dispatched");
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.url, "/api/echo");
        assert_eq!(captured.body.as_deref(), Some("{\"x\":1}"));
        assert_eq!(
            captured.headers.get("content-type").map(String::as_str),
            Some("application/json"),
        );
    }

    #[tokio::test]
    async fn plugin_handler_receives_put_and_delete() {
        for method in ["PUT", "DELETE", "PATCH"] {
            let dispatcher = Arc::new(RecordingDispatcher {
                last: tokio::sync::Mutex::new(None),
                outcome: echo_response(),
            });
            let state = state_with_dispatcher(dispatcher.clone(), "/api/echo");
            let router = test_router(state);

            let resp = router
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/api/echo")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "method {method} should reach plugin handler"
            );
            let captured = dispatcher.last.lock().await.clone().unwrap();
            assert_eq!(captured.method, method);
        }
    }

    #[tokio::test]
    async fn plugin_handler_still_works_for_get() {
        // Regression: switching to `any(...)` must not break the
        // pre-existing GET path.
        let dispatcher = Arc::new(RecordingDispatcher {
            last: tokio::sync::Mutex::new(None),
            outcome: echo_response(),
        });
        let state = state_with_dispatcher(dispatcher.clone(), "/api/echo");
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/echo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let captured = dispatcher.last.lock().await.clone().unwrap();
        assert_eq!(captured.method, "GET");
    }

    #[tokio::test]
    async fn post_to_built_in_livereload_js_still_returns_405() {
        // Built-in route is registered with `get(...)`; axum returns
        // 405 with `Allow: GET, HEAD` for any other method. This is
        // the exact CSRF-relevant guarantee the issue calls out:
        // do NOT loosen built-in routes when broadening user-registered
        // devMiddleware prefixes.
        let state = test_state();
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/__zfb/livereload.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        let allow = resp
            .headers()
            .get(header::ALLOW)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default();
        assert!(
            allow.to_ascii_uppercase().contains("GET"),
            "Allow header missing GET: {allow:?}"
        );
    }

    #[tokio::test]
    async fn post_to_built_in_livereload_sse_still_returns_405() {
        // Same as above, for the SSE endpoint.
        let state = test_state();
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/__zfb/reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn post_to_unregistered_path_returns_405() {
        // A POST that does not match any plugin registration must NOT
        // be silently treated as a page lookup. Returning a clear 405
        // makes the dev-server method policy obvious to plugin authors
        // who forgot to register the path they're posting to.
        let state = test_state();
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/some/random/path")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        let allow = resp
            .headers()
            .get(header::ALLOW)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(allow.contains("GET"), "Allow header missing GET: {allow}");
        assert!(allow.contains("HEAD"), "Allow header missing HEAD: {allow}");
    }

    #[tokio::test]
    async fn plugin_passthrough_on_post_returns_405() {
        // When a plugin is registered on a path but returns
        // Passthrough, the page-cache fallback should NOT be used for
        // non-GET methods (the cache is GET-only). 405 keeps the
        // method policy consistent.
        let dispatcher = Arc::new(RecordingDispatcher {
            last: tokio::sync::Mutex::new(None),
            outcome: PluginDispatchOutcome::Passthrough,
        });
        let state = state_with_dispatcher(dispatcher.clone(), "/api/echo");
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/echo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        // Plugin WAS invoked and chose to pass.
        assert!(dispatcher.last.lock().await.is_some());
    }

    #[tokio::test]
    async fn plugin_passthrough_on_get_falls_back_to_page_cache() {
        // Regression: Passthrough on GET must still let the page cache
        // serve the URL.
        let dispatcher = Arc::new(RecordingDispatcher {
            last: tokio::sync::Mutex::new(None),
            outcome: PluginDispatchOutcome::Passthrough,
        });
        let state = state_with_dispatcher(dispatcher.clone(), "/maybe");
        state
            .pages
            .insert("/maybe", "<html><body>cached</body></html>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/maybe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("cached"), "body: {body}");
    }

    #[tokio::test]
    async fn body_bytes_to_utf8_string_drops_empty_and_non_utf8() {
        // Empty body collapses to None.
        assert!(body_bytes_to_utf8_string(&Bytes::new()).is_none());
        // UTF-8 round-trips.
        assert_eq!(
            body_bytes_to_utf8_string(&Bytes::from("hello")),
            Some("hello".to_string())
        );
        // Non-UTF-8 is dropped (binary upload is a future extension).
        let bad = Bytes::from(vec![0xff, 0xfe, 0xfd]);
        assert!(body_bytes_to_utf8_string(&bad).is_none());
    }

    // ---- base prefix mounting (issue #229) -------------------------------
    //
    // The router-level fixture is identical to `test_router` except it
    // builds the AppState with `base_prefix: Some("/foo")`. With the
    // prefix set, every dev-server route (pages, livereload script,
    // SSE endpoint) must mount under `/foo/...`; bare unprefixed
    // requests must be redirected (for `/`) or return the
    // base-aware 404 hint.

    fn test_router_with_base(state: AppState) -> Router {
        build_router(state)
    }

    #[tokio::test]
    async fn base_prefix_serves_root_page_under_prefix() {
        let state = test_state_with_base("/foo");
        state
            .pages
            .insert("/", "<html><body><h1>home</h1></body></html>")
            .await;
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(Request::builder().uri("/foo/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("<h1>home</h1>"), "body: {body}");
        // The injected livereload script must point at the prefixed URL
        // (otherwise the browser hits /__zfb/livereload.js and 404s).
        assert!(
            body.contains("<script src=\"/foo/__zfb/livereload.js\"></script>"),
            "expected prefixed livereload tag, body: {body}"
        );
    }

    #[tokio::test]
    async fn base_prefix_serves_nested_page_under_prefix() {
        let state = test_state_with_base("/foo");
        state
            .pages
            .insert("/about", "<html><body>about</body></html>")
            .await;
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/foo/about")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("about"), "body: {body}");
        assert!(
            body.contains("<script src=\"/foo/__zfb/livereload.js\"></script>"),
            "expected prefixed livereload tag, body: {body}"
        );
    }

    #[tokio::test]
    async fn base_prefix_redirects_bare_root_to_prefix() {
        let state = test_state_with_base("/foo");
        // No page inserted — the redirect happens before any page
        // lookup and doesn't depend on the cache.
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            location, "/foo/",
            "expected redirect to /foo/, got {location}"
        );
    }

    #[tokio::test]
    async fn base_prefix_redirects_bare_root_with_query_to_prefix() {
        // GET /?x=1 must redirect to /foo/?x=1 — the query string must
        // be preserved so shareable URLs (e.g. /?c=1a) survive the
        // base-prefix redirect without silently losing their params.
        let state = test_state_with_base("/foo");
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(Request::builder().uri("/?x=1").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            location, "/foo/?x=1",
            "query string must be carried through the bare-root redirect (got {location:?})"
        );
    }

    #[tokio::test]
    async fn base_prefix_does_not_redirect_prefix_root_to_itself() {
        // The redirect-loop avoidance the issue's review explicitly
        // calls out: GET /foo/ must serve the home page directly, not
        // 302 back to itself.
        let state = test_state_with_base("/foo");
        state
            .pages
            .insert("/", "<html><body><h1>home</h1></body></html>")
            .await;
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(Request::builder().uri("/foo/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "GET /foo/ must serve, not redirect"
        );
    }

    #[tokio::test]
    async fn base_prefix_serves_livereload_js_under_prefix() {
        let state = test_state_with_base("/foo");
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/foo/__zfb/livereload.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            ct.starts_with("application/javascript"),
            "unexpected content-type: {ct}"
        );
        let body = body_string(resp).await;
        assert!(body.contains("EventSource"), "body missing EventSource");
    }

    #[tokio::test]
    async fn base_prefix_returns_hinted_404_for_unprefixed_path() {
        let state = test_state_with_base("/foo");
        let router = test_router_with_base(state);

        // Asset request without the base prefix — the typical
        // "stale HTML referenced /assets/main.css instead of
        // /foo/assets/main.css" case. The dev server returns a 404
        // pointing at the configured base.
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/assets/main.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_string(resp).await;
        assert!(
            body.contains("/foo"),
            "expected 404 body to mention the configured base, body: {body}"
        );
    }

    #[tokio::test]
    async fn base_prefix_unknown_prefixed_path_uses_dev_404() {
        // A request UNDER the base prefix that doesn't match the page
        // cache should still get the regular dev-mode 404 body (which
        // is the page-cache miss path) — not the "outside base" hint.
        let state = test_state_with_base("/foo");
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/foo/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_string(resp).await;
        assert!(
            body.contains("page not in cache"),
            "expected dev-mode 404 body, got: {body}"
        );
        // Even the 404 page should carry the prefixed livereload tag
        // so the page upgrades automatically once the route lands.
        assert!(
            body.contains("<script src=\"/foo/__zfb/livereload.js\"></script>"),
            "expected prefixed livereload tag in 404, got: {body}"
        );
    }

    #[tokio::test]
    async fn no_base_prefix_keeps_root_serving_directly() {
        // Regression guard: with `base_prefix: None` the router must
        // not introduce any redirect for `/` — that would break every
        // existing dev session.
        let state = test_state(); // base_prefix = None
        state
            .pages
            .insert("/", "<html><body>home</body></html>")
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "GET / must serve directly when no base is set"
        );
    }

    // ---- dev-mode link rewrite (zfb#228 + zfb#229 follow-up) -----------
    //
    // Codex review of the merge caught that the dev server was serving
    // cached HTML verbatim, so user-authored `<a href="/about">` and
    // `<form action="/login">` literals weren't rewritten under a
    // `base` mount. Production builds rewrite on disk; the dev server
    // now mirrors that in-flight via `page_response_bytes`. These
    // tests pin both directions.

    #[tokio::test]
    async fn dev_rewrites_user_authored_a_href_under_base() {
        let state = test_state_with_base("/foo");
        state
            .pages
            .insert(
                "/",
                "<html><body><a href=\"/about\">About</a></body></html>",
            )
            .await;
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(Request::builder().uri("/foo/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(
            body.contains(r#"href="/foo/about""#),
            "user a-href should be rewritten under base; got: {body}"
        );
        // Bare unprefixed href must NOT survive — the whole point of
        // the fix is that clicking the rendered link lands inside the
        // configured base, not at the unprefixed root.
        assert!(
            !body.contains(r#"href="/about""#),
            "unprefixed href leaked into served HTML: {body}"
        );
    }

    #[tokio::test]
    async fn dev_rewrites_user_authored_form_action_under_base() {
        let state = test_state_with_base("/foo");
        state
            .pages
            .insert(
                "/",
                "<html><body><form action=\"/login\"><input/></form></body></html>",
            )
            .await;
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(Request::builder().uri("/foo/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(
            body.contains(r#"action="/foo/login""#),
            "form action should be rewritten under base; got: {body}"
        );
    }

    #[tokio::test]
    async fn dev_no_base_keeps_user_links_unchanged() {
        // Regression guard: existing projects without `base` set must
        // see byte-identical user links in the served HTML.
        let state = test_state();
        state
            .pages
            .insert(
                "/",
                "<html><body><a href=\"/about\">About</a></body></html>",
            )
            .await;
        let router = test_router(state);

        let resp = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains(r#"href="/about""#),
            "no-base mode should not rewrite user href: {body}"
        );
    }

    #[tokio::test]
    async fn dev_rewrite_skips_data_no_base_opt_out() {
        let state = test_state_with_base("/foo");
        state
            .pages
            .insert(
                "/",
                "<html><body><a href=\"/legal\" data-no-base>legal</a></body></html>",
            )
            .await;
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(Request::builder().uri("/foo/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains(r#"href="/legal""#),
            "data-no-base opt-out should preserve original href: {body}"
        );
        assert!(
            !body.contains("/foo/legal"),
            "opt-out href should not be prefixed: {body}"
        );
    }

    /// A public file must remain reachable when the dev server runs
    /// under a `base` prefix. `GET /foo/logo.svg` with `base: "/foo"`
    /// resolves to `<public_root>/logo.svg` — the prefix is stripped
    /// before the on-disk fallback runs, matching the production
    /// `copy_public_dir` layout where `public/*` lands under the same
    /// `<base-segment>/` in `dist/`.
    #[tokio::test]
    async fn base_prefix_serves_public_file_at_prefixed_root() {
        // Stage a real public_root with a fixture file. The default
        // test_state_with_base uses a bogus path; override it here so
        // the on-disk read actually finds something.
        let tmp = tempfile::tempdir().expect("tempdir");
        let public_root = tmp.path().to_path_buf();
        std::fs::write(public_root.join("logo.svg"), b"<svg/>").unwrap();

        let mut state = test_state_with_base("/foo");
        state.public_root = public_root;
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/foo/logo.svg")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1_000_000).await.unwrap();
        assert_eq!(bytes.as_ref(), b"<svg/>");
    }

    /// Sibling check: under a `base` prefix, requesting the same file
    /// WITHOUT the prefix must 404 (lands in the outside-base 404
    /// handler), not silently serve the public file from the root.
    /// This locks in that the public on-disk fallback is gated by the
    /// prefix-stripping in `serve_page` and not reachable via a bare
    /// unprefixed URL.
    #[tokio::test]
    async fn base_prefix_unprefixed_public_path_is_not_served() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let public_root = tmp.path().to_path_buf();
        std::fs::write(public_root.join("logo.svg"), b"<svg/>").unwrap();

        let mut state = test_state_with_base("/foo");
        state.public_root = public_root;
        let router = test_router_with_base(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/logo.svg")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "bare unprefixed asset URL must not bypass the base prefix"
        );
    }

    // -------------------------------------------------------------------
    // Issue #899 — symlink-containment: symlinks pointing outside the
    // served root must be blocked; legitimate in-root symlinks must work.
    // -------------------------------------------------------------------

    /// A symlink inside `dist/` pointing OUTSIDE it must yield a 404,
    /// not silently serve the out-of-root target.
    #[cfg(unix)]
    #[tokio::test]
    async fn read_from_dist_rejects_out_of_root_symlink() {
        use std::os::unix::fs::symlink;

        let outside = tempfile::tempdir().expect("outside dir");
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();

        let dist = tempfile::tempdir().expect("dist dir");
        // Plant a symlink inside dist pointing at the outside file.
        symlink(
            outside.path().join("secret.txt"),
            dist.path().join("escape.txt"),
        )
        .unwrap();

        let result = read_from_dist(dist.path(), "escape.txt").await;
        assert!(
            result.is_none(),
            "out-of-root symlink in dist must not be served"
        );
    }

    /// A real file inside `dist/` is still served normally — the
    /// containment check must not break legitimate files.
    #[tokio::test]
    async fn read_from_dist_serves_real_in_root_file() {
        let dist = tempfile::tempdir().expect("dist dir");
        std::fs::write(dist.path().join("page.html"), b"<h1>hello</h1>").unwrap();

        let result = read_from_dist(dist.path(), "page.html").await;
        assert_eq!(result.as_deref(), Some(b"<h1>hello</h1>".as_ref()));
    }

    /// A symlink inside `dist/` that points to another file WITHIN `dist/`
    /// must still be served — in-root symlinks are legitimate.
    #[cfg(unix)]
    #[tokio::test]
    async fn read_from_dist_serves_in_root_symlink() {
        use std::os::unix::fs::symlink;

        let dist = tempfile::tempdir().expect("dist dir");
        std::fs::write(dist.path().join("real.html"), b"<h1>real</h1>").unwrap();
        // Symlink inside dist pointing at another file inside dist.
        symlink(
            dist.path().join("real.html"),
            dist.path().join("alias.html"),
        )
        .unwrap();

        let result = read_from_dist(dist.path(), "alias.html").await;
        assert_eq!(
            result.as_deref(),
            Some(b"<h1>real</h1>".as_ref()),
            "in-root symlink must be served"
        );
    }

    /// A symlink inside `public/` pointing OUTSIDE it must yield None.
    #[cfg(unix)]
    #[tokio::test]
    async fn read_from_public_rejects_out_of_root_symlink() {
        use std::os::unix::fs::symlink;

        let outside = tempfile::tempdir().expect("outside dir");
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();

        let public = tempfile::tempdir().expect("public dir");
        symlink(
            outside.path().join("secret.txt"),
            public.path().join("escape.txt"),
        )
        .unwrap();

        let result = read_from_public(public.path(), "escape.txt").await;
        assert!(
            result.is_none(),
            "out-of-root symlink in public must not be served"
        );
    }

    /// A real file inside `public/` is still served normally.
    #[tokio::test]
    async fn read_from_public_serves_real_in_root_file() {
        let public = tempfile::tempdir().expect("public dir");
        std::fs::write(public.path().join("logo.svg"), b"<svg/>").unwrap();

        let result = read_from_public(public.path(), "logo.svg").await;
        assert_eq!(result.as_deref(), Some(b"<svg/>".as_ref()));
    }

    /// A symlink inside `public/` that points to another file WITHIN
    /// `public/` must still be served.
    #[cfg(unix)]
    #[tokio::test]
    async fn read_from_public_serves_in_root_symlink() {
        use std::os::unix::fs::symlink;

        let public = tempfile::tempdir().expect("public dir");
        std::fs::write(public.path().join("real.svg"), b"<svg/>").unwrap();
        symlink(
            public.path().join("real.svg"),
            public.path().join("alias.svg"),
        )
        .unwrap();

        let result = read_from_public(public.path(), "alias.svg").await;
        assert_eq!(
            result.as_deref(),
            Some(b"<svg/>".as_ref()),
            "in-root symlink in public must be served"
        );
    }

    /// End-to-end router test: a symlink inside dist pointing outside
    /// the root must produce a 404 response via the dev server.
    #[cfg(unix)]
    #[tokio::test]
    async fn dev_server_rejects_out_of_root_symlink_in_dist() {
        use std::os::unix::fs::symlink;

        let outside = tempfile::tempdir().expect("outside dir");
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();

        let dist = tempfile::tempdir().expect("dist dir");
        symlink(
            outside.path().join("secret.txt"),
            dist.path().join("escape.txt"),
        )
        .unwrap();

        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let state = AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.path().to_path_buf(),
            html_root: dist.path().to_path_buf(),
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: None,
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        };
        let router = build_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/escape.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(
            resp.status(),
            StatusCode::OK,
            "out-of-root symlink in dist must not be served (got {:?})",
            resp.status()
        );
    }

    /// End-to-end router test: a symlink inside public pointing outside
    /// the root must produce a 404 response via the dev server.
    #[cfg(unix)]
    #[tokio::test]
    async fn dev_server_rejects_out_of_root_symlink_in_public() {
        use std::os::unix::fs::symlink;

        let outside = tempfile::tempdir().expect("outside dir");
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();

        let public = tempfile::tempdir().expect("public dir");
        symlink(
            outside.path().join("secret.txt"),
            public.path().join("escape.txt"),
        )
        .unwrap();

        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let dist = tempfile::tempdir().expect("dist dir");
        let state = AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.path().to_path_buf(),
            html_root: dist.path().to_path_buf(),
            public_root: public.path().to_path_buf(),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: None,
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        };
        let router = build_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/escape.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(
            resp.status(),
            StatusCode::OK,
            "out-of-root symlink in public must not be served (got {:?})",
            resp.status()
        );
    }

    /// End-to-end router test: a symlink inside `dist/assets/` pointing
    /// outside the dist root must produce a 404 for the `/assets/` route.
    #[cfg(unix)]
    #[tokio::test]
    async fn dev_server_rejects_out_of_root_symlink_in_assets() {
        use std::os::unix::fs::symlink;

        let outside = tempfile::tempdir().expect("outside dir");
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();

        let dist = tempfile::tempdir().expect("dist dir");
        let assets_dir = dist.path().join("assets");
        std::fs::create_dir_all(&assets_dir).expect("mk assets dir");
        // Symlink inside assets pointing to a file outside dist.
        symlink(
            outside.path().join("secret.txt"),
            assets_dir.join("evil.txt"),
        )
        .unwrap();

        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let state = AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.path().to_path_buf(),
            html_root: dist.path().to_path_buf(),
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: None,
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        };
        let router = build_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/assets/evil.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "out-of-root symlink in dist/assets must not be served (got {:?})",
            resp.status()
        );
    }

    /// End-to-end router test: a symlink inside `dist/assets/` pointing
    /// to another file **inside** `dist/assets/` must still be served.
    #[cfg(unix)]
    #[tokio::test]
    async fn dev_server_serves_in_root_symlink_in_assets() {
        use std::os::unix::fs::symlink;

        let dist = tempfile::tempdir().expect("dist dir");
        let assets_dir = dist.path().join("assets");
        std::fs::create_dir_all(&assets_dir).expect("mk assets dir");
        std::fs::write(assets_dir.join("real.css"), b"body{}").unwrap();
        // Symlink inside assets pointing to another file inside assets.
        symlink(assets_dir.join("real.css"), assets_dir.join("alias.css")).unwrap();

        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let state = AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.path().to_path_buf(),
            html_root: dist.path().to_path_buf(),
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: None,
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        };
        let router = build_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/assets/alias.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "in-root symlink in dist/assets must be served (got {:?})",
            resp.status()
        );
    }

    /// End-to-end router test: a normal (non-symlink) asset in `dist/assets/`
    /// must be served with the correct content type.
    #[tokio::test]
    async fn dev_server_serves_normal_asset_with_correct_content_type() {
        let dist = tempfile::tempdir().expect("dist dir");
        let assets_dir = dist.path().join("assets");
        std::fs::create_dir_all(&assets_dir).expect("mk assets dir");
        std::fs::write(assets_dir.join("main.js"), b"console.log(1)").unwrap();

        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let state = AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.path().to_path_buf(),
            html_root: dist.path().to_path_buf(),
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: None,
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        };
        let router = build_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/assets/main.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("javascript") || ct.contains("application/js"),
            "normal asset in dist/assets must have JS content-type, got: {ct}"
        );
    }

    /// End-to-end router test: a HEAD request to an asset in `dist/assets/`
    /// must be served (200) with an empty body.
    #[tokio::test]
    async fn dev_server_head_request_for_asset_is_ok() {
        let dist = tempfile::tempdir().expect("dist dir");
        let assets_dir = dist.path().join("assets");
        std::fs::create_dir_all(&assets_dir).expect("mk assets dir");
        std::fs::write(assets_dir.join("style.css"), b"body{}").unwrap();

        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let state = AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.path().to_path_buf(),
            html_root: dist.path().to_path_buf(),
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: None,
            host_validation: crate::host_validation::HostValidation::disabled(),
            render_on_request_hook: None,
        };
        let router = build_router(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/assets/style.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "HEAD request for existing asset must return 200"
        );
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert!(
            body_bytes.is_empty(),
            "HEAD response body must be empty (got {} bytes)",
            body_bytes.len()
        );
    }

    // --- LAN security: origin_rejection unit tests -----------------------
    //
    // These tests call `origin_rejection` directly to cover the
    // fail-closed missing-Origin path and the loopback short-circuit
    // without needing a full plugin/SSR stack wired up.

    fn enforced_state() -> AppState {
        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let dist = std::env::temp_dir().join("zfb-test-dist");
        AppState {
            mode: crate::ServerMode::Dev,
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            embed_handlers: None,
            dist_root: dist.clone(),
            html_root: dist,
            public_root: std::env::temp_dir().join("zfb-test-public"),
            base_prefix: None,
            trailing_slash: false,
            islands_bundle_url: None,
            css_bundle_url: None,
            // Non-loopback bind → enforcement on.
            host_validation: crate::host_validation::HostValidation::for_bind(
                std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                None,
                &[],
                crate::ServerMode::Dev,
            ),
            render_on_request_hook: None,
        }
    }

    #[test]
    fn origin_rejection_get_is_always_allowed() {
        let state = enforced_state();
        let headers = HeaderMap::new();
        // GET with no Origin must pass even when enforced.
        let result = origin_rejection(&state, &Method::GET, &headers);
        assert!(
            result.is_none(),
            "GET without Origin must not be rejected when enforced"
        );
        // HEAD likewise.
        let result = origin_rejection(&state, &Method::HEAD, &headers);
        assert!(
            result.is_none(),
            "HEAD without Origin must not be rejected when enforced"
        );
    }

    #[test]
    fn origin_rejection_missing_origin_rejected_when_enforced() {
        let state = enforced_state();
        let headers = HeaderMap::new(); // no Origin
        let result = origin_rejection(&state, &Method::POST, &headers);
        assert!(
            result.is_some(),
            "POST without Origin must be 403 when enforced"
        );
        let resp = result.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "missing Origin on enforced server must be 403"
        );
    }

    #[test]
    fn origin_rejection_missing_origin_allowed_on_loopback() {
        let state = test_state(); // host_validation: disabled (loopback)
        let headers = HeaderMap::new();
        // Not enforced — missing Origin must not reject.
        let result = origin_rejection(&state, &Method::POST, &headers);
        assert!(
            result.is_none(),
            "POST without Origin must be allowed on loopback-bound server"
        );
    }

    #[test]
    fn origin_rejection_present_allowed_origin_passes() {
        let state = enforced_state();
        let mut headers = HeaderMap::new();
        // localhost is always in the built-in allowlist.
        headers.insert(header::ORIGIN, "http://localhost:3000".parse().unwrap());
        let result = origin_rejection(&state, &Method::POST, &headers);
        assert!(
            result.is_none(),
            "POST from localhost Origin must pass when enforced"
        );
    }

    #[test]
    fn origin_rejection_disallowed_origin_rejected_when_enforced() {
        let state = enforced_state();
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "https://evil.test".parse().unwrap());
        let result = origin_rejection(&state, &Method::POST, &headers);
        assert!(
            result.is_some(),
            "POST from disallowed Origin must be 403 when enforced"
        );
        assert_eq!(result.unwrap().status(), StatusCode::FORBIDDEN);
    }

    // --- LAN security: DefaultBodyLimit (body size cap) ---------------

    #[tokio::test]
    async fn router_rejects_oversized_post_body() {
        let state = test_state();
        let router = test_router(state);

        // 2 MiB + 1 byte — just over the cap set in build_router.
        let oversized = vec![b'x'; 2 * 1024 * 1024 + 1];
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .header(header::CONTENT_LENGTH, oversized.len().to_string())
                    .body(Body::from(oversized))
                    .unwrap(),
            )
            .await
            .unwrap();

        // axum returns 413 Payload Too Large when DefaultBodyLimit is exceeded.
        assert_eq!(
            resp.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "POST body exceeding 2 MiB cap must be rejected with 413"
        );
    }
}
