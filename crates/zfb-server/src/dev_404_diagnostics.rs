//! Read-only route diagnosis for dev 404 responses (issue #4783).
//!
//! `zfb dev` used to serve one identical 404 body for two different
//! situations: a URL that is no longer in the live SSG route table
//! (removed, renamed, or no longer yielded by `paths()`), and a URL the
//! route table still knows but that has no rendered output to serve. The
//! [`DevRouteDiagnostics`] seam lets the bin crate answer that question
//! from its authoritative route universe, so the waterfall's final 404
//! can say which case applies through the
//! [`DEV_404_REASON_HEADER`] header and an extra body paragraph.
//!
//! The seam is diagnostic only. It never renders, claims, or changes
//! status codes, serving order, routing, SSR, plugin/custom handlers,
//! Preview, or Embed: the server consults it only after every waterfall
//! leg has missed, only in [`crate::ServerMode::Dev`], and only when a
//! provider was configured through [`crate::ServeOpts`].

use std::sync::Arc;

use axum::http::HeaderName;

/// Response header carrying the dev 404 reason.
pub const DEV_404_REASON_HEADER: HeaderName = HeaderName::from_static("x-zfb-dev-404-reason");

/// Shared handle threaded through [`crate::ServeOpts`] and
/// [`crate::routes::AppState`].
pub type DevRouteDiagnosticsHandle = Arc<dyn DevRouteDiagnostics>;

/// Why the dev waterfall found nothing to serve for a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevRouteDiagnosis {
    /// The URL is absent from the current, published SSG route table.
    NotInRouteTable,
    /// The URL is in the current route table but no rendered output was
    /// servable. `activity` says what the provider observed at diagnosis
    /// time, so the body only blames a render error when nothing that
    /// could still produce the output was running.
    RouteKnownNotRendered { activity: RenderActivity },
    /// The provider cannot answer authoritatively (no route table has been
    /// published yet, or no renderer session exists).
    Unavailable,
}

/// What the provider observed for a known route without output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderActivity {
    /// The boot render finished and no route refresh is running.
    Idle,
    /// A route-table refresh is running right now.
    RefreshInProgress,
    /// The boot render has not finished yet: the route may simply not
    /// have been rendered so far.
    BootRenderPending,
}

impl DevRouteDiagnosis {
    /// The [`DEV_404_REASON_HEADER`] value for this diagnosis.
    pub fn reason(self) -> &'static str {
        match self {
            DevRouteDiagnosis::NotInRouteTable => "not-in-route-table",
            DevRouteDiagnosis::RouteKnownNotRendered { .. } => "route-known-not-rendered",
            DevRouteDiagnosis::Unavailable => "unavailable",
        }
    }

    fn body_paragraph(self) -> &'static str {
        match self {
            DevRouteDiagnosis::NotInRouteTable => {
                "<p data-zfb-dev-404-reason=\"not-in-route-table\">This URL is not in the dev server's current route table: the page may have been removed or renamed, or its <code>paths()</code> no longer yields it.</p>"
            }
            DevRouteDiagnosis::RouteKnownNotRendered {
                activity: RenderActivity::Idle,
            } => {
                "<p data-zfb-dev-404-reason=\"route-known-not-rendered\">This URL is in the dev server's current route table, but no rendered output was available for it. Check the dev server output for a render error.</p>"
            }
            DevRouteDiagnosis::RouteKnownNotRendered {
                activity: RenderActivity::RefreshInProgress,
            } => {
                "<p data-zfb-dev-404-reason=\"route-known-not-rendered\">This URL is in the dev server's current route table, but no rendered output was available for it. A route refresh is in progress; this tab reloads when it publishes.</p>"
            }
            DevRouteDiagnosis::RouteKnownNotRendered {
                activity: RenderActivity::BootRenderPending,
            } => {
                "<p data-zfb-dev-404-reason=\"route-known-not-rendered\">This URL is in the dev server's current route table and has not been rendered yet: the dev server is still finishing its first render. This tab reloads when it publishes.</p>"
            }
            DevRouteDiagnosis::Unavailable => {
                "<p data-zfb-dev-404-reason=\"unavailable\">The dev server cannot yet tell whether this URL is in its route table.</p>"
            }
        }
    }
}

/// Provider implemented by the bin crate over its live route universe.
///
/// `diagnose` receives the base-prefix-stripped, percent-encoded canonical
/// URL path with a leading slash (`/blog/caf%C3%A9`). It must be cheap and
/// must not block on rendering: hold any lock only for the lookup itself.
pub trait DevRouteDiagnostics: Send + Sync {
    fn diagnose(&self, url_path: &str) -> DevRouteDiagnosis;
}

/// Splice the diagnosis paragraph into a dev 404 body just before
/// `</body>`, leaving the established text untouched.
pub(crate) fn dev_404_body_with_diagnosis(base: &str, diagnosis: DevRouteDiagnosis) -> String {
    let paragraph = diagnosis.body_paragraph();
    match base.rfind("</body>") {
        Some(at) => {
            let mut body = String::with_capacity(base.len() + paragraph.len());
            body.push_str(&base[..at]);
            body.push_str(paragraph);
            body.push_str(&base[at..]);
            body
        }
        None => format!("{base}{paragraph}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::DEV_404_BODY;

    #[test]
    fn reasons_are_stable_header_values() {
        assert_eq!(
            DevRouteDiagnosis::NotInRouteTable.reason(),
            "not-in-route-table"
        );
        assert_eq!(
            DevRouteDiagnosis::RouteKnownNotRendered {
                activity: RenderActivity::BootRenderPending
            }
            .reason(),
            "route-known-not-rendered"
        );
        assert_eq!(DevRouteDiagnosis::Unavailable.reason(), "unavailable");
    }

    #[test]
    fn diagnosis_body_keeps_compatibility_text() {
        let body = dev_404_body_with_diagnosis(DEV_404_BODY, DevRouteDiagnosis::NotInRouteTable);
        assert!(body.contains("404 — page not in cache"), "{body}");
        assert!(
            body.contains("The dev server has no rendered HTML for this URL."),
            "{body}"
        );
        assert!(body.contains("not in the dev server's current route table"));
        assert!(body.ends_with("</p></body></html>"), "{body}");
    }

    #[test]
    fn render_error_hint_requires_idle_activity() {
        let body = |activity| {
            dev_404_body_with_diagnosis(
                DEV_404_BODY,
                DevRouteDiagnosis::RouteKnownNotRendered { activity },
            )
        };
        let idle = body(RenderActivity::Idle);
        assert!(idle.contains("render error"), "{idle}");
        assert!(!idle.contains("refresh is in progress"), "{idle}");
        let busy = body(RenderActivity::RefreshInProgress);
        assert!(busy.contains("refresh is in progress"), "{busy}");
        assert!(!busy.contains("render error"), "{busy}");
        let booting = body(RenderActivity::BootRenderPending);
        assert!(booting.contains("first render"), "{booting}");
        assert!(!booting.contains("render error"), "{booting}");
    }
}
