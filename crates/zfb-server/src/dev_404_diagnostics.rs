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
    /// servable. `refresh_in_progress` is `true` only when the provider
    /// observed a route-table refresh actually running at diagnosis time.
    RouteKnownNotRendered { refresh_in_progress: bool },
    /// The provider cannot answer authoritatively (no route table has been
    /// published yet, or no renderer session exists).
    Unavailable,
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
                refresh_in_progress: false,
            } => {
                "<p data-zfb-dev-404-reason=\"route-known-not-rendered\">This URL is in the dev server's current route table, but no rendered output was available for it. Check the dev server output for a render error.</p>"
            }
            DevRouteDiagnosis::RouteKnownNotRendered {
                refresh_in_progress: true,
            } => {
                "<p data-zfb-dev-404-reason=\"route-known-not-rendered\">This URL is in the dev server's current route table, but no rendered output was available for it. A route refresh is in progress; this tab reloads when it publishes.</p>"
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
                refresh_in_progress: true
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
    fn refresh_sentence_requires_observed_refresh() {
        let idle = dev_404_body_with_diagnosis(
            DEV_404_BODY,
            DevRouteDiagnosis::RouteKnownNotRendered {
                refresh_in_progress: false,
            },
        );
        assert!(!idle.contains("refresh is in progress"), "{idle}");
        let busy = dev_404_body_with_diagnosis(
            DEV_404_BODY,
            DevRouteDiagnosis::RouteKnownNotRendered {
                refresh_in_progress: true,
            },
        );
        assert!(busy.contains("refresh is in progress"), "{busy}");
    }
}
