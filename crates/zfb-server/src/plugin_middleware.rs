//! Plugin dev-middleware integration (Sub 3 / #108).
//!
//! `zfb-server` itself does not depend on `zfb-build::PluginHost` — it
//! accepts a [`DevMiddlewareDispatcher`] trait object instead. The
//! dev command (`crates/zfb/src/commands/dev.rs`) plumbs the host
//! through a small adapter and the server only sees:
//!
//! - the list of `(path-prefix, handler-id)` registrations declared by
//!   user plugins, and
//! - a [`DevMiddlewareDispatcher::dispatch`] callback that takes a
//!   `(handler_id, request)` and returns the plugin's response.
//!
//! The dev router matches each incoming request's path against the
//! longest-prefix registration. A handler that returns
//! [`PluginPassthrough`] signals "I did not handle this request" — the
//! server then falls through to its built-in routes (the page cache,
//! `/__zfb/livereload.js`, etc.) so dev-middleware can layer cleanly
//! over the dev surface without owning the whole URL space.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

/// One registered dev-middleware handler.
#[derive(Debug, Clone)]
pub struct PluginRegistration {
    /// URL path prefix this handler claims. Matched as a longest
    /// prefix; a registration on `/doc-history` matches
    /// `/doc-history`, `/doc-history/`, and `/doc-history/foo`.
    pub path: String,
    /// Opaque token the dispatcher uses to identify the handler in the
    /// underlying plugin host.
    pub handler_id: String,
    /// Plugin display name. Surfaced in error responses so a 5xx from
    /// a plugin handler points at the offending plugin.
    pub plugin: String,
}

/// One HTTP request shipped from the dev server to a plugin handler.
#[derive(Debug, Clone)]
pub struct PluginRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
}

/// One HTTP response returned from a plugin handler.
///
/// `headers` is an ordered `Vec<(name, value)>` multimap (not a map) so
/// multi-valued response headers — most notably multiple `Set-Cookie` —
/// survive to the wire: the dev router `append`s each entry onto the
/// `http::HeaderMap` rather than `insert`ing (which collapses duplicates
/// to last-value-wins). The request side ([`PluginRequest::headers`])
/// stays a `HashMap` — request headers don't carry the duplicate-value
/// concern here.
#[derive(Debug, Clone)]
pub struct PluginResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Response body. UTF-8 text by default; if `body_encoding` is
    /// `"base64"` the bytes are decoded before being written.
    pub body: String,
    pub body_encoding: PluginResponseEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginResponseEncoding {
    Utf8,
    Base64,
}

impl PluginResponseEncoding {
    pub fn parse(s: &str) -> Self {
        match s {
            "base64" => Self::Base64,
            _ => Self::Utf8,
        }
    }
}

/// Outcome of dispatching one request to a plugin handler.
#[derive(Debug, Clone)]
pub enum PluginDispatchOutcome {
    /// Handler returned a real response — the server should send it.
    Response(PluginResponse),
    /// Handler returned `undefined` (passthrough) — the server should
    /// fall through to its built-in routes.
    Passthrough,
}

/// Shape implemented by adapters that own the underlying plugin host
/// (typically the long-lived `PluginHost` from `zfb-build`).
#[async_trait]
pub trait DevMiddlewareDispatcher: Send + Sync {
    async fn dispatch(
        &self,
        handler_id: &str,
        request: PluginRequest,
    ) -> Result<PluginDispatchOutcome, PluginDispatchError>;
}

/// Error returned by [`DevMiddlewareDispatcher::dispatch`] when the
/// underlying plugin host fails (e.g. the plugin handler threw, or the
/// host process died).
#[derive(Debug, thiserror::Error)]
#[error("plugin `{plugin}` dev-middleware failed: {message}")]
pub struct PluginDispatchError {
    pub plugin: String,
    pub message: String,
}

/// Bundle of registrations + dispatcher passed into the dev server.
/// Cloned cheaply (the heavy state lives behind `Arc`s); each route
/// handler keeps a reference for the lifetime of its task.
#[derive(Clone)]
pub struct DevMiddlewareSet {
    pub registrations: Arc<Vec<PluginRegistration>>,
    pub dispatcher: Arc<dyn DevMiddlewareDispatcher>,
}

impl DevMiddlewareSet {
    pub fn empty(dispatcher: Arc<dyn DevMiddlewareDispatcher>) -> Self {
        Self {
            registrations: Arc::new(Vec::new()),
            dispatcher,
        }
    }

    /// Find the handler whose registered path is the longest prefix of
    /// `url_path`. Returns `None` when no plugin claims this URL.
    pub fn find_match(&self, url_path: &str) -> Option<&PluginRegistration> {
        // Longest-prefix wins so a plugin can claim `/api/foo` and a
        // separate plugin can claim `/api`. Iterate the registration
        // list and pick the longest match — registration count per
        // project is small, no need for a trie here.
        let mut best: Option<&PluginRegistration> = None;
        for reg in self.registrations.iter() {
            if path_matches_prefix(url_path, &reg.path)
                && best.map(|b| b.path.len() < reg.path.len()).unwrap_or(true)
            {
                best = Some(reg);
            }
        }
        best
    }
}

/// True iff `url_path` is exactly `prefix` or starts with `prefix/`.
/// Avoids the classic `"foox".starts_with("foo")` false-positive.
///
/// Trailing slashes are normalised first so a registration at `/foo/`
/// matches a request to `/foo`, `/foo/`, or `/foo/bar` — the operator
/// shouldn't have to think about whether a plugin spelled the prefix
/// with or without a trailing slash. Matching the empty prefix `/` is
/// allowed and behaves like a catch-all.
pub fn path_matches_prefix(url_path: &str, prefix: &str) -> bool {
    let url = strip_trailing_slash(url_path);
    let pref = strip_trailing_slash(prefix);
    // Empty `pref` after trimming = a literal `/` registration; treat
    // as a catch-all that matches every URL.
    if pref.is_empty() {
        return true;
    }
    if url == pref {
        return true;
    }
    if let Some(rest) = url.strip_prefix(pref) {
        return rest.starts_with('/');
    }
    false
}

fn strip_trailing_slash(s: &str) -> &str {
    // Keep the leading `/` so `"/"` collapses to `""` (the catch-all
    // signal in `path_matches_prefix`). Stripping a trailing slash
    // off `"/foo/"` yields `"/foo"` which matches `"/foo"` exactly.
    if s.len() > 1 && s.ends_with('/') {
        &s[..s.len() - 1]
    } else if s == "/" {
        ""
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_prefix_wins() {
        struct Noop;
        #[async_trait]
        impl DevMiddlewareDispatcher for Noop {
            async fn dispatch(
                &self,
                _id: &str,
                _r: PluginRequest,
            ) -> Result<PluginDispatchOutcome, PluginDispatchError> {
                Ok(PluginDispatchOutcome::Passthrough)
            }
        }
        let set = DevMiddlewareSet {
            registrations: Arc::new(vec![
                PluginRegistration {
                    path: "/api".into(),
                    handler_id: "h1".into(),
                    plugin: "a".into(),
                },
                PluginRegistration {
                    path: "/api/foo".into(),
                    handler_id: "h2".into(),
                    plugin: "b".into(),
                },
            ]),
            dispatcher: Arc::new(Noop),
        };
        assert_eq!(set.find_match("/api/foo").unwrap().handler_id, "h2");
        assert_eq!(set.find_match("/api/bar").unwrap().handler_id, "h1");
        assert!(set.find_match("/other").is_none());
    }

    #[test]
    fn prefix_does_not_overmatch() {
        // `/foox` must not match a `/foo` registration.
        assert!(!path_matches_prefix("/foox", "/foo"));
        assert!(path_matches_prefix("/foo", "/foo"));
        assert!(path_matches_prefix("/foo/bar", "/foo"));
    }

    #[test]
    fn trailing_slashes_are_normalised_in_both_directions() {
        // Registration with trailing slash matches request without it.
        assert!(path_matches_prefix("/foo", "/foo/"));
        // Request with trailing slash matches registration without it.
        assert!(path_matches_prefix("/foo/", "/foo"));
        // Both with trailing slash.
        assert!(path_matches_prefix("/foo/", "/foo/"));
        // Sub-paths still match.
        assert!(path_matches_prefix("/foo/bar", "/foo/"));
    }

    #[test]
    fn root_prefix_is_a_catchall() {
        // A plugin registering `/` claims every URL — used as a
        // fallback / "catch-everything" surface.
        assert!(path_matches_prefix("/", "/"));
        assert!(path_matches_prefix("/anything", "/"));
        assert!(path_matches_prefix("/anything/nested", "/"));
    }
}
