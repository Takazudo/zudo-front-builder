//! Embed-API request-extension middleware (sub-task 3.1a / #370).
//!
//! The public [`crate::Server`] builder exposes
//! `with_request_extension::<T>(value)` so an embedding host (e.g. a
//! Tauri app) can hand a per-process value (an `AppHandle`, an FS
//! capability set, an auth token, …) to its SSR handlers via the
//! standard `http::Extensions` mechanism. The retrieval pattern on the
//! handler side is:
//!
//! ```ignore
//! async fn my_handler(req: Request) -> impl IntoResponse {
//!     let ctx = req.extensions().get::<MyCtx>().cloned();
//!     // …
//! }
//! ```
//!
//! The handler does **not** import `axum::Extension<T>` — that type is
//! deliberately kept off the public surface of `zfb-server` (pin §3.3
//! of `research/346-embed-as-library-api.md`) so callers aren't forced
//! to adopt the axum extractor model.
//!
//! Implementation: each `.with_request_extension(value)` call captures
//! `value` (a `T: Clone + Send + Sync + 'static`) in a closure typed as
//! `Box<dyn Fn(&mut http::Extensions) + Send + Sync>`. The collection
//! of injectors is flattened into one `RequestExtensionLayer` that runs
//! per request, cloning each captured value into the request's
//! extensions before the inner service sees the request.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::Extensions;
use axum::middleware::{from_fn_with_state, Next};
use axum::response::Response;
use axum::Router;

/// Type-erased "insert a clone of the captured value into the
/// request's extensions map" closure. One closure per
/// `.with_request_extension(value)` call.
pub(crate) type RequestExtensionInjector =
    Arc<dyn Fn(&mut Extensions) + Send + Sync + 'static>;

/// Build a single [`RequestExtensionInjector`] that clones `value` into
/// each incoming request's extensions. `T: Clone + Send + Sync +
/// 'static` is the minimum bound that lets the embed builder accept
/// arbitrary host context types.
pub(crate) fn make_injector<T>(value: T) -> RequestExtensionInjector
where
    T: Clone + Send + Sync + 'static,
{
    let value = Arc::new(value);
    Arc::new(move |exts: &mut Extensions| {
        exts.insert(T::clone(&value));
    })
}

/// Apply a fan-out of request-extension injectors to the given router.
///
/// Called by [`crate::Server::serve`] / [`crate::Server::serve_in_thread`]
/// once the inner router has been built. When `injectors` is empty this
/// is a no-op and the router is returned unchanged so the existing
/// `zfb dev` / `zfb preview` paths pay nothing for the embed feature.
pub(crate) fn apply_request_extension_layer(
    router: Router,
    injectors: Vec<RequestExtensionInjector>,
) -> Router {
    if injectors.is_empty() {
        return router;
    }
    let state = Arc::new(injectors);
    router.layer(from_fn_with_state(state, inject_extensions_middleware))
}

/// The actual middleware closure passed to [`from_fn_with_state`]. Runs
/// every captured injector against the request's extensions map, then
/// hands the (possibly enriched) request to the inner service.
async fn inject_extensions_middleware(
    axum::extract::State(injectors): axum::extract::State<Arc<Vec<RequestExtensionInjector>>>,
    mut req: Request,
    next: Next,
) -> Response {
    let exts = req.extensions_mut();
    for injector in injectors.iter() {
        injector(exts);
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use tower::ServiceExt; // for `Router::oneshot`.

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct CtxA(&'static str);

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct CtxB(u32);

    /// Multiple `with_request_extension` calls with distinct types
    /// must all reach the handler.
    #[tokio::test]
    async fn injects_multiple_types_into_request_extensions() {
        let injectors = vec![
            make_injector(CtxA("hello")),
            make_injector(CtxB(42)),
        ];

        let router: Router = Router::new().route(
            "/",
            get(|req: HttpRequest<Body>| async move {
                let a = req.extensions().get::<CtxA>().cloned();
                let b = req.extensions().get::<CtxB>().cloned();
                format!("{a:?}|{b:?}")
            }),
        );
        let router = apply_request_extension_layer(router, injectors);

        let resp = router
            .oneshot(
                HttpRequest::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains(r#"CtxA("hello")"#), "got: {body}");
        assert!(body.contains("CtxB(42)"), "got: {body}");
    }

    /// With no injectors the router passes through unchanged.
    #[tokio::test]
    async fn no_injectors_is_noop() {
        let router: Router = Router::new().route(
            "/",
            get(|req: HttpRequest<Body>| async move {
                let has = req.extensions().get::<CtxA>().is_some();
                format!("has={has}")
            }),
        );
        let router = apply_request_extension_layer(router, Vec::new());

        let resp = router
            .oneshot(
                HttpRequest::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"has=false");
    }
}
