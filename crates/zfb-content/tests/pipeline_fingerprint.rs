//! Integration tests for the pipeline config fingerprint (zfb#905).
//!
//! The fingerprint joins the MDX compile-cache key, so its correctness
//! bar is: **every config knob that can change emitted JSX for the same
//! input must change the fingerprint** (a missed knob silently serves
//! stale JSX), and pipelines whose visitor chain cannot be derived from
//! config (manual mutation, per-file state, filesystem-reading feature
//! plugins) must have NO fingerprint at all.

use std::collections::HashSet;

use serde_json::json;
use zfb_content::pipeline::{HastNode, HastVisitor, Pipeline, ResolvedGfmConstructs};
use zfb_content::{ExternalLinksConfig, HeadingIdStrategy, MarkdownFeaturesConfig, TocConfig};

fn features(value: serde_json::Value) -> MarkdownFeaturesConfig {
    serde_json::from_value(value).expect("valid features config")
}

fn full_config(
    theme: Option<&str>,
    gfm: ResolvedGfmConstructs,
    cjk_friendly: bool,
    hard_breaks: bool,
    feats: Option<&MarkdownFeaturesConfig>,
) -> Pipeline {
    Pipeline::with_defaults_and_full_config(theme, gfm, None, cjk_friendly, hard_breaks, feats)
        .expect("no themes_dir — cannot fail")
}

fn baseline() -> Pipeline {
    full_config(None, ResolvedGfmConstructs::CONSERVATIVE, true, false, None)
}

#[test]
fn equal_configs_produce_equal_fingerprints() {
    // Two independently constructed pipelines from the same config MUST
    // agree — otherwise dev ticks (which rebuild the pipeline every
    // tick) would never hit the compile cache.
    let a = baseline().config_fingerprint().expect("fingerprinted");
    let b = baseline().config_fingerprint().expect("fingerprinted");
    assert_eq!(a, b);

    // Same for the legacy defaults chain and the bare constructor.
    assert_eq!(
        Pipeline::with_defaults().config_fingerprint(),
        Pipeline::with_defaults().config_fingerprint(),
    );
    assert_eq!(
        Pipeline::new().config_fingerprint(),
        Pipeline::new().config_fingerprint(),
    );

    // `features: None` and an explicitly-default features object build
    // the same visitor chain, so they MUST share a fingerprint (a split
    // here would just cost cache hits, but parity is cheap to pin).
    let default_features = MarkdownFeaturesConfig::default();
    assert_eq!(
        baseline().config_fingerprint(),
        full_config(
            None,
            ResolvedGfmConstructs::CONSERVATIVE,
            true,
            false,
            Some(&default_features),
        )
        .config_fingerprint(),
    );
}

#[test]
fn every_knob_changes_the_fingerprint() {
    // One variant per pipeline-visible knob. All fingerprints — the
    // baseline plus every single-knob flip — must be pairwise distinct.
    let mut variants: Vec<(&str, Pipeline)> = vec![
        ("baseline", baseline()),
        (
            "theme",
            full_config(
                Some("InspiredGitHub"),
                ResolvedGfmConstructs::CONSERVATIVE,
                true,
                false,
                None,
            ),
        ),
        (
            "gfm-all-on",
            full_config(None, ResolvedGfmConstructs::ALL_ON, true, false, None),
        ),
        (
            "gfm-all-off",
            full_config(None, ResolvedGfmConstructs::ALL_OFF, true, false, None),
        ),
        (
            "cjk-off",
            full_config(None, ResolvedGfmConstructs::CONSERVATIVE, false, false, None),
        ),
        (
            "hard-breaks",
            full_config(None, ResolvedGfmConstructs::CONSERVATIVE, true, true, None),
        ),
        (
            "feature-github-alerts",
            full_config(
                None,
                ResolvedGfmConstructs::CONSERVATIVE,
                true,
                false,
                Some(&features(json!({ "githubAlerts": true }))),
            ),
        ),
        (
            "feature-mermaid",
            full_config(
                None,
                ResolvedGfmConstructs::CONSERVATIVE,
                true,
                false,
                Some(&features(json!({ "mermaid": true }))),
            ),
        ),
        (
            "feature-directives",
            full_config(
                None,
                ResolvedGfmConstructs::CONSERVATIVE,
                true,
                false,
                Some(&features(json!({ "directives": { "note": "Note" } }))),
            ),
        ),
        (
            "feature-directives-other-map",
            full_config(
                None,
                ResolvedGfmConstructs::CONSERVATIVE,
                true,
                false,
                Some(&features(json!({ "directives": { "note": "Callout" } }))),
            ),
        ),
        (
            "feature-reading-time-wpm",
            full_config(
                None,
                ResolvedGfmConstructs::CONSERVATIVE,
                true,
                false,
                Some(&features(json!({ "readingTime": { "wpm": 250 } }))),
            ),
        ),
        (
            "feature-heading-ids-hierarchical",
            full_config(
                None,
                ResolvedGfmConstructs::CONSERVATIVE,
                true,
                false,
                Some(&features(json!({ "headingIds": { "strategy": "hierarchical" } }))),
            ),
        ),
        ("legacy-defaults-chain", Pipeline::with_defaults()),
        ("bare", Pipeline::new()),
    ];

    // Named config-driven mutators count as knobs too.
    let mut strip = baseline();
    strip.add_strip_md_ext();
    variants.push(("strip-md-ext", strip));

    let mut strip_no_slash = baseline();
    strip_no_slash.set_add_trailing_slash(false);
    strip_no_slash.add_strip_md_ext();
    variants.push(("strip-md-ext-no-trailing-slash", strip_no_slash));

    let mut toc = baseline();
    toc.add_toc(TocConfig::default());
    variants.push(("toc-default", toc));

    let mut toc_deep = baseline();
    toc_deep.add_toc(TocConfig {
        max_depth: 4,
        ..TocConfig::default()
    });
    variants.push(("toc-max-depth-4", toc_deep));

    let mut external = baseline();
    external.add_external_links(ExternalLinksConfig::default(), None);
    variants.push(("external-links-default", external));

    let mut external_site = baseline();
    external_site.add_external_links(
        ExternalLinksConfig::default(),
        Some("https://example.com"),
    );
    variants.push(("external-links-with-site", external_site));

    let mut strategy = baseline();
    strategy.set_heading_id_strategy(HeadingIdStrategy::Hierarchical);
    variants.push(("set-heading-id-strategy", strategy));

    let mut seen: HashSet<String> = HashSet::new();
    for (label, pipeline) in &variants {
        let fp = pipeline
            .config_fingerprint()
            .unwrap_or_else(|| panic!("variant `{label}` must be fingerprinted"));
        assert!(
            seen.insert(fp),
            "variant `{label}` aliases another variant's fingerprint — \
             this knob would silently serve stale JSX from the compile cache"
        );
    }
}

#[test]
fn named_mutator_call_order_does_not_split_the_fingerprint() {
    // The bundler calls add_toc → add_strip_md_ext → add_external_links;
    // the snapshot walker calls add_strip_md_ext → add_toc →
    // add_external_links. Both orders produce the IDENTICAL effective
    // visitor chain (add_toc inserts at a fixed position), and the two
    // surfaces must share compile-cache entries — so the fingerprint
    // must be call-order insensitive.
    let mut bundler_order = baseline();
    bundler_order.add_toc(TocConfig::default());
    bundler_order.add_strip_md_ext();
    bundler_order.add_external_links(ExternalLinksConfig::default(), Some("https://example.com"));

    let mut snapshot_order = baseline();
    snapshot_order.add_strip_md_ext();
    snapshot_order.add_toc(TocConfig::default());
    snapshot_order.add_external_links(ExternalLinksConfig::default(), Some("https://example.com"));

    assert_eq!(
        bundler_order.config_fingerprint(),
        snapshot_order.config_fingerprint(),
        "bundler and snapshot walker wire the same chain in different \
         call order — they must share one fingerprint"
    );
}

#[test]
fn manual_visitor_mutation_invalidates_the_fingerprint() {
    struct NoopHastVisitor;
    impl HastVisitor for NoopHastVisitor {
        fn visit(&mut self, _node: &mut HastNode) {}
    }

    let mut p = baseline();
    assert!(p.config_fingerprint().is_some());
    p.add_hast_visitor(Box::new(NoopHastVisitor));
    assert!(
        p.config_fingerprint().is_none(),
        "add_hast_visitor must invalidate — a Box<dyn HastVisitor> cannot be keyed"
    );
}

#[test]
fn resolve_links_invalidates_the_fingerprint() {
    // ResolveLinksPlugin output depends on the per-file source_dir (set
    // between compiles) and drains broken-link diagnostics — both
    // incompatible with input-keyed caching.
    let mut p = baseline();
    assert!(p.config_fingerprint().is_some());
    p.add_resolve_links(std::collections::HashMap::new());
    assert!(p.config_fingerprint().is_none());
}

#[test]
fn filesystem_reading_features_invalidate_the_fingerprint() {
    // These plugins read OTHER files at compile time, so their output
    // cannot be keyed on the input string — a cached entry could go
    // stale when the referenced file changes between dev ticks.
    for (label, value) in [
        ("transclude", json!({ "transclude": {} })),
        ("imageDimensions", json!({ "imageDimensions": {} })),
        ("linkValidation", json!({ "linkValidation": {} })),
    ] {
        let feats = features(value);
        let p = full_config(
            None,
            ResolvedGfmConstructs::CONSERVATIVE,
            true,
            false,
            Some(&feats),
        );
        assert!(
            p.config_fingerprint().is_none(),
            "features.{label} reads the filesystem — the pipeline must be uncacheable"
        );
    }
}

#[test]
fn themes_dir_contents_are_part_of_the_fingerprint() {
    // A minimal valid `.tmTheme` plist — enough for syntect's ThemeSet
    // parser. The fingerprint hashes the file BYTES, so editing the
    // theme between two pipeline constructions must change the
    // fingerprint (stale-highlighting guard), while reconstructing with
    // an untouched dir must not.
    // Mirrors MINIMAL_TMTHEME in `src/syntect_highlight.rs` — the
    // smallest plist syntect's ThemeSet parser accepts.
    fn theme_plist(background: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
    "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>name</key>
    <string>FP Test Theme</string>
    <key>settings</key>
    <array>
        <dict>
            <key>settings</key>
            <dict>
                <key>background</key>
                <string>{background}</string>
                <key>foreground</key>
                <string>#d4d4d4</string>
            </dict>
        </dict>
    </array>
    <key>uuid</key>
    <string>aaaaaaaa-0000-0000-0000-000000000905</string>
</dict>
</plist>"#
        )
    }
    let theme_a = theme_plist("#1e1e1e");
    let theme_b = theme_plist("#2e2e2e");

    let dir = tempfile::tempdir().expect("tempdir");
    let theme_path = dir.path().join("custom.tmTheme");

    std::fs::write(&theme_path, &theme_a).expect("write theme A");
    let build = || {
        Pipeline::with_defaults_and_theme_and_gfm_and_themes_dir(
            None,
            ResolvedGfmConstructs::CONSERVATIVE,
            dir.path(),
            true,
        )
        .expect("themes dir loads")
    };
    let fp_a1 = build().config_fingerprint().expect("fingerprinted");
    let fp_a2 = build().config_fingerprint().expect("fingerprinted");
    assert_eq!(fp_a1, fp_a2, "unchanged themes dir → stable fingerprint");

    std::fs::write(&theme_path, &theme_b).expect("write theme B");
    let fp_b = build().config_fingerprint().expect("fingerprinted");
    assert_ne!(
        fp_a1, fp_b,
        "edited .tmTheme bytes must change the fingerprint — otherwise \
         the compile cache would serve JSX highlighted with the old theme"
    );

    // And a themes_dir-less pipeline must not collide with either.
    let fp_none = Pipeline::with_defaults().config_fingerprint().expect("fingerprinted");
    assert_ne!(fp_none, fp_a1);
    assert_ne!(fp_none, fp_b);
}
