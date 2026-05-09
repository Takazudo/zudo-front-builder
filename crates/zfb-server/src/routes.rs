//! Axum routes for the dev server.
//!
//! ## Route map
//!
//! - `GET /assets/*path` — static files from `<dist_root>/assets/`,
//!   served via [`tower_http::services::ServeDir`].
//! - `GET /public/*path` — static files from `<public_root>/`,
//!   served via [`tower_http::services::ServeDir`].
//! - `GET /__zfb/livereload.js` — bundled JS that opens an SSE
//!   connection back to this server. Always served with
//!   `Cache-Control: no-store`.
//! - `GET /__zfb/reload` — SSE event stream. See
//!   [`crate::livereload::sse_response`].
//! - `GET /` and `GET /*path` — render HTML out of the in-memory page
//!   cache. See [`PageCache`] for keying conventions.
//!
//! ## Page key resolution
//!
//! For a request to `/blog/foo` we look up the cache in this order:
//!
//! 1. `/blog/foo`
//! 2. `/blog/foo/index.html`
//! 3. `/blog/foo/` (trailing slash — useful when the renderer keys by
//!    directory-style path)
//!
//! For a request to `/` we look up `/` and then `/index.html`. First
//! hit wins. Misses respond with the dev-mode 404 body
//! ([`DEV_404_BODY`]).
//!
//! All HTML responses (including 404) go through
//! [`crate::inject::inject_livereload`] before being returned, so every
//! served page wires itself up to the live-reload SSE stream.

use std::collections::HashMap;
use std::path::{Component, PathBuf};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::Router;
use tokio::sync::RwLock;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::inject::inject_livereload;
use crate::livereload::{sse_response, ReloadTx};
use crate::plugin_middleware::{
    DevMiddlewareSet, PluginDispatchOutcome, PluginRegistration, PluginRequest,
    PluginResponseEncoding,
};

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
    /// Page cache that renderers populate.
    pub pages: PageCache,
    /// Live-reload broadcast sender — cloned per SSE subscription.
    pub broadcast: ReloadTx,
    /// Optional plugin dev-middleware set. `None` when no user plugins
    /// declared a `devMiddleware` hook.
    pub plugins: Option<DevMiddlewareSet>,
    /// Build output directory, used as a disk fallback when a page is
    /// not yet in the in-memory cache. `serve_page` reads
    /// `<dist_root>/<path>/index.html` when the cache misses.
    pub dist_root: std::path::PathBuf,
}

/// Build the axum router for the dev server.
///
/// `dist_root` is the build output directory (used for `/assets/*`).
/// `public_root` is the project's static assets directory (used for
/// `/public/*`). Both are served via [`ServeDir`]; missing files fall
/// back to a plain 404.
///
/// ## Method policy (issue #230)
///
/// Built-in routes keep their existing GET-only semantics:
///
/// - `/__zfb/livereload.js`, `/__zfb/reload`, `/assets/*`, `/public/*`
///   are all registered with `get(...)` or via [`ServeDir`] (which is
///   GET/HEAD-only). A non-GET request to any of these surfaces gets a
///   `405 Method Not Allowed` from axum / tower-http directly — those
///   routes are dev-server infrastructure, not user code, and CSRF
///   posture for them must stay tight.
/// - The page-renderer mounts (`/` and `/{*path}`) accept ALL methods
///   so user-registered `devMiddleware` handlers can serve `POST`,
///   `PUT`, `DELETE`, `PATCH`, etc. on prefixes they claim. The handler
///   itself in [`serve_page`] still returns `405 Allow: GET, HEAD` if a
///   non-GET request slips through to the page-cache fallback (i.e. no
///   plugin claimed the URL or a plugin returned `Passthrough`).
pub fn build_router(state: AppState, public_root: std::path::PathBuf) -> Router {
    let assets_dir = state.dist_root.join("assets");
    let assets_service = ServeDir::new(&assets_dir);
    let public_service = ServeDir::new(&public_root);

    Router::new()
        .route("/__zfb/livereload.js", get(livereload_js))
        .route("/__zfb/reload", get(sse_handler))
        .nest_service("/assets", assets_service)
        .nest_service("/public", public_service)
        // `any` (vs `get`) so user-registered devMiddleware handlers can
        // serve every HTTP method. `serve_page` enforces GET/HEAD-only
        // for the page-cache fallback when no plugin claims the URL.
        .route("/", any(page_root))
        .route("/{*path}", any(page_handler))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
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
    body: Bytes,
) -> Response {
    serve_page(&state, "/", &uri, method, headers, body).await
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
    body: Bytes,
) -> Response {
    serve_page(&state, &path, &uri, method, headers, body).await
}

async fn serve_page(
    state: &AppState,
    raw_path: &str,
    uri: &Uri,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Strip any leading slash from the captured wildcard so we can
    // build canonical lookup keys ourselves.
    let trimmed = raw_path.trim_start_matches('/');

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
    // reject the method. Built-in routes (`/__zfb/...`, `/assets/*`,
    // `/public/*`) are unaffected — those are routed by axum directly
    // and stay GET-only.
    if let Some(set) = state.plugins.as_ref() {
        let path_only = format!("/{trimmed}");
        if let Some(reg) = set.find_match(&path_only) {
            // Path + optional query.
            let full = match uri.path_and_query() {
                Some(pq) => pq.as_str().to_string(),
                None => path_only.clone(),
            };
            let plugin_headers = headermap_to_string_map(&headers);
            let plugin_body = body_bytes_to_utf8_string(&body);
            match dispatch_plugin(
                set,
                reg,
                &full,
                method.as_str(),
                plugin_headers,
                plugin_body,
            )
            .await
            {
                PluginDispatchAttempt::Responded(resp) => return resp,
                PluginDispatchAttempt::Passthrough => {}
                PluginDispatchAttempt::Errored(resp) => return resp,
            }
        }
    }

    // No plugin handled this request. The dev page-cache fallback is
    // GET/HEAD-only — anything else 405s here so a misrouted POST does
    // not silently get treated as a page lookup.
    if !is_get_like {
        return method_not_allowed_get_head();
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
            return page_response_bytes(StatusCode::OK, entry.body, &content_type, is_html);
        }
    }

    // In-memory cache miss: fall back to reading from the dist directory
    // on disk. The dev pipeline writes rendered HTML there after each
    // watcher tick; serving from disk means the browser always sees the
    // latest output even when the in-memory cache hasn't been populated
    // (e.g. on cold start before the first watcher tick fires).
    if let Some(bytes) = read_from_dist(&state.dist_root, trimmed) {
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
        return page_response_bytes(StatusCode::OK, bytes, &content_type, is_html);
    }

    // 404 is always the dev HTML body so the page is replaced once
    // a real one lands. The live-reload script gets injected.
    page_response_bytes(
        StatusCode::NOT_FOUND,
        DEV_404_BODY.as_bytes().to_vec(),
        "text/html; charset=utf-8",
        true,
    )
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
fn read_from_dist(dist_root: &std::path::Path, trimmed: &str) -> Option<Vec<u8>> {
    if !is_safe_url_path(trimmed) {
        return None;
    }
    let candidates: [PathBuf; 2] = [
        dist_root.join(trimmed).join("index.html"),
        dist_root.join(trimmed),
    ];
    for path in &candidates {
        if let Ok(bytes) = std::fs::read(path) {
            return Some(bytes);
        }
    }
    None
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
                            return PluginDispatchAttempt::Errored(plugin_error_response(&msg));
                        }
                    }
                }
                PluginResponseEncoding::Utf8 => resp.body.into_bytes(),
            };
            let mut builder = Response::builder().status(status);
            for (k, v) in resp.headers {
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
                Err(e) => PluginDispatchAttempt::Errored(plugin_error_response(&format!(
                    "failed to build response from plugin `{}`: {e}",
                    reg.plugin,
                ))),
            }
        }
        Ok(PluginDispatchOutcome::Passthrough) => PluginDispatchAttempt::Passthrough,
        Err(err) => PluginDispatchAttempt::Errored(plugin_error_response(&format!(
            "plugin `{}` dev-middleware failed: {}",
            err.plugin, err.message,
        ))),
    }
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

fn plugin_error_response(message: &str) -> Response {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>zfb dev — plugin error</title></head><body><h1>Plugin dev-middleware error</h1><pre>{}</pre></body></html>",
        html_escape(message),
    );
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

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
fn page_response_bytes(
    status: StatusCode,
    body: Vec<u8>,
    content_type: &str,
    inject_reload: bool,
) -> Response {
    let body_out: Vec<u8> = if inject_reload {
        // HTML should always be valid UTF-8; fall back to raw bytes on
        // the rare occasion it isn't so we don't panic in dev mode.
        match std::str::from_utf8(&body) {
            Ok(html) => inject_livereload(html).into_bytes(),
            Err(_) => body,
        }
    } else {
        body
    };

    // The Content-Type may come from a user frontmatter override and
    // therefore can't be statically validated; fall back to a safe
    // default if the value contains characters HTTP rejects.
    let ct_header = HeaderValue::try_from(content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("text/html; charset=utf-8"));

    let mut resp = (status, [(header::CONTENT_TYPE, ct_header)], body_out).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
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
        AppState {
            pages: PageCache::new(),
            broadcast: tx,
            plugins: None,
            dist_root: std::env::temp_dir().join("zfb-test-dist"),
        }
    }

    fn test_router(state: AppState) -> Router {
        // Use bogus dist/public roots — ServeDir will simply 404, which
        // is fine: these tests don't exercise asset routing (Sub 6
        // covers integration with a real fixture project).
        let tmp = std::env::temp_dir();
        build_router(state, tmp.join("zfb-test-public"))
    }

    async fn body_string(resp: Response) -> String {
        let bytes = to_bytes(resp.into_body(), 1_000_000).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
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
        assert!(body.contains("/__zfb/reload"));
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
        ) -> Result<PluginDispatchOutcome, crate::plugin_middleware::PluginDispatchError>
        {
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

    fn state_with_dispatcher(
        dispatcher: Arc<RecordingDispatcher>,
        path: &str,
    ) -> AppState {
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
        AppState {
            pages: PageCache::new(),
            broadcast: tx,
            plugins: Some(set),
            dist_root: std::env::temp_dir().join("zfb-test-dist"),
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
}
