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
//! - `outcome.pages_written.len() > 0` **or** `outcome.pages_stale.len() > 0`
//!   **or** `outcome.pages_pruned.len() > 0` **or**
//!   `outcome.client_scripts_changed` **or**
//!   `outcome.ssr_routes_published`
//!   ⇒  emit one [`ReloadEvent::Page`] (deduplicated — at most one per tick).
//!   `pages_stale` (issue #1027) covers the lazy dev-render default: a tick
//!   that rendered nothing eagerly but marked routes stale must still tell
//!   the browser to reload — the reload's GET is what triggers the
//!   request-time re-render. `pages_pruned` (issue #1391) covers a
//!   prune-only tick (a route's HTML was deleted, e.g. a coalesced macOS
//!   FSEvents directory delete, with no page written or marked stale) — a
//!   tab still sitting on that now-deleted route must reload so it hits
//!   the dev server's 404 path instead of showing stale content forever.
//!   `ssr_routes_published` (issue #1826) covers the SSR (`prerender =
//!   false`) case the other four structurally cannot: an SSR route has
//!   no `dist/` output path, so it never appears in any of those path
//!   lists. It is the ONE shared bit both of `zfb dev`'s deferred-window
//!   self-heal channels set — the healthy deferred boot publish and the
//!   cold-bootstrap recovery seam — so the two paths heal an open tab
//!   identically instead of one healing better than the other.
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
///
/// Wire contract: `component: ""` means "the bundle changed but we don't
/// know which components — reload the whole bundle by re-importing
/// `bundle_url`". The JS consumer must NOT short-circuit on empty
/// `component`; it reads only `bundleUrl` to construct the swap URL.
pub fn outcome_to_events(outcome: &BuildOutcome) -> Vec<ReloadEvent> {
    let mut events = Vec::new();
    // Emit a single Page event if pages were written, OR routes were
    // marked stale (issue #1027 — the lazy dev-render default: nothing
    // rendered eagerly this tick, but the browser must reload so its
    // GET triggers the request-time re-render; #1025 pins the ordering
    // so the stale map and route tables are committed before this
    // outcome exists), OR routes were pruned (issue #1391 — a
    // prune-only tick, e.g. macOS FSEvents coalescing a directory
    // delete so `remove_node` returns empty consumers and nothing else
    // fires; a tab sitting on the deleted route must still reload so it
    // hits the dev 404 path instead of rendering deleted content
    // forever), OR the client-scripts bundle changed (a changed
    // `dist/assets/client/<name>.js` file is only picked up on a full
    // document reload — v1 has no hot-swap event for client scripts), OR
    // the dev server published its live SSR route handle this tick
    // (issue #1826, Dev Self Heal epic #1999 — SSR routes have no
    // `dist/` output path and so can never reach `pages_stale`, which is
    // why an SSR-only project's self-heal never told the tab anything;
    // this is the shared bit BOTH deferred-window channels set, so the
    // healthy publish and the cold-bootstrap recovery heal identically).
    // All five conditions feed ONE `push`, so a mixed SSG/SSR tick that
    // sets `pages_stale` AND `ssr_routes_published` still emits exactly
    // one `Page` event.
    if !outcome.pages_written.is_empty()
        || !outcome.pages_stale.is_empty()
        || !outcome.pages_pruned.is_empty()
        || outcome.client_scripts_changed
        || outcome.ssr_routes_published
    {
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

    /// Issue #958 — a NARROWED content-edit tick writes only the edited
    /// entry's route (plus the always-rendered set); `pages_written`
    /// holding just that page must still yield `ReloadEvent::Page` so
    /// the browser livereloads the edited page exactly as on a full
    /// fan-out tick.
    #[test]
    fn livereload_page_event_fires_for_narrowed_edit() {
        let outcome = BuildOutcome {
            // The narrowed tick's shape: one route rendered + written.
            pages_rendered: 1,
            pages_written: vec![pid("blog/a/index.html")],
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

    /// Issue #979 — a changed client-script bundle requires a full page
    /// reload: the stable `dist/assets/client/<name>.js` URL is only
    /// re-fetched on a full document load (no hot-swap event in v1).
    #[test]
    fn client_scripts_changed_emits_page_reload() {
        let outcome = BuildOutcome {
            client_scripts_rerun: true,
            client_scripts_changed: true,
            ..Default::default()
        };
        assert_eq!(outcome_to_events(&outcome), vec![ReloadEvent::Page]);
    }

    /// Client-scripts runner ran but every output was byte-identical:
    /// no event, no spurious browser reload.
    #[test]
    fn client_scripts_rerun_without_change_emits_nothing() {
        let outcome = BuildOutcome {
            client_scripts_rerun: true,
            client_scripts_changed: false,
            ..Default::default()
        };
        assert!(outcome_to_events(&outcome).is_empty());
    }

    /// Pages written AND client scripts changed in the same tick must
    /// still emit exactly ONE Page event (deduplicated).
    #[test]
    fn pages_and_client_scripts_dedupe_to_single_page_event() {
        let outcome = BuildOutcome {
            pages_written: vec![pid("/a")],
            client_scripts_rerun: true,
            client_scripts_changed: true,
            ..Default::default()
        };
        assert_eq!(outcome_to_events(&outcome), vec![ReloadEvent::Page]);
    }

    /// Issue #1027 — a fully-lazy tick writes NOTHING eagerly; it only
    /// marks routes stale. The browser must still receive exactly one
    /// `Page` event: the reload's GET is what triggers the request-time
    /// re-render, so suppressing the event would leave every open tab
    /// permanently stale under the lazy default.
    #[test]
    fn pages_stale_only_emits_single_page_event() {
        let outcome = BuildOutcome {
            pages_stale: vec![std::path::PathBuf::from("posts/b/index.html")],
            ..Default::default()
        };
        assert_eq!(outcome_to_events(&outcome), vec![ReloadEvent::Page]);
    }

    /// Issue #1027 — a mixed lazy tick (eager own-route write + stale
    /// remainder, the content-edit shape) must dedupe to exactly ONE
    /// Page event, same as the written+client-scripts pin above.
    #[test]
    fn pages_written_and_stale_dedupe_to_single_page_event() {
        let outcome = BuildOutcome {
            pages_written: vec![pid("posts/a/index.html")],
            pages_stale: vec![
                std::path::PathBuf::from("index.html"),
                std::path::PathBuf::from("posts/b/index.html"),
            ],
            ..Default::default()
        };
        assert_eq!(outcome_to_events(&outcome), vec![ReloadEvent::Page]);
    }

    /// Issue #1391 — a prune-only tick (a route's HTML was deleted and
    /// nothing else changed — the shape a coalesced macOS FSEvents
    /// directory delete produces, where `remove_node` returns empty
    /// consumers) must still emit exactly ONE `Page` event. Without this,
    /// a tab left open on the deleted route never reloads and keeps
    /// rendering content that no longer exists.
    #[test]
    fn pages_pruned_only_emits_single_page_event() {
        let outcome = BuildOutcome {
            pages_pruned: vec![std::path::PathBuf::from("dist/posts/old/index.html")],
            ..Default::default()
        };
        assert_eq!(outcome_to_events(&outcome), vec![ReloadEvent::Page]);
    }

    /// Issue #1826 (Dev Self Heal epic #1999, Wave 2 #2002) — the
    /// INVERSION of Wave 1's red pin. Both self-heal channels for the
    /// deferred window (`mark_all_routes_stale`, called by the healthy
    /// Cold-lazy deferred publish per #1182/#1808, AND by the
    /// `recover_cold_bootstrap_after_publish` cold-bootstrap-recovery seam
    /// per #1809) walk ONLY `routes_by_source` — the SSG route table.
    /// `prerender = false` (SSR) routes live in a separate `ssr_routes`
    /// table with no `dist/` output path, so they can never be marked
    /// stale: in a project whose routes are ALL `prerender = false`, both
    /// channels drain a `pages_stale` set of exactly zero paths and the
    /// pre-fix outcome was indistinguishable from a totally empty tick.
    /// The server had already recovered — only the browser was never told.
    ///
    /// `ssr_routes_published` is the shared bit that closes that gap. This
    /// test pins the exact `BuildOutcome` shape BOTH channels now produce
    /// for an SSR-only project — every path list empty, no client scripts,
    /// only the new bit — and asserts it emits exactly one
    /// `ReloadEvent::Page`. Note the bit alone is sufficient: nothing else
    /// in this outcome is non-default.
    #[test]
    fn ssr_only_deferred_publish_outcome_emits_single_page_event() {
        let outcome = BuildOutcome {
            pages_written: Vec::new(),
            pages_stale: Vec::new(),
            pages_pruned: Vec::new(),
            client_scripts_changed: false,
            ssr_routes_published: true,
            ..Default::default()
        };
        assert_eq!(
            outcome_to_events(&outcome),
            vec![ReloadEvent::Page],
            "an SSR-only deferred-publish (or cold-bootstrap-recovery) outcome must tell \
             the open tab to reload — the whole point of issue #1826"
        );
    }

    /// Issue #1826 — the no-regression half. An SSG-only project never
    /// raises `ssr_routes_published` at all (`note_ssr_routes_published`
    /// is a no-op when `ssr_routes` is empty), so its outcomes are
    /// byte-identical to before the fix: a stale-marking tick still emits
    /// exactly one `Page`, and a genuinely empty tick still emits nothing.
    /// Pinned here so a future change that raises the bit unconditionally
    /// (e.g. on every publish regardless of SSR route count) is caught.
    #[test]
    fn ssg_only_outcomes_are_unchanged_without_the_ssr_bit() {
        let stale_tick = BuildOutcome {
            pages_stale: vec![std::path::PathBuf::from("index.html")],
            ssr_routes_published: false,
            ..Default::default()
        };
        assert_eq!(outcome_to_events(&stale_tick), vec![ReloadEvent::Page]);

        let empty_tick = BuildOutcome {
            ssr_routes_published: false,
            ..Default::default()
        };
        assert!(
            outcome_to_events(&empty_tick).is_empty(),
            "a tick that published no SSR routes and changed nothing must stay silent"
        );
    }

    /// Issue #1826 — a MIXED SSG/SSR project sets BOTH `pages_stale` (its
    /// SSG routes) and `ssr_routes_published` (its SSR routes) from the
    /// same self-heal channel. Both feed the SAME single `push`, so the
    /// tab must receive exactly ONE `Page` event, never two — the
    /// "no duplicate events" acceptance criterion, and the reason the fix
    /// widened the existing gate instead of adding a second emit site.
    #[test]
    fn mixed_ssg_and_ssr_signals_dedupe_to_single_page_event() {
        let outcome = BuildOutcome {
            pages_stale: vec![std::path::PathBuf::from("index.html")],
            ssr_routes_published: true,
            ..Default::default()
        };
        assert_eq!(outcome_to_events(&outcome), vec![ReloadEvent::Page]);

        // Same for every other member of the Page gate — no combination
        // of them can produce a second Page event.
        let everything = BuildOutcome {
            pages_written: vec![pid("pages/index.tsx")],
            pages_stale: vec![std::path::PathBuf::from("index.html")],
            pages_pruned: vec![std::path::PathBuf::from("dist/old/index.html")],
            client_scripts_rerun: true,
            client_scripts_changed: true,
            ssr_routes_published: true,
            ..Default::default()
        };
        assert_eq!(outcome_to_events(&everything), vec![ReloadEvent::Page]);
    }

    /// Issue #1027 — stale marks combine with the other event kinds the
    /// same way written pages do: one Page, then Css.
    #[test]
    fn pages_stale_and_css_emits_both() {
        let outcome = BuildOutcome {
            pages_stale: vec![std::path::PathBuf::from("index.html")],
            css_changed: true,
            css_rerun: true,
            ..Default::default()
        };
        assert_eq!(
            outcome_to_events(&outcome),
            vec![ReloadEvent::Page, ReloadEvent::Css]
        );
    }

    /// Issue #1027 — stale marks + client scripts still dedupe to one
    /// Page event.
    #[test]
    fn pages_stale_and_client_scripts_dedupe_to_single_page_event() {
        let outcome = BuildOutcome {
            pages_stale: vec![std::path::PathBuf::from("index.html")],
            client_scripts_rerun: true,
            client_scripts_changed: true,
            ..Default::default()
        };
        assert_eq!(outcome_to_events(&outcome), vec![ReloadEvent::Page]);
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
    fn islands_bundle_changed_empty_components_emits_single_generic_event() {
        // Contract: when the islands bundle changes but the component list is
        // empty (runtime-only diff), outcome_to_events must emit exactly one
        // ReloadEvent::Islands { component: "", bundle_url: <expected> }.
        // The browser consumer keys the reload on bundleUrl, not on component,
        // so the empty-string component must NOT suppress the event — removing
        // the empty-component branch in outcome_to_events must break this test.
        let outcome = BuildOutcome {
            islands_rerun: true,
            islands_changed: true,
            islands_bundle: Some(IslandsBundleInfo {
                changed: true,
                bundle_url: "/assets/islands-abc12345.js".to_string(),
                components: vec![],
            }),
            ..Default::default()
        };
        let events = outcome_to_events(&outcome);
        assert_eq!(
            events.len(),
            1,
            "expected exactly one event for empty components"
        );
        assert_eq!(
            events[0],
            ReloadEvent::Islands {
                component: String::new(),
                bundle_url: "/assets/islands-abc12345.js".to_string(),
            },
            "empty-component event must carry the bundle_url so the consumer can re-import it"
        );
    }

    #[test]
    fn event_names_match_browser_protocol() {
        // The strings here MUST match the addEventListener calls in
        // src/livereload.js.
        let script = include_str!("livereload.js");
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
        for name in ["page", "css", "islands"] {
            assert!(
                script.contains(&format!("addEventListener(\"{name}\"")),
                "livereload.js must subscribe to the `{name}` event"
            );
        }
    }

    /// Issue #1027 — the browser script must derive the SSE stream URL
    /// from its own `<script src>` (which IS base-prefix-aware,
    /// inject.rs `livereload_tag`) instead of hardcoding the unprefixed
    /// `/__zfb/reload`. Pins both the derivation mechanism
    /// (`document.currentScript`) and the unprefixed fallback literal
    /// (kept for non-currentScript execution contexts).
    #[test]
    fn browser_script_derives_stream_url_from_script_src() {
        let script = include_str!("livereload.js");
        assert!(
            script.contains("document.currentScript"),
            "livereload.js must read document.currentScript to honor the \
             dev server's base prefix"
        );
        assert!(
            script.contains("/__zfb/reload"),
            "livereload.js must keep the unprefixed fallback stream URL"
        );
        assert!(
            !script.contains("new EventSource(\"/__zfb/reload\")"),
            "the EventSource constructor must not hardcode the unprefixed \
             stream URL — base-prefixed dev servers would never connect"
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
