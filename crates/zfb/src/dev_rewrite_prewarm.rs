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
//! 1. **Ask whether it is already fresh, and mark it stale only if it is
//!    not** ([`DevRenderSession::mark_rewrite_target_stale_if_needed`]).
//!    The order is load-bearing: `mark_stale` re-inserts a stale entry
//!    unconditionally, so marking first would make the freshness question
//!    answer itself and the pass would re-render every target of every
//!    rule on every run. Asking first is also the only reading that
//!    matches production — the adapter consults the same stale map
//!    through the same `claim_stale` and serves the on-disk bytes when it
//!    finds nothing. When it does mark, the mark rides the normal
//!    tick-stale → `pages_stale` → `ReloadEvent::Page` channel a tab
//!    already listens on.
//! 2. **Warm it** through [`LazyRenderAdapter::render_stale_route`] — the
//!    adapter's synchronous core, i.e. the exact claim → render →
//!    guarded-write flow a direct `GET /target` would take.
//!
//! Step 2 is what actually closes the gap. Marking stale alone cannot:
//! a stale entry is only ever consumed by a request for that same path,
//! and the whole point of #1825 is that such a request never arrives —
//! the rewrite serves `/alias`, not `/target`.
//!
//! ### What this costs, stated plainly
//!
//! At a fresh Cold boot the boot render's own `mark_all_routes_stale`
//! has just marked EVERY route stale, so every enumerated target really
//! does get rendered — uncapped, on the boot task. That is the fix, not
//! an accident: a target nobody renders is a permanent 404. The bound is
//! the number of concrete `200`-rule targets a project authors, and the
//! work is the same work a direct `GET` for each would do. What the
//! freshness check buys is every LATER run: a live `_redirects` edit
//! re-runs the pass and, for every target already warmed, does nothing
//! at all — no render, no stale mark, no reload.
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
//!   rule set names, each is genuinely a no-op when already fresh (step 1
//!   returns `Fresh` before the renderer is touched at all), and the edit
//!   that triggers it is a human keystroke, not a hot loop.
//! - **After a cold-bootstrap recovery: yes.** When the deferred Cold
//!   bundle FAILS (#1809), the boot hook's pre-warm runs against the
//!   empty scaffold route tables and warms nothing. The recovery's own
//!   `mark_all_routes_stale` then makes each target *claimable* but
//!   nothing ever claims it — a rewrite serves `/alias`, and `/target` is
//!   never requested. So `recover_cold_bootstrap_after_publish` raises a
//!   one-shot latch that `commands::dev`'s `on_outcome` drains to re-run
//!   this pass against the freshly published tables.
//!
//! All three entry points funnel through [`prewarm_rewrite_targets_now`],
//! so there is one implementation of "what pre-warming means" and the
//! lifecycle moments cannot drift.
//!
//! ## Who drains the marks — the other half of the lifecycle
//!
//! Step 1 writes into the per-tick stale buffer, which is drained
//! elsewhere and turned into one `ReloadEvent::Page`. A registration
//! point that runs the pass without arranging for that drain leaks the
//! marks: they surface later as spurious `pages_stale` in an unrelated
//! tick, and the run that earned them broadcasts nothing. So exactly one
//! of the three has a caller-side drain, and the other two do their own:
//!
//! - **Boot** — the deferred hook drains `take_tick_stale()` a few steps
//!   after the pass and folds it into the single `run_with_boot`
//!   broadcast (`RewritePrewarmWiring::run_for_caller_drain`).
//! - **Live `_redirects` edit** and **cold-bootstrap recovery** — no
//!   drain follows either, so both use
//!   `RewritePrewarmWiring::run_and_broadcast`, which drains and sends.
//!   The recovery case additionally hands its tick's OWN events to that
//!   call rather than sending them first: a tab reloaded before the
//!   target is warm lands straight back on the dev 404, and the pass's
//!   marks would then arrive in an already-drained buffer. Deferring the
//!   send gives the recovery path the same "one broadcast, after the
//!   warm" ordering the boot path gets for free.

use zfb_server::{prewarm_rewrite_targets, PrewarmSkipReason, Redirects, RedirectsHandle};

use crate::commands::dev::{BootLazyMode, DevRenderSession, RewriteTargetMark};
use crate::lazy_render_adapter::{LazyRenderAdapter, LazyRenderOutcome};

/// What one pre-warm pass did. Diagnostics + tests only — no caller
/// branches on it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PrewarmPassReport {
    /// Targets the enumerator produced.
    pub enumerated: usize,
    /// Targets that resolved to a live SSG route which was NOT fresh, and
    /// were therefore marked stale.
    pub marked: usize,
    /// Targets the stale map reported as already current — neither marked
    /// nor rendered. The pass's genuine no-op class; see the `Fresh` arm.
    pub already_fresh: usize,
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
        // Step 1 — resolve, and stale-mark only what is not already
        // fresh. The three outcomes are deliberately NOT interchangeable:
        //
        // - `Fresh` is the pass's real no-op. The stale map says a direct
        //   `GET` for this target would serve the on-disk bytes without
        //   re-rendering, so neither does this pass, and it does not
        //   queue a `pages_stale` mark that would bounce a tab for
        //   nothing. This is what keeps a live `_redirects` edit from
        //   re-rendering every target of every rule.
        // - `Unresolved` must NOT skip step 2. The reverse URL index also
        //   misses for a DYNAMIC injected route, which has no concrete
        //   URL at boot and is instead resolved by the adapter's own
        //   injected-pattern fallback in step 2 (that fallback marks such
        //   a route stale itself, by construction). Skipping would leave
        //   `/alias /preset-docs/foo 200` at the very 404 this pass
        //   exists to prevent, even though a direct GET renders it fine.
        // - `MarkedStale` is the #1825 case: the target has no servable
        //   bytes yet, so step 2 must render it.
        match session.mark_rewrite_target_stale_if_needed(&target.request_path) {
            RewriteTargetMark::MarkedStale(_) => report.marked += 1,
            RewriteTargetMark::Fresh => {
                report.already_fresh += 1;
                continue;
            }
            RewriteTargetMark::Unresolved => {}
        }

        // Step 2 — warm. Uses `request_path` verbatim: the enumerator
        // derives it from the encoded canonical form through the same
        // pipeline the live waterfall uses, and `lookup_by_url`'s
        // documented input contract is exactly that shape. Re-deriving
        // or "tidying" the spelling here is how a pre-warm silently
        // misses.
        match adapter.render_stale_route(&target.request_path) {
            LazyRenderOutcome::Rendered { .. } => report.rendered += 1,
            LazyRenderOutcome::NoRoute => tracing::debug!(
                site = "rewrite_prewarm",
                target = %target.raw_target,
                request_path = %target.request_path,
                "_redirects 200-rewrite target matches no SSG route and no injected pattern; \
                 nothing to pre-warm",
            ),
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
        // What a Cold boot looks like when the pass runs: `run_boot_render`'s
        // boot-lazy branch has just called `mark_all_routes_stale`, so no
        // route has servable bytes yet. Without this the route would be
        // FRESH and the pass would correctly decline to do anything — see
        // the sibling test below.
        session.mark_routes_stale([PathBuf::from("posts/caf%C3%A9/index.html")]);
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
                already_fresh: 0,
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

    /// Epic-review finding on #1999: the freshness check must be
    /// REACHABLE, not decorative.
    ///
    /// The pre-fix pass called `mark_stale` and only then asked the
    /// adapter whether the route was stale — so the answer was always
    /// "yes, because I just said so", and every boot and every live
    /// `_redirects` edit re-rendered every 200-rewrite target. The
    /// module docs claimed the opposite ("a no-op when already fresh").
    ///
    /// Same fixture as the test above with ONE difference: nothing has
    /// marked the route stale, i.e. it has already been warmed. The pass
    /// must then do nothing at all — and "nothing" is asserted three
    /// ways, because `rendered: 0` alone is satisfied by the old
    /// behaviour too (the stub has no renderer):
    ///
    /// - `already_fresh: 1, marked: 0` — the check ran and answered.
    /// - the stale map is still EMPTY — the pass did not re-stale the
    ///   route, which is what would force a pointless re-render on the
    ///   next real request.
    /// - the tick-stale buffer is EMPTY — no spurious `pages_stale`, so
    ///   no `ReloadEvent::Page` bounces a tab for a page that did not
    ///   change. Restoring the unconditional `mark_stale` fails all
    ///   three.
    #[test]
    fn already_fresh_target_is_neither_marked_nor_rendered() {
        use std::path::{Path, PathBuf};
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

        let report = prewarm_rewrite_targets_now(
            &Redirects::parse("/alias /posts/caf%C3%A9 200\n"),
            None,
            &session,
            &adapter,
        );

        assert_eq!(
            report,
            PrewarmPassReport {
                enumerated: 1,
                marked: 0,
                already_fresh: 1,
                rendered: 0,
            },
        );
        assert!(
            session
                .claim_stale(Path::new("posts/caf%C3%A9/index.html"))
                .is_none(),
            "a fresh target must not be re-staled by the pre-warm",
        );
        assert!(
            session.take_tick_stale_for_tests().is_empty(),
            "a fresh target must queue no `pages_stale` mark, so no tab is bounced",
        );
    }

    /// Codex review finding on #2004: a step-1 miss must NOT skip step 2.
    ///
    /// A dynamic injected route (`/preset-docs/[slug]`) has no concrete
    /// URL at boot, so `lookup_by_url` — and therefore
    /// `mark_rewrite_target_stale` — misses by design; the adapter's own
    /// injected-pattern fallback is what resolves it, and marks it stale
    /// itself. Short-circuiting on the miss would leave
    /// `/alias /preset-docs/foo 200` on exactly the 404 this pass exists
    /// to prevent, even though a direct GET renders it fine.
    ///
    /// The evidence is the stale entry the fallback inserts for the
    /// synthesized output path: it can only exist if step 2 ran.
    #[test]
    fn injected_route_target_still_reaches_the_adapter_after_a_step_one_miss() {
        use std::path::{Path, PathBuf};
        use std::sync::{Arc, Mutex};
        use zfb_build::DevAssetPipeline;
        use zfb_build::InjectedRoute;
        use zfb_server::InjectedRouteSet;

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let session = crate::commands::dev::stub_session_for_adapter_tests(
            root.clone(),
            // No SSG routes at all: the ONLY way to reach the target is
            // the adapter's injected-pattern fallback.
            Vec::new(),
            Arc::new(Mutex::new(None)),
            true,
        );
        let pipeline = DevAssetPipeline::new();
        let adapter = LazyRenderAdapter::new(
            session.clone(),
            pipeline.request_writer(),
            root.join("dev-pages"),
            InjectedRouteSet::new(vec![InjectedRoute {
                pattern: "/preset-docs/[slug]".into(),
                entrypoint: PathBuf::from("/tmp/stub.tsx"),
                plugin: "test-plugin".into(),
                prerender: None,
            }]),
        );

        let report = prewarm_rewrite_targets_now(
            &Redirects::parse("/alias /preset-docs/foo 200\n"),
            None,
            &session,
            &adapter,
        );
        assert_eq!(report.enumerated, 1);
        assert_eq!(report.marked, 0, "the reverse index cannot know this route");
        assert!(
            session
                .claim_stale(Path::new("preset-docs/foo/index.html"))
                .is_some(),
            "step 2 must still run: only the adapter's injected fallback can \
             stale this synthesized output path",
        );
    }
}
