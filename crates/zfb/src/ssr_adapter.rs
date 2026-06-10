//! Adapter that fulfils `zfb_server::SsrDispatcher` by routing requests
//! into the long-lived embedded V8 host owned by the dev session
//! (Gap 1 of #367).
//!
//! ## Why the indirection
//!
//! `zfb-server` deliberately doesn't depend on `zfb-render` /
//! `zfb-build`. The dev router asks for a `dyn SsrDispatcher` and
//! the bin crate (here) supplies one that knows how to drive the V8
//! host. The wire shape is intentionally minimal — see
//! `crates/zfb-server/src/ssr.rs` for the full contract.
//!
//! ## Threading model
//!
//! The V8 isolate is pinned to its own OS thread by
//! [`ThreadedV8Host`](crate::v8_host_adapter::ThreadedV8Host) — see
//! that module for the dedicated-thread + mpsc-channel pattern. This
//! adapter doesn't spawn a new thread; it owns a clone of the
//! `Arc<Mutex<Option<RendererState>>>` already in
//! [`DevRenderSession`](crate::commands::dev::DevRenderSession), and
//! goes through it via [`tokio::task::spawn_blocking`] so the axum
//! worker isn't parked on a sync `mpsc::recv()`.
//!
//! ## Concurrency
//!
//! `EmbeddedV8Host::dispatch_fetch` takes `&mut self` — concurrent
//! SSR requests serialise on the renderer mutex. That's an accepted
//! v1 trade-off because the host is single-threaded anyway (one
//! request resolves at a time on the dedicated V8 thread, even if we
//! didn't hold the mutex). Multiple parallel SSR requests would
//! require a host pool, not finer-grained locking.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use zfb_build::renderer::{HttpResponseLike, RendererState};
use zfb_server::{SsrDispatchError, SsrDispatcher, SsrRequest, SsrResponse};

/// Shared, thread-safe handle to the renderer state. Cloned by the
/// dev session's `DevRenderInner` and by this adapter so both layers
/// see the same V8 host without ownership tugs-of-war.
pub type SharedRendererState = Arc<Mutex<Option<RendererState>>>;

/// Adapter that fulfils [`SsrDispatcher`] by dispatching through the
/// renderer's embedded V8 host.
pub struct EmbeddedV8SsrAdapter {
    state: SharedRendererState,
}

impl EmbeddedV8SsrAdapter {
    pub fn new(state: SharedRendererState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl SsrDispatcher for EmbeddedV8SsrAdapter {
    async fn dispatch(&self, request: SsrRequest) -> Result<SsrResponse, SsrDispatchError> {
        let state = Arc::clone(&self.state);
        let url_for_err = request.url_path.clone();
        // `dispatch_fetch_full` is sync (it blocks on `mpsc::recv`
        // waiting for the V8 thread's reply); hop to a blocking task
        // so the axum worker thread stays unwedged.
        //
        // The full-fidelity dispatch path (added for issue #367) takes
        // method, headers, and body alongside the URL path so a
        // `prerender = false` page can implement non-GET endpoints
        // exactly the way it would in Cloudflare. The default trait
        // impl on `EmbeddedV8Host` falls back to `dispatch_fetch` for
        // hosts that don't know how to forward the full request shape
        // (test stubs); `ThreadedV8Host` overrides the default and
        // threads everything through.
        let dispatch_result: std::result::Result<
            HttpResponseLike,
            zfb_build::renderer::RendererError,
        > = tokio::task::spawn_blocking(move || {
            let mut guard = match state.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let st = guard.as_mut().ok_or_else(|| {
                zfb_build::renderer::RendererError::EmbeddedV8(
                    "renderer state has been shut down".into(),
                )
            })?;
            let host = st.embedded_v8_host_mut().ok_or_else(|| {
                zfb_build::renderer::RendererError::EmbeddedV8(
                    "renderer is not backed by an embedded V8 host".into(),
                )
            })?;
            host.dispatch_fetch_full(
                &request.url_path,
                &request.method,
                &request.headers,
                &request.body,
            )
        })
        .await
        .map_err(|join_err| SsrDispatchError {
            url_path: url_for_err.clone(),
            message: format!("spawn_blocking join error: {join_err}"),
        })?;

        let resp = dispatch_result.map_err(|e| SsrDispatchError {
            url_path: url_for_err.clone(),
            message: e.to_string(),
        })?;

        Ok(http_response_to_ssr(resp))
    }
}

/// Convert a build-side [`HttpResponseLike`] into a server-side
/// [`SsrResponse`], preserving every header the V8 bundle set.
///
/// Deep-review fix (PR #376): earlier this seam forwarded only
/// `content-type` and silently dropped Cache-Control, Set-Cookie,
/// Location, CORS, and X-* headers — breaking dev/prod parity for
/// `prerender = false` pages. Now ALL headers flow through; the
/// `content_type` field on `HttpResponseLike` is preferred when the
/// header map didn't already carry one (the renderer duplicates
/// content-type out to a typed field for its own hot path).
///
/// Multi-valued headers (notably `Set-Cookie`) still collapse to the
/// last value via `BTreeMap<String, String>` — a preexisting design
/// limit of the seam documented on `HttpResponseLike`. Fixing that
/// needs a wider switch to `Vec<(String, String)>` or `http::HeaderMap`
/// and is intentionally deferred.
fn http_response_to_ssr(resp: HttpResponseLike) -> SsrResponse {
    let mut headers = resp.headers;
    if !resp.content_type.is_empty() {
        headers
            .entry("content-type".into())
            .or_insert(resp.content_type);
    }
    SsrResponse {
        status: resp.status,
        headers,
        body: resp.body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Deep-review regression (PR #376): every header the V8 bundle
    /// sets must reach the SsrResponse — earlier only content-type
    /// survived. Pin Cache-Control + a custom X-Trace-Id to mirror the
    /// classes of header Cloudflare's adapter forwards.
    #[test]
    fn http_response_to_ssr_forwards_all_headers() {
        let mut headers = BTreeMap::new();
        headers.insert("cache-control".into(), "no-store".into());
        headers.insert("x-trace-id".into(), "abc-123".into());
        headers.insert("location".into(), "/elsewhere".into());
        let resp = HttpResponseLike {
            status: 302,
            content_type: "text/html; charset=utf-8".into(),
            headers,
            body: b"redirected".to_vec(),
        };
        let ssr = http_response_to_ssr(resp);
        assert_eq!(ssr.status, 302);
        assert_eq!(ssr.body, b"redirected");
        assert_eq!(
            ssr.headers.get("cache-control").map(String::as_str),
            Some("no-store"),
        );
        assert_eq!(
            ssr.headers.get("x-trace-id").map(String::as_str),
            Some("abc-123"),
        );
        assert_eq!(
            ssr.headers.get("location").map(String::as_str),
            Some("/elsewhere"),
        );
        assert_eq!(
            ssr.headers.get("content-type").map(String::as_str),
            Some("text/html; charset=utf-8"),
        );
    }

    /// When the rich `headers` map already carries content-type, it
    /// wins over the duplicated `content_type` field — the JS bundle's
    /// own Header is the source of truth.
    #[test]
    fn http_response_to_ssr_prefers_header_map_content_type() {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".into(), "application/json".into());
        let resp = HttpResponseLike {
            status: 200,
            content_type: "text/html".into(),
            headers,
            body: b"{}".to_vec(),
        };
        let ssr = http_response_to_ssr(resp);
        assert_eq!(
            ssr.headers.get("content-type").map(String::as_str),
            Some("application/json"),
        );
    }

    /// An empty `content_type` field with no header-map entry yields
    /// no content-type header (defensive parity with the old behaviour).
    #[test]
    fn http_response_to_ssr_omits_content_type_when_empty() {
        let resp = HttpResponseLike {
            status: 204,
            content_type: String::new(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        };
        let ssr = http_response_to_ssr(resp);
        assert!(ssr.headers.is_empty());
    }
}
