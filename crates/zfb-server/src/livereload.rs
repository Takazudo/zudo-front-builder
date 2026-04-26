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
//! - When only CSS changed we deliberately do **not** emit `Page`; the
//!   browser swaps the `<link>` href in place without reloading the
//!   document, so reloading would defeat the hot-swap.
//! - When both pages and CSS changed we emit both events. The browser
//!   handles `page` first (full reload) which renders the CSS swap moot
//!   for that tab, but other tabs subscribed to the same stream may
//!   still benefit from the CSS event.

use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use zfb_build::BuildOutcome;

/// A single live-reload event delivered over the SSE channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadEvent {
    /// One or more pages were re-rendered. The browser should fully
    /// reload the document (`location.reload()`).
    Page,
    /// CSS asset changed. The browser should hot-swap every
    /// `<link rel="stylesheet">` by bumping its query string.
    Css,
}

impl ReloadEvent {
    /// SSE `event:` field name. Matches the strings the browser script
    /// listens for at `/__zfb/livereload.js`.
    pub fn name(&self) -> &'static str {
        match self {
            ReloadEvent::Page => "page",
            ReloadEvent::Css => "css",
        }
    }
}

/// Translate a [`BuildOutcome`] into the live-reload events the browser
/// should observe.
///
/// See module docs for the rules.
pub fn outcome_to_events(outcome: &BuildOutcome) -> Vec<ReloadEvent> {
    let mut events = Vec::with_capacity(2);
    if !outcome.pages_written.is_empty() {
        events.push(ReloadEvent::Page);
    }
    if outcome.css_changed {
        events.push(ReloadEvent::Css);
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
            Ok(ev) => Some(Ok(Event::default().event(ev.name()))),
            Err(_lagged) => None,
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
    fn islands_changes_dont_emit() {
        // We don't currently model islands hot-swap. A pages_written
        // entry for the affected page is what triggers reload.
        let outcome = BuildOutcome {
            islands_rerun: true,
            islands_changed: true,
            ..Default::default()
        };
        assert!(outcome_to_events(&outcome).is_empty());
    }

    #[test]
    fn event_names_match_browser_protocol() {
        // The strings here MUST match the addEventListener calls in
        // src/livereload.js.
        assert_eq!(ReloadEvent::Page.name(), "page");
        assert_eq!(ReloadEvent::Css.name(), "css");
    }
}
