//! Symlink-containment wrapper for the `/assets` [`ServeDir`] route.
//!
//! [`tower_http::services::ServeDir`] performs only lexical sanitisation
//! (percent-decode + reject `..` / root components) and therefore follows
//! symlinks freely.  A symlink planted at `dist/assets/evil → /etc/passwd`
//! would be served without this layer.
//!
//! ## How it works
//!
//! [`ContainedAssetsService`] wraps `ServeDir` and intercepts every request
//! before forwarding it.  It replicates the exact path-building step
//! `ServeDir` uses internally (verified against tower-http 0.6.11 source):
//!
//! 1. Take the URI path after `nest_service` has stripped `/assets`
//!    (so the path starts with `/`).
//! 2. Trim the leading `/`.
//! 3. Percent-decode the remainder.
//! 4. Walk `Path::components()`, accepting only `Normal` components
//!    (skip `CurDir`, reject everything else — same as ServeDir does).
//! 5. Join the accepted components onto `assets_dir`.
//!
//! The resulting FS path is then canonicalized with
//! `tokio::fs::canonicalize`.  A canonicalize error (path does not
//! exist, or a symlink target does not exist) immediately returns 404 —
//! this is consistent with how [`resolve_within_root`](crate::routes) works
//! for the `dist` / `public` fallbacks.  If the canonical path does
//! **not** start with the canonical assets root, it is also 404.
//!
//! Only when the containment check passes does the request reach `ServeDir`,
//! preserving all of its behaviour: HEAD, conditional-GET (`ETag`,
//! `If-Modified-Since`), `Range`, MIME detection, directory redirects.
//!
//! ## TOCTOU narrowing — delegate the canonical path, not the original
//!
//! On pass, the request URI is rewritten to the **canonical** path
//! relative to the canonical root (each segment re-percent-encoded so
//! ServeDir's decode round-trips) before delegating.  ServeDir then
//! opens the already-resolved file instead of re-traversing the original
//! mutable path — without the rewrite, a symlink swap between our
//! containment check and ServeDir's `open()` would bypass the guard.
//! This matches the canonical-path-read narrowing used by
//! `read_from_dist` / `read_from_public` in `routes.rs` (they read the
//! path returned by `resolve_within_root`, not the joined input).
//!
//! Residual race: a *directory* component swapped for a symlink after
//! the check can still redirect the open — the same posture as the
//! other readers.  Full elimination needs openat-style component-wise
//! opening (`O_NOFOLLOW` walk), tracked separately.

use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{Request, Response, StatusCode, Uri};
use percent_encoding::{percent_decode, percent_encode, NON_ALPHANUMERIC};
use tower_http::services::ServeDir;
use tower_service::Service;

/// The concrete request body type the service is parametrised over.
/// Our wrapper is mounted via `nest_service` which uses `axum::body::Body`.
type ReqBody = Body;

/// The concrete inner Service type (ServeDir with default fallback, Body request).
type InnerService = ServeDir;

/// One asset root: its raw dir, the pre-canonicalized form, and the inner
/// [`ServeDir`] that serves files from it.
#[derive(Clone)]
struct AssetRoot {
    /// Canonical form of the assets dir.  Pre-computed at construction time
    /// so the per-request hot path only calls `tokio::fs::canonicalize` once
    /// (for the candidate path).
    ///
    /// `None` means the dir didn't exist at boot (first dev boot before a
    /// build finishes).  Each request retries the canonicalize so assets
    /// start serving as soon as the dir appears.
    ///
    /// Wrapped in `Arc` so cloning (required by Tower) is cheap.
    canonical_root: Arc<Option<PathBuf>>,
    /// The raw (non-canonical) assets directory.
    assets_dir: PathBuf,
    /// Inner service that does the actual file serving for this root.
    inner: InnerService,
}

impl AssetRoot {
    fn new(assets_dir: PathBuf) -> Self {
        // Best-effort pre-canonicalize at boot (sync, outside the hot loop).
        let canonical_root = std::fs::canonicalize(&assets_dir).ok();
        Self {
            canonical_root: Arc::new(canonical_root),
            inner: ServeDir::new(&assets_dir),
            assets_dir,
        }
    }
}

/// A thin Tower [`Service`] that enforces symlink-containment on one or more
/// `assets/` directories before delegating to the inner [`ServeDir`].
///
/// Constructed via [`ContainedAssetsService::new`] (single root) or
/// [`ContainedAssetsService::layered`] (ordered roots).
#[derive(Clone)]
pub(crate) struct ContainedAssetsService {
    /// Asset roots tried IN ORDER on every request: the first root whose
    /// containment check passes AND that actually holds the file serves it;
    /// a miss falls through to the next root.
    ///
    /// `zfb dev` (issue #1189) passes two roots — the isolated
    /// `.zfb-build/dev-assets/assets/` first, then the build `dist/assets/`
    /// as a boot-lazy-seed fallback — so a concurrent `zfb build` that wipes
    /// `dist/` can never 404 dev's stable `/assets/styles.css`. The
    /// namespaces don't collide: dev's isolated dir only holds STABLE names
    /// (`styles.css`, `islands.js`, chunks, `client/*.js`) while a prebuilt
    /// seed only serves HASHED names, so isolated-first never shadows a
    /// legitimately newer hashed seed asset. Single-root callers (preview /
    /// embed / tests) get exactly the historical behavior.
    roots: Vec<AssetRoot>,
}

impl ContainedAssetsService {
    /// Build a single-root [`ContainedAssetsService`] that serves files from
    /// `assets_dir`, rejecting any request whose resolved FS path escapes
    /// that directory (even via symlinks). Byte-identical to the historical
    /// behavior (preview / embed / tests).
    ///
    /// `assets_dir` need not exist at construction time — a missing root
    /// simply means every request will get a 404 (same behaviour as an
    /// empty directory from the client's perspective).
    pub(crate) fn new(assets_dir: PathBuf) -> Self {
        Self {
            roots: vec![AssetRoot::new(assets_dir)],
        }
    }

    /// Build a layered [`ContainedAssetsService`]: the roots are tried in
    /// priority order, and the first one that contains the requested file
    /// serves it. Each root is canonicalized and containment-checked
    /// INDEPENDENTLY, so the symlink-containment guarantee holds for every
    /// root (never canonicalize a merged root). Used by `zfb dev` to put the
    /// isolated dev-assets dir ahead of the build `dist/assets/` (#1189).
    pub(crate) fn layered(assets_dirs: Vec<PathBuf>) -> Self {
        Self {
            roots: assets_dirs.into_iter().map(AssetRoot::new).collect(),
        }
    }

    /// Replicate ServeDir's `build_and_validate_path` logic (tower-http
    /// 0.6.11, `ServeVariant::Directory` branch).
    ///
    /// Returns the FS path candidate that ServeDir *would* access, or
    /// `None` if the path is lexically invalid (non-UTF-8 after decode,
    /// contains a disallowed component type).  Does **not** canonicalize.
    fn candidate_path(assets_dir: &Path, uri_path: &str) -> Option<PathBuf> {
        // ServeDir trims the leading '/' before processing.
        let path = uri_path.trim_start_matches('/');
        // Percent-decode; reject non-UTF-8 sequences.
        let decoded = percent_decode(path.as_bytes()).decode_utf8().ok()?;
        let decoded_path = Path::new(&*decoded);

        let mut result = assets_dir.to_path_buf();
        for component in decoded_path.components() {
            match component {
                Component::Normal(comp) => {
                    // Guard against paths like `/foo/c:/bar` on Windows
                    // (a Normal component can itself contain a drive prefix
                    // on some platforms).
                    if Path::new(comp)
                        .components()
                        .all(|c| matches!(c, Component::Normal(_)))
                    {
                        result.push(comp);
                    } else {
                        return None;
                    }
                }
                // '.' — harmless, skip (ServeDir does the same)
                Component::CurDir => {}
                // '..', '/', drive prefix — reject (ServeDir does the same)
                Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                    return None;
                }
            }
        }
        Some(result)
    }
}

/// The result type for the containment future.
type BoxedFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

impl Service<Request<ReqBody>> for ContainedAssetsService {
    type Response = Response<Body>;
    type Error = std::convert::Infallible;
    type Future = BoxedFuture<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // ServeDir is always immediately ready (no bounded resource pool), so
        // this loop short-circuits on the first poll. Polling every root keeps
        // the contract honest if that ever changes.
        for root in &mut self.roots {
            match <InnerService as Service<Request<ReqBody>>>::poll_ready(&mut root.inner, cx) {
                Poll::Ready(Ok(())) => {}
                // ServeDir's error is `Infallible` — the arm is uninhabited.
                Poll::Ready(Err(e)) => match e {},
                Poll::Pending => return Poll::Pending,
            }
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        let uri_path = req.uri().path().to_owned();
        // Clone the roots so we can move them into the async block.
        let roots = self.roots.clone();

        Box::pin(async move {
            // Phase 1 — find the first root that actually contains the file. A
            // successful `canonicalize` means the file exists; a failure (or a
            // containment-check failure) is a miss for THIS root, so we fall
            // through to the next. ServeDir is invoked only for the winning
            // root (Phase 2), so `req` is moved exactly once regardless of how
            // many roots are probed.
            let mut hit: Option<(PathBuf, PathBuf, InnerService)> = None;
            for root in &roots {
                // Build the candidate FS path using the same logic as ServeDir.
                let Some(candidate) = Self::candidate_path(&root.assets_dir, &uri_path) else {
                    continue;
                };

                // Canonicalize resolves symlinks; if it fails the file/target
                // doesn't exist in this root — try the next root.
                let canonical_candidate = match tokio::fs::canonicalize(&candidate).await {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Determine the canonical root.  Prefer the pre-computed
                // value; fall back to a fresh canonicalize if it was None at
                // boot. Each root is canonicalized INDEPENDENTLY so the
                // containment guarantee holds per-root.
                let canon_root = match root.canonical_root.as_ref() {
                    Some(r) => r.clone(),
                    None => match tokio::fs::canonicalize(&root.assets_dir).await {
                        Ok(p) => p,
                        Err(_) => continue,
                    },
                };

                // Containment check: the canonical candidate must be inside
                // (or equal to) this canonical assets root. An escape is a
                // miss for this root (and will escape the others too) — fall
                // through; the loop ends in a 404.
                if !canonical_candidate.starts_with(&canon_root) {
                    continue;
                }

                hit = Some((canonical_candidate, canon_root, root.inner.clone()));
                break;
            }

            let Some((canonical_candidate, canon_root, mut inner)) = hit else {
                return Ok(not_found());
            };

            // TOCTOU narrowing: rewrite the request URI to the CANONICAL
            // path relative to the canonical root before delegating, so
            // ServeDir opens the already-resolved file instead of
            // re-traversing the original mutable path.  Without this, a
            // symlink swap between the check above and ServeDir's open()
            // would bypass the containment guard.  Each segment is
            // percent-encoded so ServeDir's percent-decode round-trips.
            // (Residual directory-swap race matches read_from_dist /
            // read_from_public — see module docs.)
            let relative = match canonical_candidate.strip_prefix(&canon_root) {
                Ok(r) => r,
                // Unreachable after the starts_with check above; stay
                // graceful rather than panicking in the request path.
                Err(_) => return Ok(not_found()),
            };
            let mut encoded = String::new();
            for component in relative.components() {
                let Some(segment) = component.as_os_str().to_str() else {
                    // A canonical segment that is not valid UTF-8 cannot
                    // round-trip through a URI.  Hashed asset names are
                    // always ASCII, so this is effectively unreachable.
                    return Ok(not_found());
                };
                encoded.push('/');
                encoded.extend(percent_encode(segment.as_bytes(), NON_ALPHANUMERIC));
            }
            if encoded.is_empty() {
                encoded.push('/');
            }
            // Preserve a trailing slash so ServeDir's directory handling
            // (index.html append / redirect) sees the same URI shape as
            // the original request.
            if uri_path.ends_with('/') && !encoded.ends_with('/') {
                encoded.push('/');
            }
            let new_path_and_query = match req.uri().query() {
                Some(q) => format!("{encoded}?{q}"),
                None => encoded,
            };
            let mut uri_parts = req.uri().clone().into_parts();
            uri_parts.path_and_query = match new_path_and_query.parse() {
                Ok(pq) => Some(pq),
                Err(_) => return Ok(not_found()),
            };
            match Uri::from_parts(uri_parts) {
                Ok(u) => *req.uri_mut() = u,
                Err(_) => return Ok(not_found()),
            }

            // Poll readiness then delegate to ServeDir.  The cloned inner
            // service uses the same shared state as the original; ServeDir
            // is always immediately ready (no bounded resource pool).
            //
            // We map the response body from ServeDir's `ServeFileSystemResponseBody`
            // (`UnsyncBoxBody<Bytes, io::Error>`) to `axum::body::Body` so our
            // Service impl has a consistent `Response` type.
            let serve_resp = <InnerService as Service<Request<ReqBody>>>::call(&mut inner, req)
                .await
                // ServeDir is `Infallible` — this unwrap never panics.
                .unwrap_or_else(|e| match e {});

            // Map the response body from `ServeFileSystemResponseBody`
            // (`UnsyncBoxBody<Bytes, io::Error>`) to `axum::body::Body`.
            // `Body::new` accepts any `http_body::Body<Data=Bytes>` whose
            // error is `Into<BoxError>`; `io::Error` satisfies this.
            let (parts, body) = serve_resp.into_parts();
            let axum_body = Body::new(body);
            Ok(Response::from_parts(parts, axum_body))
        })
    }
}

/// Build a minimal 404 response with an empty body.
fn not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::empty())
        .expect("static 404 response is always valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // for `.oneshot()`

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Build a `ContainedAssetsService` backed by a tempdir, run `setup`
    /// inside it, and return `(service, tempdir)`.  The `tempdir` is
    /// returned so the caller keeps it alive for the test duration.
    fn make_service(setup: impl FnOnce(&Path)) -> (ContainedAssetsService, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        setup(dir.path());
        let svc = ContainedAssetsService::new(dir.path().to_path_buf());
        (svc, dir)
    }

    async fn status_for(svc: ContainedAssetsService, path: &str) -> StatusCode {
        let resp = svc
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        resp.status()
    }

    // -----------------------------------------------------------------------
    // Normal asset — must be served (200)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn normal_asset_serves_ok() {
        let (svc, _dir) = make_service(|p| {
            std::fs::write(p.join("style.css"), b"body{}").unwrap();
        });
        let status = status_for(svc, "/style.css").await;
        assert_eq!(status, StatusCode::OK, "normal asset must be served");
    }

    // -----------------------------------------------------------------------
    // Content-Type check (MIME is handled by ServeDir, not the wrapper)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn normal_asset_has_correct_content_type() {
        let (svc, _dir) = make_service(|p| {
            std::fs::write(p.join("main.js"), b"console.log(1)").unwrap();
        });
        let resp = svc
            .oneshot(
                Request::builder()
                    .uri("/main.js")
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
            "expected JS content-type, got: {ct}"
        );
    }

    // -----------------------------------------------------------------------
    // HEAD request — must pass through with no body but correct status
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn head_request_returns_ok_no_body() {
        let (svc, _dir) = make_service(|p| {
            std::fs::write(p.join("font.woff2"), b"\x00\x01\x02").unwrap();
        });
        let resp = svc
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/font.woff2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "HEAD for existing asset must return 200"
        );
        let body_bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert!(
            body_bytes.is_empty(),
            "HEAD response body must be empty (got {} bytes)",
            body_bytes.len()
        );
    }

    // -----------------------------------------------------------------------
    // Range request sanity check
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn range_request_returns_partial_content_or_ok() {
        let (svc, _dir) = make_service(|p| {
            std::fs::write(p.join("data.bin"), b"0123456789").unwrap();
        });
        let resp = svc
            .oneshot(
                Request::builder()
                    .uri("/data.bin")
                    .header("range", "bytes=0-4")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // ServeDir returns 206 for a valid range or 200 for the full body.
        // Either is acceptable here — we just confirm the wrapper doesn't
        // block range requests.
        assert!(
            resp.status() == StatusCode::PARTIAL_CONTENT || resp.status() == StatusCode::OK,
            "range request must not be blocked by the containment wrapper (got {:?})",
            resp.status()
        );
    }

    // -----------------------------------------------------------------------
    // Symlink tests — require Unix (symlink creation needs no special
    // privileges on Linux/macOS, but is not always available on Windows)
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[tokio::test]
    async fn out_of_root_symlink_returns_404() {
        use std::os::unix::fs::symlink;

        let outside = tempfile::tempdir().expect("outside dir");
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();

        let (svc, _assets) = make_service(|assets| {
            // Symlink inside assets pointing to a file outside the dir.
            symlink(outside.path().join("secret.txt"), assets.join("evil.txt")).unwrap();
        });

        let status = status_for(svc, "/evil.txt").await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "out-of-root symlink must be blocked (got {:?})",
            status
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn in_root_symlink_is_served() {
        use std::os::unix::fs::symlink;

        let (svc, _dir) = make_service(|assets| {
            std::fs::write(assets.join("real.css"), b"body{}").unwrap();
            // Symlink inside assets pointing to another file inside assets.
            symlink(assets.join("real.css"), assets.join("alias.css")).unwrap();
        });

        let status = status_for(svc, "/alias.css").await;
        assert_eq!(
            status,
            StatusCode::OK,
            "in-root symlink must be served (got {:?})",
            status
        );
    }

    /// TOCTOU-narrowing regression: after the containment check passes,
    /// the request URI is rewritten to the canonical path before reaching
    /// ServeDir.  Requesting the symlink name must therefore serve the
    /// canonical target's exact bytes (ServeDir opens `real.css`, not
    /// the mutable `alias.css` link).
    #[cfg(unix)]
    #[tokio::test]
    async fn in_root_symlink_serves_canonical_target_bytes() {
        use std::os::unix::fs::symlink;

        let (svc, _dir) = make_service(|assets| {
            std::fs::write(assets.join("real.css"), b"body{color:red}").unwrap();
            symlink(assets.join("real.css"), assets.join("alias.css")).unwrap();
        });

        let resp = svc
            .oneshot(
                Request::builder()
                    .uri("/alias.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "in-root symlink must be served via the rewritten canonical URI"
        );
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        assert!(
            ct.contains("css"),
            "canonical target must keep its CSS content-type, got: {ct}"
        );
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(
            &body[..],
            b"body{color:red}",
            "rewritten-URI delegation must serve the canonical target's bytes"
        );
    }

    // -----------------------------------------------------------------------
    // Layered two-root service (issue #1189) — `zfb dev` serves `/assets/*`
    // from the isolated dev-assets root first, with `dist/assets/` as the
    // boot-lazy-seed fallback. A concurrent `zfb build` that wipes `dist/`
    // must not 404 the dev-served stylesheet.
    // -----------------------------------------------------------------------

    /// Build a layered service over two tempdirs (primary tried first), run
    /// `setup` on each, and return `(service, primary_dir, secondary_dir)`.
    /// The tempdirs are returned so the caller keeps them alive.
    fn make_layered(
        primary_setup: impl FnOnce(&Path),
        secondary_setup: impl FnOnce(&Path),
    ) -> (ContainedAssetsService, tempfile::TempDir, tempfile::TempDir) {
        let primary = tempfile::tempdir().expect("primary tempdir");
        let secondary = tempfile::tempdir().expect("secondary tempdir");
        primary_setup(primary.path());
        secondary_setup(secondary.path());
        let svc = ContainedAssetsService::layered(vec![
            primary.path().to_path_buf(),
            secondary.path().to_path_buf(),
        ]);
        (svc, primary, secondary)
    }

    async fn body_for(svc: ContainedAssetsService, path: &str) -> (StatusCode, Vec<u8>) {
        let resp = svc
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        (status, bytes.to_vec())
    }

    #[tokio::test]
    async fn layered_serves_from_primary_root() {
        let (svc, _p, _s) = make_layered(
            |p| std::fs::write(p.join("styles.css"), b"primary{}").unwrap(),
            |_s| {},
        );
        let (status, body) = body_for(svc, "/styles.css").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"primary{}", "primary root must serve its file");
    }

    #[tokio::test]
    async fn layered_falls_back_to_secondary_root() {
        // Mirrors a boot-lazy prebuilt seed: only the build `dist/assets/`
        // (secondary) holds the hashed asset; the isolated dev root doesn't.
        let (svc, _p, _s) = make_layered(
            |_p| {},
            |s| std::fs::write(s.join("styles-abc123.css"), b"seed{}").unwrap(),
        );
        let (status, body) = body_for(svc, "/styles-abc123.css").await;
        assert_eq!(status, StatusCode::OK, "must fall through to dist seed");
        assert_eq!(&body[..], b"seed{}");
    }

    #[tokio::test]
    async fn layered_primary_shadows_same_name_in_secondary() {
        let (svc, _p, _s) = make_layered(
            |p| std::fs::write(p.join("styles.css"), b"dev-stable{}").unwrap(),
            |s| std::fs::write(s.join("styles.css"), b"stale-build{}").unwrap(),
        );
        let (status, body) = body_for(svc, "/styles.css").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            &body[..],
            b"dev-stable{}",
            "primary (isolated dev-assets) must win over the secondary"
        );
    }

    /// THE regression for issue #1189: dev's stable `styles.css` lives in the
    /// isolated primary root; a concurrent `zfb build` wipes the secondary
    /// (`dist/`). The stylesheet must STILL be served (before the fix, dev
    /// wrote into `dist/assets/` and this returned 404 → unstyled site).
    #[tokio::test]
    async fn layered_primary_survives_secondary_wipe() {
        let (svc, _p, secondary) = make_layered(
            |p| std::fs::write(p.join("styles.css"), b"body{color:blue}").unwrap(),
            |s| {
                std::fs::write(s.join("styles-old.css"), b"seed{}").unwrap();
            },
        );

        // Sanity: served before the wipe.
        let (status, body) = body_for(svc.clone(), "/styles.css").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"body{color:blue}");

        // Simulate `zfb build` wiping the shared `dist/` (the secondary root).
        std::fs::remove_dir_all(secondary.path()).unwrap();

        // The dev-served stylesheet is untouched — still 200.
        let (status, body) = body_for(svc, "/styles.css").await;
        assert_eq!(
            status,
            StatusCode::OK,
            "isolated dev stylesheet must survive a dist/ wipe (#1189)"
        );
        assert_eq!(&body[..], b"body{color:blue}");
    }

    #[tokio::test]
    async fn layered_missing_in_both_roots_is_404() {
        let (svc, _p, _s) = make_layered(|_p| {}, |_s| {});
        let (status, _body) = body_for(svc, "/nope.css").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn layered_enforces_containment_on_both_roots() {
        // A traversal escape must be rejected regardless of which root it
        // would resolve under — each root is canonicalized independently.
        let (svc, _p, _s) = make_layered(
            |p| std::fs::write(p.join("ok.css"), b"x{}").unwrap(),
            |s| std::fs::write(s.join("ok.css"), b"y{}").unwrap(),
        );
        let (status, _body) = body_for(svc, "/../../etc/passwd").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "traversal must 404");
    }

    // -----------------------------------------------------------------------
    // candidate_path unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn candidate_path_normal() {
        let base = Path::new("/assets");
        let p = ContainedAssetsService::candidate_path(base, "/style.css").unwrap();
        assert_eq!(p, Path::new("/assets/style.css"));
    }

    #[test]
    fn candidate_path_strips_leading_slash() {
        let base = Path::new("/assets");
        let p = ContainedAssetsService::candidate_path(base, "/foo/bar.js").unwrap();
        assert_eq!(p, Path::new("/assets/foo/bar.js"));
    }

    #[test]
    fn candidate_path_percent_decoded() {
        let base = Path::new("/assets");
        let p = ContainedAssetsService::candidate_path(base, "/my%20file.css").unwrap();
        assert_eq!(p, Path::new("/assets/my file.css"));
    }

    #[test]
    fn candidate_path_dot_segment_skipped() {
        let base = Path::new("/assets");
        let p = ContainedAssetsService::candidate_path(base, "/./foo.js").unwrap();
        assert_eq!(p, Path::new("/assets/foo.js"));
    }

    #[test]
    fn candidate_path_parent_dir_rejected() {
        let base = Path::new("/assets");
        assert!(
            ContainedAssetsService::candidate_path(base, "/../etc/passwd").is_none(),
            "ParentDir component must be rejected"
        );
    }

    #[test]
    fn candidate_path_encoded_dotdot_blocked() {
        // %2e%2e decodes to '..' — after decoding it's a ParentDir component
        let base = Path::new("/assets");
        assert!(
            ContainedAssetsService::candidate_path(base, "/%2e%2e/secret").is_none(),
            "encoded '..' must be rejected after decoding"
        );
    }
}
