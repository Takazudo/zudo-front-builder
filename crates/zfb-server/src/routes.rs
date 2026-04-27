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
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::sync::RwLock;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::inject::inject_livereload;
use crate::livereload::{sse_response, ReloadTx};

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedPage {
    /// Body to send back to the browser.
    pub body: String,
    /// Optional explicit `Content-Type` (typically supplied by a page's
    /// `export const contentType = "…"` frontmatter via Sub 1's
    /// extractor). `None` means "derive from the URL/extension".
    pub content_type: Option<String>,
}

impl CachedPage {
    /// Convenience: build an HTML cache entry with no explicit
    /// content-type override (the server will derive one).
    pub fn html(body: impl Into<String>) -> Self {
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
    pub async fn insert_with_content_type(
        &self,
        key: impl Into<String>,
        body: impl Into<String>,
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
/// `Content-Type` header. The mapping is intentionally short and only
/// covers the extensions a static-site page is likely to emit; the
/// catch-all is `text/html; charset=utf-8` because the dev server's
/// page cache is HTML-by-default. Pages that need an exotic
/// extension should set `export const contentType = "…"` in their
/// frontmatter and rely on the [`CachedPage::content_type`] override
/// path instead.
///
/// Mirrors [`zfb_render::meta::derive_content_type`]; keep both in
/// sync (and ADR-003).
pub fn content_type_for_extension(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "xml" => "application/xml",
        "rss" => "application/rss+xml",
        "atom" => "application/atom+xml",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" | "cjs" => "application/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        _ => "text/html; charset=utf-8",
    }
}

/// Pick the `Content-Type` for a page-cache entry, applying the
/// documented precedence:
///
/// 1. The entry's `content_type` override, if `Some`,
/// 2. else [`content_type_for_extension`] applied to `url_path`'s
///    trailing extension,
/// 3. else `text/html; charset=utf-8`.
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
    content_type_for_extension(ext).to_string()
}

/// Shared state for the route handlers.
#[derive(Clone)]
pub struct AppState {
    /// Page cache that renderers populate.
    pub pages: PageCache,
    /// Live-reload broadcast sender — cloned per SSE subscription.
    pub broadcast: ReloadTx,
}

/// Build the axum router for the dev server.
///
/// `dist_root` is the build output directory (used for `/assets/*`).
/// `public_root` is the project's static assets directory (used for
/// `/public/*`). Both are served via [`ServeDir`]; missing files fall
/// back to a plain 404.
pub fn build_router(
    state: AppState,
    dist_root: std::path::PathBuf,
    public_root: std::path::PathBuf,
) -> Router {
    let assets_dir = dist_root.join("assets");
    let assets_service = ServeDir::new(&assets_dir);
    let public_service = ServeDir::new(&public_root);

    Router::new()
        .route("/__zfb/livereload.js", get(livereload_js))
        .route("/__zfb/reload", get(sse_handler))
        .nest_service("/assets", assets_service)
        .nest_service("/public", public_service)
        .route("/", get(page_root))
        .route("/{*path}", get(page_handler))
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

/// Handler for `GET /` — serve the root page.
pub async fn page_root(State(state): State<AppState>) -> Response {
    serve_page(&state, "").await
}

/// Handler for `GET /*path` — serve any other rendered page.
pub async fn page_handler(
    State(state): State<AppState>,
    AxumPath(path): AxumPath<String>,
) -> Response {
    serve_page(&state, &path).await
}

async fn serve_page(state: &AppState, raw_path: &str) -> Response {
    // Strip any leading slash from the captured wildcard so we can
    // build canonical lookup keys ourselves.
    let trimmed = raw_path.trim_start_matches('/');

    let candidates = lookup_keys(trimmed);
    for key in &candidates {
        if let Some(entry) = state.pages.get(key).await {
            // Pick the Content-Type per the precedence rule:
            //   override on the cache entry > URL extension default
            //   > text/html. The cache key wins over `raw_path`
            //   because the matched key reflects what the renderer
            //   actually emitted (e.g. `/sitemap.xml`).
            let content_type = resolve_content_type(&entry, key);
            let is_html = content_type
                .to_ascii_lowercase()
                .starts_with("text/html");
            return page_response(StatusCode::OK, &entry.body, &content_type, is_html);
        }
    }

    // 404 is always the dev HTML body so the page is replaced once
    // a real one lands. The live-reload script gets injected.
    page_response(
        StatusCode::NOT_FOUND,
        DEV_404_BODY,
        "text/html; charset=utf-8",
        true,
    )
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
    // Drop a trailing slash for the "exact" candidate so callers that
    // store `/blog/foo` still match a request to `/blog/foo/`.
    let stripped = path.trim_end_matches('/');
    let mut out = Vec::with_capacity(3);
    out.push(format!("/{stripped}"));
    out.push(format!("/{stripped}/index.html"));
    if path.ends_with('/') || stripped != path {
        // request had a trailing slash — also try the slash key
        out.push(format!("/{path}"));
    } else {
        out.push(format!("/{stripped}/"));
    }
    out
}

/// Build a page response with a custom `Content-Type`, injecting the
/// live-reload tag only for HTML responses (Sub 49: non-HTML pages
/// like RSS feeds or sitemaps must not be polluted with a script
/// tag — they'd fail XML parsers).
fn page_response(
    status: StatusCode,
    body: &str,
    content_type: &str,
    inject_reload: bool,
) -> Response {
    let body_out = if inject_reload {
        inject_livereload(body)
    } else {
        body.to_string()
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
        }
    }

    fn test_router(state: AppState) -> Router {
        // Use bogus dist/public roots — ServeDir will simply 404, which
        // is fine: these tests don't exercise asset routing (Sub 6
        // covers integration with a real fixture project).
        let tmp = std::env::temp_dir();
        build_router(
            state,
            tmp.join("zfb-test-dist"),
            tmp.join("zfb-test-public"),
        )
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
        assert_eq!(content_type_for_extension("html"), "text/html; charset=utf-8");
        assert_eq!(content_type_for_extension("xml"), "application/xml");
        assert_eq!(content_type_for_extension("rss"), "application/rss+xml");
        assert_eq!(content_type_for_extension("json"), "application/json");
        assert_eq!(content_type_for_extension("txt"), "text/plain; charset=utf-8");
    }

    #[test]
    fn content_type_for_extension_unknown_falls_back_to_html() {
        assert_eq!(
            content_type_for_extension("weird"),
            "text/html; charset=utf-8",
        );
    }

    #[test]
    fn resolve_content_type_uses_override_first() {
        let entry = CachedPage {
            body: "<rss/>".into(),
            content_type: Some("application/rss+xml".into()),
        };
        // URL says `.xml`, but the override (`application/rss+xml`)
        // wins per the precedence rule.
        assert_eq!(resolve_content_type(&entry, "/feed.xml"), "application/rss+xml");
    }

    #[test]
    fn resolve_content_type_falls_back_to_url_extension() {
        let entry = CachedPage {
            body: "<urlset/>".into(),
            content_type: None,
        };
        assert_eq!(resolve_content_type(&entry, "/sitemap.xml"), "application/xml");
        assert_eq!(resolve_content_type(&entry, "/llms.txt"), "text/plain; charset=utf-8");
    }

    #[test]
    fn resolve_content_type_html_default_for_extensionless() {
        let entry = CachedPage {
            body: "<p>x</p>".into(),
            content_type: None,
        };
        // No extension on the URL → HTML default.
        assert_eq!(resolve_content_type(&entry, "/about"), "text/html; charset=utf-8");
        assert_eq!(resolve_content_type(&entry, "/"), "text/html; charset=utf-8");
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
        state.pages.insert("/about", "<html><body>x</body></html>").await;
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
}
