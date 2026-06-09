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

use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use percent_encoding::percent_decode;
use tower_http::services::ServeDir;
use tower_service::Service;

/// The concrete request body type the service is parametrised over.
/// Our wrapper is mounted via `nest_service` which uses `axum::body::Body`.
type ReqBody = Body;

/// The concrete inner Service type (ServeDir with default fallback, Body request).
type InnerService = ServeDir;

/// A thin Tower [`Service`] that enforces symlink-containment on the
/// `dist/assets/` directory before delegating to the inner [`ServeDir`].
///
/// Constructed via [`ContainedAssetsService::new`].
#[derive(Clone)]
pub(crate) struct ContainedAssetsService {
    /// Canonical form of `<dist_root>/assets/`.  Pre-computed at
    /// construction time so the per-request hot path only calls
    /// `tokio::fs::canonicalize` once (for the candidate path).
    ///
    /// `None` means the assets directory didn't exist at boot (first dev
    /// boot before a build finishes).  Each request retries the
    /// canonicalize so assets start serving as soon as the dir appears.
    ///
    /// Wrapped in `Arc` so cloning (required by Tower) is cheap.
    canonical_root: Arc<Option<PathBuf>>,
    /// The raw (non-canonical) assets directory.
    assets_dir: PathBuf,
    /// Inner service that does the actual file serving.
    inner: InnerService,
}

impl ContainedAssetsService {
    /// Build a new [`ContainedAssetsService`] that serves files from
    /// `assets_dir`, rejecting any request whose resolved FS path escapes
    /// that directory (even via symlinks).
    ///
    /// `assets_dir` need not exist at construction time — a missing root
    /// simply means every request will get a 404 (same behaviour as an
    /// empty directory from the client's perspective).
    pub(crate) fn new(assets_dir: PathBuf) -> Self {
        // Best-effort pre-canonicalize at boot (sync, outside the hot loop).
        // If the dir doesn't exist yet, `canonical_root` is None and the
        // per-request path retries canonicalize so it becomes available
        // as soon as the first build completes.
        let canonical_root = std::fs::canonicalize(&assets_dir).ok();
        Self {
            canonical_root: Arc::new(canonical_root),
            inner: ServeDir::new(&assets_dir),
            assets_dir,
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
        <InnerService as Service<Request<ReqBody>>>::poll_ready(&mut self.inner, cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let uri_path = req.uri().path().to_owned();
        let assets_dir = self.assets_dir.clone();
        let canonical_root = Arc::clone(&self.canonical_root);

        // Clone the inner service so we can move it into the async block.
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // Build the candidate FS path using the same logic as ServeDir.
            let candidate = match Self::candidate_path(&assets_dir, &uri_path) {
                Some(p) => p,
                None => return Ok(not_found()),
            };

            // Canonicalize resolves symlinks; if it fails the file/target
            // doesn't exist — 404.
            let canonical_candidate = match tokio::fs::canonicalize(&candidate).await {
                Ok(p) => p,
                Err(_) => return Ok(not_found()),
            };

            // Determine the canonical root.  Prefer the pre-computed value;
            // fall back to a fresh canonicalize if it was None at boot.
            let canon_root = match canonical_root.as_ref() {
                Some(r) => r.clone(),
                None => match tokio::fs::canonicalize(&assets_dir).await {
                    Ok(p) => p,
                    Err(_) => return Ok(not_found()),
                },
            };

            // Containment check: the canonical candidate must be inside
            // (or equal to) the canonical assets root.
            if !canonical_candidate.starts_with(&canon_root) {
                return Ok(not_found());
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
    fn make_service(
        setup: impl FnOnce(&Path),
    ) -> (ContainedAssetsService, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        setup(dir.path());
        let svc = ContainedAssetsService::new(dir.path().to_path_buf());
        (svc, dir)
    }

    async fn status_for(svc: ContainedAssetsService, path: &str) -> StatusCode {
        let resp = svc
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
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
            resp.status() == StatusCode::PARTIAL_CONTENT
                || resp.status() == StatusCode::OK,
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
            symlink(
                outside.path().join("secret.txt"),
                assets.join("evil.txt"),
            )
            .unwrap();
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
