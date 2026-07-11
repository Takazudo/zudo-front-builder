//! Confirm sub (zfb#1535), check 6, part 1 of 2: classifier <-> stylesheet
//! role-list parity.
//!
//! The Highlight Tokens epic's role taxonomy is authored in THREE places
//! that must never drift apart:
//!
//! 1. `zfb_content::hi_roles::HiRole::ALL` (zfb#1529) — the scope->role
//!    classifier's fixed 18-role list, keyed by [`HiRole::short_name`].
//! 2. `zfb::config::CODE_HIGHLIGHT_ROLES` (zfb#1530) — the config-side
//!    validation copy, keyed by full role name (`Debug` lowercased).
//! 3. `zfb_css::default_hi_css()` (zfb#1531/#1533) — the shipped
//!    `--zfb-hi-*` token stylesheet, keyed by [`HiRole::short_name`].
//!
//! `zfb-css` cannot see `zfb::config::CODE_HIGHLIGHT_ROLES` (that crate
//! depends on `zfb-css`, not the other way around — pulling it in here
//! would be a cycle), so THIS test only checks leg 1 <-> leg 3: every
//! `HiRole` in [`HiRole::ALL`], in taxonomy order, has BOTH a declared
//! `--zfb-hi-<suffix>` custom property AND a `.hi-<suffix>` role
//! selector in [`zfb_css::default_hi_css`]'s output. The full three-way
//! parity assertion (all 3 legs, including the config-side list) lives
//! in `crates/zfb/src/commands/build.rs` (that crate depends on both
//! `zfb-content` and `zfb-css`) — see that file's
//! `hi_role_taxonomy_parity_across_classifier_config_and_stylesheet` test.
//! The three-way test lives in the `zfb` crate, whose test binaries link
//! the embedded V8 host, so it runs in CI (health.yml's `cargo nextest
//! run --workspace`); this `zfb-css` partial is V8-free and runs in the
//! everyday `cargo test -p zfb-css` loop, so the classifier<->stylesheet
//! legs are guarded on every local run too.

use zfb_content::hi_roles::HiRole;

#[test]
fn every_hi_role_short_name_has_a_declared_custom_property_and_selector() {
    let css = zfb_css::default_hi_css();

    assert_eq!(
        HiRole::ALL.len(),
        18,
        "classifier taxonomy must stay fixed at 18 roles (zfb#1529)"
    );

    for role in HiRole::ALL {
        let suffix = role.short_name();
        let prop_decl = format!("--zfb-hi-{suffix}:");
        assert!(
            css.contains(&prop_decl),
            "default_hi_css() must declare `{prop_decl}` for classifier role \
             {role:?} (short_name {suffix:?}); stylesheet:\n{css}"
        );
        let selector = format!(".hi-{suffix} {{");
        assert!(
            css.contains(&selector),
            "default_hi_css() must declare a `{selector}` role selector for \
             classifier role {role:?} (short_name {suffix:?}); stylesheet:\n{css}"
        );
    }
}

/// The reverse direction: every `--zfb-hi-<suffix>` custom property AND
/// every `.hi-<suffix>` role selector declared in the stylesheet must
/// correspond to a real classifier short name — no orphaned CSS that the
/// classifier can never emit. Guards against the stylesheet silently
/// growing a role the classifier doesn't know about (or a stale one left
/// behind after a taxonomy edit).
///
/// `--zfb-hi-fg` / `--zfb-hi-bg` are the two non-role base tokens (root
/// foreground/background) and are explicitly excluded — they are not
/// role short names and never will be (see the stylesheet's own header
/// comment on `fg`/`bg`).
#[test]
fn stylesheet_declares_no_orphaned_role_properties() {
    let css = zfb_css::default_hi_css();
    let known_suffixes: std::collections::HashSet<&str> =
        HiRole::ALL.iter().map(|r| r.short_name()).collect();

    for line in css.lines() {
        let trimmed = line.trim();

        // (a) `--zfb-hi-<suffix>:` custom-property declarations.
        if let Some(rest) = trimmed.strip_prefix("--zfb-hi-") {
            if let Some(suffix) = rest.split(':').next() {
                // `-bg` suffixed variants (`ins-bg`, `del-bg`) are
                // diff-background companions to a real role property, not
                // standalone roles.
                let base_suffix = suffix.strip_suffix("-bg").unwrap_or(suffix);
                if base_suffix != "fg" && base_suffix != "bg" {
                    assert!(
                        known_suffixes.contains(base_suffix),
                        "stylesheet declares --zfb-hi-{suffix} but no HiRole short_name() \
                         produces {base_suffix:?}; known suffixes: {known_suffixes:?}"
                    );
                }
            }
            continue;
        }

        // (b) `.hi-<suffix> {` role-selector rules. A stale `.hi-orphan {`
        // left behind after a taxonomy edit must be caught here too — the
        // property scan in (a) alone would miss a selector with no matching
        // custom property. The first whitespace-delimited token after the
        // prefix is the selector name (`.hi-esc` in `.hi-esc {`); a
        // brace-hugging form (`.hi-esc{`) has its trailing `{` trimmed.
        if let Some(rest) = trimmed.strip_prefix(".hi-") {
            let token = rest.split_whitespace().next().unwrap_or(rest);
            let suffix = token.strip_suffix('{').unwrap_or(token);
            // `.hi-root` is the container class (root fg/bg), not a role.
            if suffix.is_empty() || suffix == "root" {
                continue;
            }
            assert!(
                known_suffixes.contains(suffix),
                "stylesheet declares a .hi-{suffix} selector but no HiRole \
                 short_name() produces {suffix:?}; known suffixes: {known_suffixes:?}"
            );
        }
    }
}
