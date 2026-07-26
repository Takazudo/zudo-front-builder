//! Dev-server wiring for the `_redirects` **200-rewrite pre-warm**
//! (issue #2004, part 2 of 2 for #1825, Dev Self Heal epic #1999).
//!
//! Wave 3 (#2003) produced the pure enumerator,
//! [`zfb_server::prewarm_rewrite_targets`]. This module is the lifecycle
//! half: it consumes that plan and makes each target servable.
//!
//! ## The gap being closed
//!
//! `zfb_server`'s `serve_from_waterfall` resolves a `_redirects` `200`
//! rewrite through the on-disk waterfall **without** re-running plugin
//! dev-middleware, embed handlers, SSR dispatch, or the render-on-request
//! hook. That is deliberate and correct (#1546): it is the `_redirects`
//! "resolve once, no chaining" contract, and it mirrors Cloudflare
//! Workers' Static Assets layer, which never hands a rewritten request
//! back to the Worker either.
//!
//! It was harmless while `ZFB_DEV_BOOT_LAZY=1` (Auto) required a servable
//! prebuilt `dist/` seed — the target's HTML was already on disk.
//! Seedless `ZFB_DEV_BOOT_LAZY=cold` (#1808) removes that requirement, so
//! at a fresh Cold boot NO waterfall leg has real content for the target
//! and the 404 persists **indefinitely**: the hook that would make it
//! fresh is never invoked for this dispatch path, so nothing claims it
//! unless the target happens to be requested directly.
//!
//! ## What this pass does, and what it deliberately does not
//!
//! For every enumerated target, in `_redirects` file order:
//!
//! 1. **Mark it stale** ([`DevRenderSession::mark_rewrite_target_stale`])
//!    — alongside the other stale-marking passes, so it is claimable and
//!    so the mark rides the normal tick-stale → `pages_stale` →
//!    `ReloadEvent::Page` channel a tab already listens on.
//! 2. **Warm it** through [`LazyRenderAdapter::render_stale_route`] — the
//!    adapter's synchronous core, i.e. the exact claim → render →
//!    guarded-write flow a direct `GET /target` would take.
//!
//! Step 2 is what actually closes the gap. Marking stale alone cannot:
//! a stale entry is only ever consumed by a request for that same path,
//! and the whole point of #1825 is that such a request never arrives —
//! the rewrite serves `/alias`, not `/target`.
//!
//! **Where this runs is the entire design.** The rejected alternative
//! (see #1825 and this epic's decision list) was to give
//! `serve_from_waterfall` a Cold-mode claim-and-render of its own. That
//! would put a render back on the rewrite's request path — exactly the
//! chaining `_redirects` avoids, and a divergence from the Cloudflare
//! layer `serve_from_waterfall` mirrors. This pass instead runs entirely
//! **outside** any request: on the dev boot task, and on the
//! `_redirects` watch task. `serve_from_waterfall` is untouched; the
//! rewrite request path still re-runs nothing.
//!
//! ## Lifecycle — registration points (the stated answer)
//!
//! - **Boot: yes.** [`prewarm_rewrite_targets_decision`] gates the pass
//!   to Cold, and `commands::dev`'s deferred boot hook runs it right
//!   after `run_boot_render`, once the route tables are published and
//!   before the boot outcome's tick-stale drain — so step 1's marks land
//!   in the same single boot broadcast as every other boot mark.
//! - **Live edit of `public/_redirects`: yes — RE-REGISTERED, no restart
//!   required.** The dedicated `_redirects` watch task (#1546) already
//!   re-parses the file and swaps the live [`RedirectsHandle`] in place;
//!   it now re-runs this pass immediately afterwards, against the
//!   just-swapped ruleset. So adding `/alias /brand-new 200` to a running
//!   Cold dev server makes `/alias` servable without a restart, matching
//!   the no-restart promise the `_redirects` watch was built for. The
//!   alternative (boot-only registration) was rejected as a worse
//!   surprise than the pass's cost: a re-warm renders only the targets a
//!   rule set names, each is a no-op when already fresh (the stale
//!   pre-check returns before touching the renderer), and the edit that
//!   triggers it is a human keystroke, not a hot loop.
//!
//! Both entry points funnel through [`prewarm_rewrite_targets_now`], so
//! there is one implementation of "what pre-warming means" and the two
//! lifecycle moments cannot drift.

use zfb_server::{prewarm_rewrite_targets, PrewarmSkipReason, Redirects, RedirectsHandle};

use crate::commands::dev::{BootLazyMode, DevRenderSession};
use crate::lazy_render_adapter::{LazyRenderAdapter, LazyRenderOutcome};

/// What one pre-warm pass did. Diagnostics + tests only — no caller
/// branches on it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PrewarmPassReport {
    /// Targets the enumerator produced.
    pub enumerated: usize,
    /// Targets that resolved to a live SSG route and were marked stale.
    pub marked: usize,
    /// Targets whose warm render actually wrote (or byte-deduped) HTML.
    pub rendered: usize,
}

/// Should the rewrite pre-warm run for this boot mode?
///
/// **Cold only** — the same "non-Cold modes are unaffected" contract the
/// neighbouring `premark_stale_*_decision` predicates in `commands::dev`
/// carry, and for the same reason:
///
/// - `Off` (eager): the boot render writes every route's HTML before the
///   first request, so a rewrite target is on disk already.
/// - `Auto`: boot-lazy only engages with a servable prebuilt `dist/`
///   seed, so the waterfall's `dist_root` leg has real (stale but
///   servable) bytes for the target — precisely why #1825 could not
///   happen before Cold existed.
/// - `Cold`: seedless. No leg has content, and the hook never runs for
///   this path. This is the only mode with a gap to close.
pub(crate) fn prewarm_rewrite_targets_decision(mode: BootLazyMode) -> bool {
    matches!(mode, BootLazyMode::Cold)
}

/// Run one pre-warm pass against `redirects`.
///
/// Synchronous and blocking — it renders. Both call sites are already
/// off the request path (the boot task's hook closure, and the
/// `_redirects` watch task's `spawn_blocking`), which is the whole point:
/// see the module docs.
///
/// `base_prefix` must be the SAME value the server computed for its
/// `AppState` (`zfb_types::dev_mount_prefix(cfg.base)`), or the
/// enumerator will emit prefix-stripped spellings the waterfall does not
/// probe.
pub(crate) fn prewarm_rewrite_targets_now(
    redirects: &Redirects,
    base_prefix: Option<&str>,
    session: &DevRenderSession,
    adapter: &LazyRenderAdapter,
) -> PrewarmPassReport {
    let plan = prewarm_rewrite_targets(redirects, base_prefix);
    let mut report = PrewarmPassReport {
        enumerated: plan.targets.len(),
        ..PrewarmPassReport::default()
    };

    for target in &plan.targets {
        // Step 1 — stale-mark. A `None` here means the target is not an
        // SSG route in this session (SSR route, unexpanded dynamic
        // route, or a rule pointing at nothing). Step 2 would be a
        // `NoRoute` no-op, so skip it and keep the log honest.
        if session
            .mark_rewrite_target_stale(&target.request_path)
            .is_none()
        {
            tracing::debug!(
                site = "rewrite_prewarm",
                target = %target.raw_target,
                request_path = %target.request_path,
                "_redirects 200-rewrite target resolves to no SSG route; nothing to pre-warm",
            );
            continue;
        }
        report.marked += 1;

        // Step 2 — warm. Uses `request_path` verbatim: the enumerator
        // derives it from the encoded canonical form through the same
        // pipeline the live waterfall uses, and `lookup_by_url`'s
        // documented input contract is exactly that shape. Re-deriving
        // or "tidying" the spelling here is how a pre-warm silently
        // misses.
        match adapter.render_stale_route(&target.request_path) {
            LazyRenderOutcome::Rendered { .. } => report.rendered += 1,
            other => tracing::debug!(
                site = "rewrite_prewarm",
                target = %target.raw_target,
                outcome = ?other,
                "_redirects 200-rewrite target pre-warm produced no write",
            ),
        }
    }

    for skip in &plan.skipped {
        tracing::debug!(
            site = "rewrite_prewarm",
            target = %skip.raw_target,
            reason = ?skip.reason,
            "_redirects 200-rewrite target not pre-warmed",
        );
    }
    // An external / protocol-relative target is the one skip class worth
    // saying out loud: it is almost always an authoring mistake in a
    // `200` rule (Cloudflare cannot proxy external domains either), and
    // it is silent otherwise.
    let external = plan
        .skipped
        .iter()
        .filter(|s| s.reason == PrewarmSkipReason::ExternalTarget)
        .count();
    if external > 0 {
        tracing::warn!(
            site = "rewrite_prewarm",
            count = external,
            "_redirects: {external} `200` rule(s) target an external URL; a rewrite cannot \
             proxy another origin, so those rules serve nothing locally",
        );
    }

    report
}

/// Boot / live-edit entry point: snapshot the live [`RedirectsHandle`]
/// under a short read lock (the same discipline `serve_page` uses) and
/// run one pass.
///
/// A no-op — not even a lock — when `session`/`adapter` are absent (the
/// renderer is disabled, or the lazy switch is off so no adapter was
/// built) or the boot mode is not Cold.
pub(crate) fn prewarm_rewrite_targets_for_dev(
    mode: BootLazyMode,
    redirects: &RedirectsHandle,
    base_prefix: Option<&str>,
    session: Option<&DevRenderSession>,
    adapter: Option<&LazyRenderAdapter>,
) -> Option<PrewarmPassReport> {
    if !prewarm_rewrite_targets_decision(mode) {
        return None;
    }
    let (session, adapter) = (session?, adapter?);
    let snapshot: Redirects = redirects.read().unwrap_or_else(|p| p.into_inner()).clone();
    Some(prewarm_rewrite_targets_now(
        &snapshot,
        base_prefix,
        session,
        adapter,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cold_pre_warms_rewrite_targets() {
        // The "non-Cold boot modes are unaffected" acceptance criterion,
        // asserted at the gate rather than assumed at the call sites.
        assert!(prewarm_rewrite_targets_decision(BootLazyMode::Cold));
        assert!(!prewarm_rewrite_targets_decision(BootLazyMode::Auto));
        assert!(!prewarm_rewrite_targets_decision(BootLazyMode::Off));
    }

    /// The pass must hand the adapter a spelling that RESOLVES. The
    /// distinguishing evidence is the outcome class: a near-miss
    /// spelling — the failure mode #2003's decision table exists to
    /// prevent, and the one that looks like it works — produces
    /// `NoRoute`, whereas a hit gets past the reverse lookup and the
    /// stale pre-check and only then fails on the deliberately-absent
    /// renderer (`RendererUnavailable`). So `marked == 1` is not
    /// bookkeeping: it is the proof that step 2 addressed a real route.
    ///
    /// The encoded target is the load-bearing one — a pure-ASCII target
    /// cannot falsify this, since its decoded and canonical forms
    /// coincide.
    #[test]
    fn pass_marks_and_warms_exactly_the_targets_that_resolve() {
        use std::path::PathBuf;
        use std::sync::{Arc, Mutex};
        use zfb_build::renderer::RouteUniverseEntry;
        use zfb_build::DevAssetPipeline;

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let session = crate::commands::dev::stub_session_for_adapter_tests(
            root.clone(),
            vec![(
                root.join("pages/posts/cafe.tsx"),
                vec![RouteUniverseEntry {
                    url_path: "/posts/caf%C3%A9".to_string(),
                    output_path: PathBuf::from("posts/caf%C3%A9/index.html"),
                    route_key: "/posts/café".to_string(),
                    static_html: false,
                    source_path: None,
                }],
            )],
            // No renderer: a resolved target gets as far as the render
            // step and stops there, which is exactly the signal this
            // test reads.
            Arc::new(Mutex::new(None)),
            true,
        );
        let pipeline = DevAssetPipeline::new();
        let adapter = LazyRenderAdapter::new(
            session.clone(),
            pipeline.request_writer(),
            root.join("dev-pages"),
            Default::default(),
        );

        let redirects = Redirects::parse(
            "\
/alias /posts/caf%C3%A9 200
/alias-missing /no/such/route 200
/alias-splat/* /new/:splat 200
/moved /elsewhere 301
",
        );
        let report = prewarm_rewrite_targets_now(&redirects, None, &session, &adapter);

        assert_eq!(
            report,
            PrewarmPassReport {
                // The splat target is non-concrete and the 301 is out of
                // scope, so two rules enumerate.
                enumerated: 2,
                marked: 1,
                rendered: 0,
            },
        );
        // The mark landed on the route's real output path, in the
        // tick-stale buffer every other stale-marking pass feeds.
        assert_eq!(
            session.take_tick_stale_for_tests(),
            vec![PathBuf::from("posts/caf%C3%A9/index.html")],
        );
    }
}
