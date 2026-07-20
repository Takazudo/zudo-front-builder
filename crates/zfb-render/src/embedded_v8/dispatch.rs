//! Request / Response shapes that bridge between Rust callers and the
//! workerd-shape JS bundle.
//!
//! The bundle exports `default { fetch }` where `fetch(req): Response`.
//! `EmbeddedV8RenderHost::dispatch_fetch` constructs a JS `Request` from
//! [`HttpRequestLike`], invokes the bundle's `fetch`, awaits the
//! resulting `Response`, and packs it into [`HttpResponseLike`] so the
//! Rust caller (`zfb-build`'s renderer, sub-issue #164) can write the
//! body bytes to disk.
//!
//! Why hand-rolled instead of `http::Request<Vec<u8>>`: the trait /
//! crate is intentionally lean (the SSG path only needs URL + method +
//! headers + body in/out) and threading the `http` crate through the JS
//! boundary brings opinions we don't need here.

use std::collections::BTreeMap;

/// Inbound request shape for [`super::EmbeddedV8RenderHost::dispatch_fetch`].
///
/// The host ignores fields that are absent from a typical SSG request
/// (e.g. body) but takes them at the type level so the signature is
/// stable across future SSR-mode usage.
#[derive(Debug, Clone)]
pub struct HttpRequestLike {
    /// Absolute URL the JS-side `Request` is constructed with.
    /// The bundle's router (Hono / etc.) reads `request.url` and
    /// dispatches on the path; the host does NOT rewrite URLs.
    pub url: String,
    /// HTTP method. Defaults to `"GET"` when constructed via
    /// [`HttpRequestLike::get`]; SSG mode only ever drives GETs.
    pub method: String,
    /// Headers passed through to the `Request` constructor.
    /// Keys are lowercase per RFC 7230 §3.2 (Hono's matcher
    /// is case-insensitive but the JS-side `Headers` API normalises
    /// to lowercase).
    pub headers: BTreeMap<String, String>,
    /// Optional body bytes. `None` for SSG GETs.
    pub body: Option<Vec<u8>>,
}

impl HttpRequestLike {
    /// Build a GET request for `url` with no headers / body. The
    /// hot path on the SSG renderer.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: "GET".to_string(),
            headers: BTreeMap::new(),
            body: None,
        }
    }
}

/// Outbound response shape produced by
/// [`super::EmbeddedV8RenderHost::dispatch_fetch`].
///
/// Body is always materialised as bytes — the host calls
/// `response.arrayBuffer()` so the caller doesn't have to deal with
/// `ReadableStream`.
#[derive(Debug, Clone)]
pub struct HttpResponseLike {
    /// HTTP status code. `200` is the typical SSG case; the renderer
    /// surfaces non-200 to the caller (e.g. `404` for a missing page
    /// becomes a build error in `zfb-build`).
    pub status: u16,
    /// Headers as returned by the JS-side `Response.headers`, as an
    /// ordered list rather than a map: the JS side's `Headers` polyfill
    /// already applies the Fetch "sort and combine" view (duplicate
    /// `set-cookie` entries kept separate, everything else comma-joined),
    /// so collapsing this into a `BTreeMap<String, String>` here would
    /// silently drop repeated headers. Keys are lowercase. The renderer
    /// reads `content-type` (via [`Self::content_type`]) to decide the
    /// on-disk extension; everything else is opaque.
    pub headers: Vec<(String, String)>,
    /// Body bytes (after `arrayBuffer()` materialisation).
    pub body: Vec<u8>,
}

impl HttpResponseLike {
    /// Convenience: read the `content-type` header (lowercase key).
    /// Returns the first match if somehow duplicated (content-type is
    /// not a multi-valued header in practice).
    pub fn content_type(&self) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_str())
    }

    /// Convenience: decode the body as UTF-8. Returns `None` if the
    /// body is not valid UTF-8 (binary asset routes).
    pub fn body_utf8(&self) -> Option<&str> {
        std::str::from_utf8(&self.body).ok()
    }
}
