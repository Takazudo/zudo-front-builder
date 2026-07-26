//! Enumerate and normalize `_redirects` **200-rewrite targets** for
//! dev pre-warming (issue #2003, part 1 of 2 for #1825).
//!
//! This module is **pure**: it reads a parsed [`Redirects`] table plus the
//! server's optional base prefix and returns a deterministic answer. It
//! touches no dev-server state, spawns nothing, renders nothing, and has no
//! behaviour of its own. Wiring it into the dev lifecycle is #2004.
//!
//! ## Why this exists
//!
//! [`crate::routes`]'s `serve_from_waterfall` resolves a `_redirects` `200`
//! rewrite through the on-disk waterfall **without** re-running the
//! render-on-request hook. That is deliberate and correct (#1546): it matches
//! the `_redirects` "resolve once, no chaining" contract and mirrors
//! Cloudflare Workers' Static Assets layer, which never hands a rewritten
//! request back to the Worker either.
//!
//! It was harmless while `ZFB_DEV_BOOT_LAZY=1` required a servable prebuilt
//! `dist/` seed — the target's HTML was already on disk. Seedless
//! `ZFB_DEV_BOOT_LAZY=cold` (#1808) removes that requirement, so at a fresh
//! Cold boot no waterfall leg has real content and the 404 persists
//! indefinitely: the hook never runs for a rewrite target, so nothing ever
//! claims it. The chosen fix (epic #1999) is to **pre-warm / mark rewrite
//! targets stale**, which makes enumerating them correctly the whole game.
//!
//! ## Decision table
//!
//! The `_redirects` spec says nothing about pre-warming, so each row below is
//! a decision this module makes on purpose. The governing principle
//! throughout: **mirror what a real request for that target would do**, by
//! calling the very same normalization functions the request path calls —
//! never by re-deriving an equivalent-looking spelling.
//!
//! | Case | Decision | Reasoning |
//! |---|---|---|
//! | Canonical path form | Reuse the production pipeline verbatim: [`waterfall_trimmed_for_rewrite_target`] → [`canonical_encode_path`] → [`lookup_keys`], the exact functions `routes.rs` calls at the rewrite call site and inside `serve_from_waterfall`. | The page cache and rendered-output dirs key on the **encoded canonical** path (#1768). A near-miss spelling would produce a pre-warm that silently misses while appearing to work. Sharing the functions makes divergence impossible by construction rather than by review. |
//! | Splat rules (`/old/* /new/:splat 200`) | **Skip** — but keyed on the **target**, not the source. A splat SOURCE with a concrete target (`/old/* /fallback 200`) IS enumerated; only a target still carrying a `:splat` this rule would substitute is skipped. | A `:splat`/`:name` target is a family of routes, not one route; resolving it needs a concrete request that has not happened yet. Expanding against the route table was rejected: it would make this function impure, couple it to a snapshot that changes across the dev session, and (at a Cold boot, the case being fixed) the route table is exactly what has not settled yet. Skipping costs nothing beyond the existing behaviour — such a target is no worse off than today. |
//! | Placeholders in the target (`/:lang/about`) | **Disqualifying**, same rule and reasoning as `:splat`. A `:` token the rule cannot resolve (no such placeholder in its source, no splat) is NOT disqualifying — `substitute_target` leaves it literal, so the target really is one concrete path. | Concreteness is about what production would substitute, not about the presence of a colon. |
//! | Query string on the target (`/target?x=1`) | **Stripped**, by delegating to [`waterfall_trimmed_for_rewrite_target`]. | Query strings are not part of a filesystem / page-cache lookup, and that is already what the live rewrite does — preview splits the query before its static waterfall and dev mirrors it. Stripping here keeps pre-warm and serve on the same key. |
//! | Fragment on the target (`/target#a`) | **Not stripped** — deliberately, because production does not strip it either. | Fidelity beats cleverness: the pre-warm key must equal the serve key. If a fragment-bearing target is wrong, it is wrong identically on both sides, and pre-warm still hits whatever the waterfall probes. |
//! | Base-path prefix | Targets are authored in **full-path spec form including the base** (e.g. `/pj/site/rewritten/`); the prefix is stripped via the same `strip_prefix_from_path` boundary rule production uses. A target that does NOT carry the prefix passes through unchanged — again matching production. | The dev route universe, the render-on-request hook, and the waterfall all work in prefix-STRIPPED space. Emitting anything else would miss. |
//! | External / absolute-URL targets | **Excluded.** Already rejected at parse time ([`Redirects::parse`] drops a `200` rule with a `scheme:` target, per the Cloudflare spec's "cannot proxy external domains"), and re-checked here as defense in depth. **Protocol-relative** targets (`//example.com/x`) are excluded here too — parse does not catch those, since they start with `/`. | There is nothing local to pre-warm. A protocol-relative target would otherwise normalize to the nonsense lookup path `example.com/x`. |
//! | Targets that are not absolute paths (`foo/bar`) | **Excluded.** | Not a same-site absolute path. Production would still probe some trimmed form of it, but pre-warming a path the author never meant risks a spurious render; declining to pre-warm only leaves the status quo. |
//! | Non-200 rules | **Out of scope** — only `is_rewrite` (status `200`) rules are read. | A `301`/`302` hands the browser a new URL, which then arrives as a normal request and goes through the hook like any other. Redirects were never affected by this bug. |
//! | Duplicates | Deduped on the normalized request path, keeping **first-file-order** occurrence. | Mirrors `_redirects` first-match-wins, and keeps the output deterministic for the caller's registration pass. |
//!
//! Skipped targets are not silently dropped: they come back in
//! [`PrewarmPlan::skipped`] with a [`PrewarmSkipReason`], so #2004 can log
//! them without this module doing any I/O.

use crate::redirects::Redirects;
use crate::routes::{canonical_encode_path, lookup_keys, waterfall_trimmed_for_rewrite_target};

/// One `_redirects` `200`-rewrite target, normalized into every spelling a
/// caller needs to pre-warm it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrewarmTarget {
    /// The target exactly as authored in `_redirects` (base prefix and
    /// query string still attached). Diagnostics only — never a lookup key.
    pub raw_target: String,
    /// The path a normal GET for this target would hand the dev layers:
    /// base-prefix-stripped, query-stripped, percent-DECODED, with a
    /// leading `/` (`/` for the site root).
    ///
    /// This is the shape `RenderOnRequestHook::render_if_stale` and the dev
    /// route universe's URL index take.
    pub request_path: String,
    /// `request_path` re-encoded per component into the canonical form the
    /// renderer wrote to disk (#1768), WITHOUT a leading slash — byte-identical
    /// to `serve_from_waterfall`'s own local `canonical`, which it passes to
    /// the `html_root` / `dist_root` disk legs.
    pub canonical_path: String,
    /// The page-cache candidate keys `serve_from_waterfall` probes, in its
    /// priority order (exact, `/index.html`, trailing slash). Each carries a
    /// leading `/`.
    pub lookup_keys: Vec<String>,
}

/// Why a `200` rewrite rule contributed no pre-warm target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrewarmSkipReason {
    /// The target still carries a `:splat` / `:name` token this rule would
    /// substitute per-request — a family of routes, not one route.
    NonConcreteTarget,
    /// An absolute URL (`https://…`) or a protocol-relative one (`//host/…`).
    /// Nothing local to pre-warm.
    ExternalTarget,
    /// The target is not a same-site absolute path (does not start with `/`).
    NotAbsolutePath,
    /// A later rule resolved to a request path an earlier rule already
    /// contributed.
    DuplicateOfEarlierTarget,
}

/// A skipped rule's target plus the reason, for caller-side logging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrewarmSkip {
    pub raw_target: String,
    pub reason: PrewarmSkipReason,
}

/// The full, deterministic answer for one `_redirects` table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrewarmPlan {
    /// Concrete, normalized, deduped targets in `_redirects` file order.
    pub targets: Vec<PrewarmTarget>,
    /// Every `200` rule that contributed nothing, with its reason.
    pub skipped: Vec<PrewarmSkip>,
}

/// Turn a parsed `_redirects` table into the set of 200-rewrite targets to
/// register for pre-warming.
///
/// Pure — see the module docs for the full decision table behind every
/// inclusion and exclusion. `base_prefix` is the dev server's mount prefix
/// (`AppState::base_prefix`): `None`, or `Some("/pj/site")` for
/// `base: "/pj/site/"`.
pub fn prewarm_rewrite_targets(redirects: &Redirects, base_prefix: Option<&str>) -> PrewarmPlan {
    let mut plan = PrewarmPlan::default();
    for raw in redirects.rewrite_targets() {
        let reason = if !raw.is_concrete {
            Some(PrewarmSkipReason::NonConcreteTarget)
        } else if is_external_or_protocol_relative(raw.target) {
            Some(PrewarmSkipReason::ExternalTarget)
        } else if !raw.target.starts_with('/') {
            Some(PrewarmSkipReason::NotAbsolutePath)
        } else {
            None
        };
        if let Some(reason) = reason {
            plan.skipped.push(PrewarmSkip {
                raw_target: raw.target.to_string(),
                reason,
            });
            continue;
        }

        // The production pipeline, verbatim: the same two calls
        // `routes.rs` makes at the rewrite call site and at the top of
        // `serve_from_waterfall`. Anything reimplemented here would be a
        // near-miss spelling waiting to happen.
        let trimmed = waterfall_trimmed_for_rewrite_target(raw.target, base_prefix);
        let canonical_path = canonical_encode_path(&trimmed);
        let request_path = if trimmed.is_empty() {
            "/".to_string()
        } else {
            format!("/{trimmed}")
        };

        if plan.targets.iter().any(|t| t.request_path == request_path) {
            plan.skipped.push(PrewarmSkip {
                raw_target: raw.target.to_string(),
                reason: PrewarmSkipReason::DuplicateOfEarlierTarget,
            });
            continue;
        }

        plan.targets.push(PrewarmTarget {
            raw_target: raw.target.to_string(),
            request_path,
            lookup_keys: lookup_keys(&canonical_path),
            canonical_path,
        });
    }
    plan
}

/// `true` for an absolute URL (`https://host/x`) or a protocol-relative one
/// (`//host/x`).
///
/// The absolute case is already rejected at parse time for `200` rules; this
/// is defense in depth plus the protocol-relative case, which parse does NOT
/// catch (it starts with `/`, so it never looks like a `scheme:`).
fn is_external_or_protocol_relative(target: &str) -> bool {
    target.starts_with("//") || crate::redirects::is_external_target(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `_redirects` fixture covering every row of the module-doc
    /// decision table. Line comments name the row each rule exercises.
    const FIXTURE: &str = "\
# comment line, skipped by the parser
/alias /target 200
/root-alias / 200
/with-query /queried?flavor=vanilla 200
/with-fragment /fragmented#section 200
/colon-literal /docs/:8080/index 200
/splat-src/* /concrete-fallback 200
/splat-target/* /new/:splat 200
/:lang/about /:lang/about-us 200
/proto-rel //example.com/evil 200
/relative-target relative/path 200
/dup-one /shared-target 200
/dup-two /shared-target 200
/encoded /posts/caf%C3%A9 200
/moved /moved-to 301
/temp /temp-to
";

    fn plan(base_prefix: Option<&str>) -> PrewarmPlan {
        prewarm_rewrite_targets(&Redirects::parse(FIXTURE), base_prefix)
    }

    fn request_paths(plan: &PrewarmPlan) -> Vec<String> {
        plan.targets
            .iter()
            .map(|t| t.request_path.clone())
            .collect()
    }

    fn skip_reason(plan: &PrewarmPlan, raw_target: &str) -> Option<PrewarmSkipReason> {
        plan.skipped
            .iter()
            .find(|s| s.raw_target == raw_target)
            .map(|s| s.reason)
    }

    // --- row: non-200 rules are out of scope ----------------------------

    #[test]
    fn non_200_rules_contribute_nothing() {
        let p = plan(None);
        // `/moved-to` (301) and `/temp-to` (default 302) appear in neither
        // list — they are never even read.
        assert!(!request_paths(&p).iter().any(|r| r == "/moved-to"));
        assert!(!request_paths(&p).iter().any(|r| r == "/temp-to"));
        assert!(p.skipped.iter().all(|s| s.raw_target != "/moved-to"));
        assert!(p.skipped.iter().all(|s| s.raw_target != "/temp-to"));
    }

    // --- row: canonical path form ---------------------------------------

    #[test]
    fn plain_target_normalizes_to_the_paths_the_waterfall_probes() {
        let p = plan(None);
        let t = p
            .targets
            .iter()
            .find(|t| t.raw_target == "/target")
            .expect("plain target enumerated");
        assert_eq!(t.request_path, "/target");
        assert_eq!(t.canonical_path, "target");
        assert_eq!(
            t.lookup_keys,
            vec!["/target", "/target/index.html", "/target/"]
        );
    }

    /// The highest-risk row: the emitted spelling must be what
    /// `serve_from_waterfall` ACTUALLY looks up, not a lookalike. Verified
    /// against the production functions themselves, for a target whose
    /// canonical form differs from its decoded form (`%C3%A9` → `café` →
    /// re-encoded). A pure-ASCII target could not falsify this.
    #[test]
    fn canonical_spelling_equals_the_production_waterfall_pipeline() {
        let p = plan(None);
        let t = p
            .targets
            .iter()
            .find(|t| t.raw_target == "/posts/caf%C3%A9")
            .expect("percent-encoded target enumerated");

        // Exactly what routes.rs does: rewrite call site, then the top of
        // `serve_from_waterfall`.
        let production_trimmed = waterfall_trimmed_for_rewrite_target("/posts/caf%C3%A9", None);
        let production_canonical = canonical_encode_path(&production_trimmed);
        let production_keys = lookup_keys(&production_canonical);

        assert_eq!(production_trimmed, "posts/café", "decoded once");
        assert_eq!(production_canonical, "posts/caf%C3%A9", "re-encoded");
        assert_eq!(t.canonical_path, production_canonical);
        assert_eq!(t.lookup_keys, production_keys);
        // And the hook-facing shape stays DECODED, like a real request's.
        assert_eq!(t.request_path, "/posts/café");
    }

    #[test]
    fn root_target_normalizes_to_the_root_lookup_keys() {
        let p = plan(None);
        let t = p
            .targets
            .iter()
            .find(|t| t.raw_target == "/")
            .expect("root target enumerated");
        assert_eq!(t.request_path, "/");
        assert_eq!(t.canonical_path, "");
        assert_eq!(t.lookup_keys, lookup_keys(""));
        assert_eq!(t.lookup_keys, vec!["/", "/index.html"]);
    }

    // --- row: query string / fragment ------------------------------------

    #[test]
    fn query_string_is_stripped_from_the_target() {
        let p = plan(None);
        let t = p
            .targets
            .iter()
            .find(|t| t.raw_target == "/queried?flavor=vanilla")
            .expect("queried target enumerated");
        assert_eq!(t.request_path, "/queried");
        assert_eq!(t.lookup_keys[0], "/queried");
    }

    #[test]
    fn fragment_is_preserved_because_the_live_rewrite_preserves_it() {
        let p = plan(None);
        let t = p
            .targets
            .iter()
            .find(|t| t.raw_target == "/fragmented#section")
            .expect("fragment target enumerated");
        // Not a cleverness decision — this asserts parity with production.
        assert_eq!(
            t.request_path,
            format!(
                "/{}",
                waterfall_trimmed_for_rewrite_target("/fragmented#section", None)
            )
        );
        assert_eq!(t.request_path, "/fragmented#section");
    }

    // --- row: splat / placeholder targets --------------------------------

    #[test]
    fn splat_target_is_skipped_but_splat_source_with_concrete_target_is_not() {
        let p = plan(None);
        assert_eq!(
            skip_reason(&p, "/new/:splat"),
            Some(PrewarmSkipReason::NonConcreteTarget)
        );
        // A splat SOURCE is fine — only the target's concreteness matters.
        assert!(request_paths(&p).contains(&"/concrete-fallback".to_string()));
    }

    #[test]
    fn placeholder_target_is_skipped() {
        let p = plan(None);
        assert_eq!(
            skip_reason(&p, "/:lang/about-us"),
            Some(PrewarmSkipReason::NonConcreteTarget)
        );
    }

    #[test]
    fn unresolvable_colon_token_is_not_treated_as_a_placeholder() {
        // `/docs/:8080/index` under a rule whose source declares no
        // placeholder and has no splat: `substitute_target` leaves it
        // literal, so it IS one concrete path.
        let p = plan(None);
        let t = p
            .targets
            .iter()
            .find(|t| t.raw_target == "/docs/:8080/index")
            .expect("literal-colon target enumerated");
        assert_eq!(t.request_path, "/docs/:8080/index");
    }

    // --- row: external / protocol-relative / non-absolute -----------------

    #[test]
    fn absolute_url_target_never_reaches_this_module() {
        // The parser already drops a `200` rule with a `scheme:` target, so
        // it is absent from BOTH lists. The defense-in-depth check in this
        // module is exercised directly below.
        let r = Redirects::parse("/ext https://example.com/x 200\n");
        let p = prewarm_rewrite_targets(&r, None);
        assert_eq!(p.targets, vec![]);
        assert_eq!(p.skipped, vec![]);
        assert!(is_external_or_protocol_relative("https://example.com/x"));
    }

    #[test]
    fn protocol_relative_target_is_skipped() {
        // Parse does NOT catch this one — it starts with `/`, so it never
        // looks like a `scheme:`. Without this module's check it would
        // normalize to the nonsense lookup path `example.com/evil`.
        let p = plan(None);
        assert_eq!(
            skip_reason(&p, "//example.com/evil"),
            Some(PrewarmSkipReason::ExternalTarget)
        );
        assert!(!request_paths(&p).iter().any(|r| r.contains("example.com")));
    }

    #[test]
    fn non_absolute_target_is_skipped() {
        let p = plan(None);
        assert_eq!(
            skip_reason(&p, "relative/path"),
            Some(PrewarmSkipReason::NotAbsolutePath)
        );
    }

    // --- row: duplicates --------------------------------------------------

    #[test]
    fn duplicate_targets_keep_the_first_and_report_the_rest() {
        let p = plan(None);
        assert_eq!(
            request_paths(&p)
                .iter()
                .filter(|r| *r == "/shared-target")
                .count(),
            1
        );
        assert_eq!(
            skip_reason(&p, "/shared-target"),
            Some(PrewarmSkipReason::DuplicateOfEarlierTarget)
        );
    }

    #[test]
    fn output_is_deterministic_and_in_file_order() {
        assert_eq!(plan(None), plan(None));
        let order = request_paths(&plan(None));
        let target_idx = order.iter().position(|r| r == "/target").unwrap();
        let dup_idx = order.iter().position(|r| r == "/shared-target").unwrap();
        assert!(target_idx < dup_idx, "file order preserved: {order:?}");
    }

    // --- row: base-path prefix --------------------------------------------

    #[test]
    fn base_prefixed_target_is_expressed_prefix_stripped() {
        let r = Redirects::parse("/pj/site/alias /pj/site/rewritten/ 200\n");
        let p = prewarm_rewrite_targets(&r, Some("/pj/site"));
        assert_eq!(request_paths(&p), vec!["/rewritten/"]);
        // The waterfall collapses the trailing slash into the same keys a
        // slashless request produces.
        assert_eq!(
            p.targets[0].lookup_keys,
            vec!["/rewritten", "/rewritten/index.html", "/rewritten/"]
        );
    }

    #[test]
    fn base_prefixed_root_target_becomes_the_root() {
        let r = Redirects::parse("/pj/site/alias /pj/site/ 200\n");
        let p = prewarm_rewrite_targets(&r, Some("/pj/site"));
        assert_eq!(request_paths(&p), vec!["/"]);
        assert_eq!(p.targets[0].lookup_keys, vec!["/", "/index.html"]);
    }

    #[test]
    fn target_missing_the_base_prefix_passes_through_like_production() {
        // Not a supported authoring shape, but the decision is explicit:
        // mirror `strip_prefix_from_path`, which leaves a non-prefixed path
        // alone. Pre-warm and serve therefore still agree on the key.
        let r = Redirects::parse("/pj/site/alias /elsewhere 200\n");
        let p = prewarm_rewrite_targets(&r, Some("/pj/site"));
        assert_eq!(
            request_paths(&p),
            vec![format!(
                "/{}",
                waterfall_trimmed_for_rewrite_target("/elsewhere", Some("/pj/site"))
            )]
        );
        assert_eq!(request_paths(&p), vec!["/elsewhere"]);
    }

    #[test]
    fn boundary_lookalike_prefix_is_not_stripped() {
        // `/pj/sitemap` is NOT prefixed by `/pj/site` — same strict boundary
        // rule the request path uses.
        let r = Redirects::parse("/pj/site/alias /pj/sitemap 200\n");
        let p = prewarm_rewrite_targets(&r, Some("/pj/site"));
        assert_eq!(request_paths(&p), vec!["/pj/sitemap"]);
    }

    // --- purity -----------------------------------------------------------

    #[test]
    fn empty_ruleset_yields_an_empty_plan() {
        let p = prewarm_rewrite_targets(&Redirects::default(), None);
        assert_eq!(p, PrewarmPlan::default());
        assert!(p.targets.is_empty() && p.skipped.is_empty());
    }
}
