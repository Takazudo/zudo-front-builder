//! SSE live-reload bridge between [`zfb_build::BuildOutcome`] and the
//! browser.
//!
//! The dev server holds a [`tokio::sync::broadcast`] channel of
//! [`ReloadEvent`]s. The bin crate (the one that owns
//! [`zfb_build::BuildOrchestrator`]) wires its `on_outcome` callback
//! through [`outcome_to_events`] and forwards the resulting events into
//! the channel. Each browser tab subscribes to the channel via the SSE
//! endpoint mounted at `/__zfb/reload` (see [`crate::routes`]).
//!
//! ## Wiring contract (cheat sheet for the bin crate)
//!
//! ```ignore
//! use tokio::sync::broadcast;
//! use zfb_build::{BuildOrchestrator, BuildContext};
//! use zfb_server::livereload::{outcome_to_events, ReloadEvent};
//!
//! let (tx, _rx) = broadcast::channel::<ReloadEvent>(64);
//! let server_tx = tx.clone();
//!
//! tokio::spawn(async move {
//!     orchestrator
//!         .run(ctx, move |outcome| {
//!             for ev in outcome_to_events(outcome) {
//!                 // Errors here just mean "no live subscribers"; ignore.
//!                 let _ = tx.send(ev);
//!             }
//!         })
//!         .await
//! });
//!
//! zfb_server::serve(ServeOpts { /* ... */ broadcast: server_tx, /* ... */ }).await?;
//! ```
//!
//! ## Event mapping rules
//!
//! - `outcome.pages_written.len() > 0`  ⇒  emit one [`ReloadEvent::Page`].
//! - `outcome.css_changed`              ⇒  emit one [`ReloadEvent::Css`].
//! - `outcome.islands_bundle.is_some()` ⇒  emit one
//!   [`ReloadEvent::Islands`] per re-bundled component, carrying the
//!   bundle's public URL so the browser-side runtime can swap-import the
//!   new module without a full page reload.
//! - When only CSS or only islands changed we deliberately do **not**
//!   emit `Page`; the browser swaps in place without reloading the
//!   document so reloading would defeat the hot-swap.
//! - When both pages and CSS/islands changed we emit all events. The
//!   browser handles `page` first (full reload) which renders the swaps
//!   moot for that tab, but other tabs subscribed to the same stream
//!   still benefit from the in-place updates.

use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use zfb_build::BuildOutcome;

/// A single live-reload event delivered over the SSE channel.
///
/// The enum carries owned data because the `Islands` variant references
/// per-component identifiers and a bundle URL the browser must dispatch
/// on. `Copy` is therefore deliberately not implemented — clones are
/// cheap (one short string per field) and the broadcast channel is
/// sized small.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReloadEvent {
    /// One or more pages were re-rendered. The browser should fully
    /// reload the document (`location.reload()`).
    Page,
    /// CSS asset changed. The browser should hot-swap every
    /// `<link rel="stylesheet">` by bumping its query string.
    Css,
    /// The islands bundle changed. The browser should swap-import the
    /// new bundle URL and re-run hydration for the named component.
    Islands {
        /// Stable identifier of the `"use client"` component whose
        /// island bundle changed. Mirrors the
        /// `zfb_islands::Island::component_name` value.
        component: String,
        /// Public URL of the freshly-emitted bundle, e.g.
        /// `/assets/islands-abc12345.js`. The browser performs
        /// `import(bundle_url)` on receipt, so the URL must already
        /// reflect the new content hash.
        bundle_url: String,
    },
}

impl ReloadEvent {
    /// SSE `event:` field name. Matches the strings the browser script
    /// listens for at `/__zfb/livereload.js`.
    pub fn name(&self) -> &'static str {
        match self {
            ReloadEvent::Page => "page",
            ReloadEvent::Css => "css",
            ReloadEvent::Islands { .. } => "islands",
        }
    }

    /// SSE `data:` payload. `Page` and `Css` carry no data (the browser
    /// reacts on the event name alone); `Islands` serialises the
    /// `{component, bundle_url}` pair as a one-line JSON object.
    pub fn data(&self) -> String {
        match self {
            ReloadEvent::Page | ReloadEvent::Css => String::new(),
            ReloadEvent::Islands {
                component,
                bundle_url,
            } => serde_json::json!({
                "component": component,
                "bundleUrl": bundle_url,
            })
            .to_string(),
        }
    }
}

/// Per-bundle metadata carried out of [`outcome_to_events`].
///
/// The `BuildOutcome` only surfaces flags today — when an islands
/// re-bundle happens the bin crate populates this side-channel via
/// [`BuildOutcome::islands_bundle`] so the SSE layer knows which
/// components and which URL to broadcast.
pub use zfb_build::IslandsBundleInfo;

/// Translate a [`BuildOutcome`] into the live-reload events the browser
/// should observe.
///
/// See module docs for the rules.
pub fn outcome_to_events(outcome: &BuildOutcome) -> Vec<ReloadEvent> {
    let mut events = Vec::new();
    if !outcome.pages_written.is_empty() {
        events.push(ReloadEvent::Page);
    }
    if outcome.css_changed {
        events.push(ReloadEvent::Css);
    }
    if let Some(info) = outcome.islands_bundle.as_ref() {
        if info.changed {
            if info.components.is_empty() {
                // Islands bundle changed (e.g. runtime-only diff) but
                // we don't know which components — emit a single
                // generic event keyed on the empty string so the
                // browser still triggers a reload. Without this the
                // user would see a successful rebuild with no live
                // update on screen.
                events.push(ReloadEvent::Islands {
                    component: String::new(),
                    bundle_url: info.bundle_url.clone(),
                });
            } else {
                for component in &info.components {
                    events.push(ReloadEvent::Islands {
                        component: component.clone(),
                        bundle_url: info.bundle_url.clone(),
                    });
                }
            }
        }
    }
    events
}

/// Type alias for the broadcast sender used by the server. The bin
/// crate constructs one of these and:
///
/// 1. Hands a clone to [`crate::ServeOpts::broadcast`] so the SSE route
///    can subscribe per connection.
/// 2. Calls [`broadcast::Sender::send`] from the orchestrator's
///    `on_outcome` callback for each event yielded by
///    [`outcome_to_events`].
///
/// `send` returns `Err` only when there are zero live receivers, which
/// is the normal state at startup or when no browser tab is open.
/// Callers should ignore that error.
pub type ReloadTx = broadcast::Sender<ReloadEvent>;

/// Build the per-connection SSE stream wired to the broadcast channel.
///
/// `BroadcastStream` yields `Result<T, BroadcastStreamRecvError>`. The
/// `Lagged` error means this connection couldn't keep up with the
/// stream — we just skip the missed event (the next real event will
/// re-trigger a reload, which converges on the up-to-date state).
pub fn sse_response(
    tx: &ReloadTx,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + Send + 'static> {
    let rx = tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|res| async move {
        match res {
            Ok(ev) => {
                let mut sse = Event::default().event(ev.name());
                let payload = ev.data();
                if !payload.is_empty() {
                    sse = sse.data(payload);
                }
                Some(Ok(sse))
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(skipped)) => {
                // The connection couldn't keep up. Log a warning with
                // the skipped count so we don't silently drop reload
                // events under load. The next real event will still
                // trigger a reload, so the browser converges on the
                // latest state.
                tracing::warn!(skipped, "live-reload SSE stream lagged; dropping events");
                None
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zfb_build::BuildOutcome;
    use zfb_graph::PageId;

    fn pid(s: &str) -> PageId {
        PageId::new(s)
    }

    #[test]
    fn empty_outcome_emits_nothing() {
        let outcome = BuildOutcome::default();
        assert!(outcome_to_events(&outcome).is_empty());
    }

    #[test]
    fn pages_written_emits_page_only() {
        let outcome = BuildOutcome {
            pages_written: vec![pid("/a")],
            ..Default::default()
        };
        assert_eq!(outcome_to_events(&outcome), vec![ReloadEvent::Page]);
    }

    #[test]
    fn css_changed_emits_css_only() {
        let outcome = BuildOutcome {
            css_changed: true,
            css_rerun: true,
            ..Default::default()
        };
        assert_eq!(outcome_to_events(&outcome), vec![ReloadEvent::Css]);
    }

    #[test]
    fn pages_and_css_emits_both() {
        let outcome = BuildOutcome {
            pages_written: vec![pid("/a"), pid("/b")],
            css_changed: true,
            css_rerun: true,
            ..Default::default()
        };
        assert_eq!(
            outcome_to_events(&outcome),
            vec![ReloadEvent::Page, ReloadEvent::Css]
        );
    }

    #[test]
    fn css_rerun_without_change_emits_nothing() {
        // CSS pipeline ran but the asset was byte-identical: skip the
        // event so we don't trigger a needless hot-swap.
        let outcome = BuildOutcome {
            css_rerun: true,
            css_changed: false,
            ..Default::default()
        };
        assert!(outcome_to_events(&outcome).is_empty());
    }

    #[test]
    fn islands_rerun_without_bundle_info_emits_nothing() {
        // The flags say a rerun ran but the runner populated no
        // bundle info — nothing to broadcast (and no URL to dispatch
        // on anyway).
        let outcome = BuildOutcome {
            islands_rerun: true,
            islands_changed: true,
            islands_bundle: None,
            ..Default::default()
        };
        assert!(outcome_to_events(&outcome).is_empty());
    }

    #[test]
    fn islands_bundle_info_unchanged_emits_nothing() {
        // A rerun produced a byte-identical bundle: the URL didn't
        // move, so the browser has nothing to swap.
        let outcome = BuildOutcome {
            islands_rerun: true,
            islands_changed: false,
            islands_bundle: Some(IslandsBundleInfo {
                changed: false,
                bundle_url: "/assets/islands-deadbeef.js".to_string(),
                components: vec!["Counter".to_string()],
            }),
            ..Default::default()
        };
        assert!(outcome_to_events(&outcome).is_empty());
    }

    #[test]
    fn islands_bundle_info_changed_emits_one_event_per_component() {
        let outcome = BuildOutcome {
            islands_rerun: true,
            islands_changed: true,
            islands_bundle: Some(IslandsBundleInfo {
                changed: true,
                bundle_url: "/assets/islands-cafef00d.js".to_string(),
                components: vec!["Counter".to_string(), "Search".to_string()],
            }),
            ..Default::default()
        };
        let events = outcome_to_events(&outcome);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            ReloadEvent::Islands { component, bundle_url }
                if component == "Counter"
                    && bundle_url == "/assets/islands-cafef00d.js"
        ));
        assert!(matches!(
            &events[1],
            ReloadEvent::Islands { component, bundle_url }
                if component == "Search"
                    && bundle_url == "/assets/islands-cafef00d.js"
        ));
    }

    #[test]
    fn pages_and_islands_emits_both() {
        let outcome = BuildOutcome {
            pages_written: vec![pid("/a")],
            islands_rerun: true,
            islands_changed: true,
            islands_bundle: Some(IslandsBundleInfo {
                changed: true,
                bundle_url: "/assets/islands-feed1234.js".to_string(),
                components: vec!["Counter".to_string()],
            }),
            ..Default::default()
        };
        let events = outcome_to_events(&outcome);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], ReloadEvent::Page);
        assert!(matches!(events[1], ReloadEvent::Islands { .. }));
    }

    #[test]
    fn event_names_match_browser_protocol() {
        // The strings here MUST match the addEventListener calls in
        // src/livereload.js.
        assert_eq!(ReloadEvent::Page.name(), "page");
        assert_eq!(ReloadEvent::Css.name(), "css");
        assert_eq!(
            ReloadEvent::Islands {
                component: "Counter".into(),
                bundle_url: "/assets/x.js".into(),
            }
            .name(),
            "islands"
        );
    }

    #[test]
    fn islands_event_carries_json_payload() {
        let ev = ReloadEvent::Islands {
            component: "Counter".into(),
            bundle_url: "/assets/islands-deadbeef.js".into(),
        };
        let payload = ev.data();
        // JSON parses cleanly and carries both keys.
        let parsed: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["component"], "Counter");
        assert_eq!(parsed["bundleUrl"], "/assets/islands-deadbeef.js");
        // Page / Css events stay payload-less.
        assert!(ReloadEvent::Page.data().is_empty());
        assert!(ReloadEvent::Css.data().is_empty());
    }
}
