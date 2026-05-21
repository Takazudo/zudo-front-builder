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
        // `dispatch_fetch` is sync (it blocks on `mpsc::recv` waiting
        // for the V8 thread's reply); hop to a blocking task so the
        // axum worker thread stays unwedged.
        //
        // The embedded V8 host's `dispatch_fetch` signature today is
        // `(url_path: &str) -> HttpResponseLike` — it does NOT take
        // method/headers/body. The path-and-query carries the URL
        // shape the bundle's fetch handler needs (the bundle wraps
        // the path in `http://localhost<path>` before constructing
        // a `Request`). Threading the method/headers/body through
        // requires extending the trait; we leave that for the
        // follow-up that wires non-GET prerender=false routes (the
        // current public surface in `crates/zfb-render/` only
        // supports GET-shape dispatches from the SSG path).
        let dispatch_result: std::result::Result<
            HttpResponseLike,
            zfb_build::renderer::RendererError,
        > = tokio::task::spawn_blocking(move || {
            let mut guard = match state.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let st = guard
                .as_mut()
                .ok_or_else(|| {
                    zfb_build::renderer::RendererError::EmbeddedV8(
                        "renderer state has been shut down".into(),
                    )
                })?;
            let host = st
                .embedded_v8_host_mut()
                .ok_or_else(|| {
                    zfb_build::renderer::RendererError::EmbeddedV8(
                        "renderer is not backed by an embedded V8 host".into(),
                    )
                })?;
            host.dispatch_fetch(&request.url_path)
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

        let mut headers = std::collections::BTreeMap::new();
        if !resp.content_type.is_empty() {
            headers.insert("content-type".into(), resp.content_type.clone());
        }
        Ok(SsrResponse {
            status: resp.status,
            headers,
            body: resp.body,
        })
    }
}
