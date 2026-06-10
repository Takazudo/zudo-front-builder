//! Integration tests for the pipeline config fingerprint (zfb#905).
//!
//! The fingerprint joins the MDX compile-cache key, so its correctness
//! bar is: **every config knob that can change emitted JSX for the same
//! input must change the fingerprint** (a missed knob silently serves
//! stale JSX), and pipelines whose visitor chain cannot be derived from
//! config (manual mutation) must have NO fingerprint at all. Per-file
//! resolve-links state is keyed by the compile cache itself (zfb#939)
//! rather than invalidating here; filesystem-reading feature plugins
//! keep their fingerprint since zfb#944 — their per-file reads are
//! validated through the read-recorder dependency manifest instead.

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
            full_config(
                None,
                ResolvedGfmConstructs::CONSERVATIVE,
                false,
                false,
                None,
            ),
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
                Some(&features(
                    json!({ "headingIds": { "strategy": "hierarchical" } }),
                )),
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
    external_site.add_external_links(ExternalLinksConfig::default(), Some("https://example.com"));
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
fn config_derived_mutators_do_not_invalidate_the_fingerprint() {
    // Invalidation rule (zfb#913): internal pushes that derive purely from
    // already-fingerprinted config MUST NOT invalidate — the named config
    // mutators extend the fingerprint with a segment instead, keeping the
    // pipeline cacheable.
    let mut p = baseline();
    p.add_toc(TocConfig::default());
    p.add_strip_md_ext();
    p.add_external_links(ExternalLinksConfig::default(), None);
    p.set_heading_id_strategy(HeadingIdStrategy::Hierarchical);
    assert!(
        p.config_fingerprint().is_some(),
        "named config-driven mutators must keep the pipeline cacheable"
    );
}

#[test]
fn manual_feature_registration_invalidates_the_fingerprint() {
    // Invalidation rule (zfb#913): manual external pushes MUST invalidate.
    // The public registration helpers are external surface — a
    // post-construction call wires visitors the construction-time
    // descriptor knows nothing about, so even an empty feature set drops
    // cacheability.
    let empty = MarkdownFeaturesConfig::default();

    let mut p = baseline();
    assert!(p.config_fingerprint().is_some());
    zfb_content::pipeline::register_features(&mut p, &empty);
    assert!(
        p.config_fingerprint().is_none(),
        "manual register_features must invalidate, even with an empty feature set"
    );

    let mut p = baseline();
    zfb_content::pipeline::register_post_syntect_features(&mut p, &empty);
    assert!(
        p.config_fingerprint().is_none(),
        "manual register_post_syntect_features must invalidate, even with an empty feature set"
    );
}

#[test]
fn canonical_features_json_covers_every_field() {
    // Companion to the compile-time drift guard
    // (`assert_features_fingerprint_covers_every_field` in pipeline.rs).
    // The guard forces a new `MarkdownFeaturesConfig` field to be bound at
    // the fingerprint site; THIS test pins that every field actually
    // reaches the canonical features JSON — a `#[serde(skip)]` /
    // `skip_serializing_if` attribute would drop the field from the
    // descriptor and silently alias configs that differ only in it.
    let v = serde_json::to_value(MarkdownFeaturesConfig::default())
        .expect("features config serializes");
    let keys: std::collections::BTreeSet<String> = v
        .as_object()
        .expect("features config serializes to a JSON object")
        .keys()
        .cloned()
        .collect();
    let expected: std::collections::BTreeSet<String> = [
        "codeEnrichment",
        "codeTabs",
        "directives",
        "githubAlerts",
        "githubAutolinks",
        "headingIds",
        "headingMarkerToc",
        "imageDimensions",
        "linkValidation",
        "mermaid",
        "readingTime",
        "ruby",
        "tocExport",
        "transclude",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(
        keys, expected,
        "every MarkdownFeaturesConfig field must appear in the canonical \
         features JSON (and in this list) — update the drift-guard \
         destructure in pipeline.rs AND this list when adding a field; \
         never serde-skip a field on these structs"
    );
}

#[test]
fn resolve_links_keeps_the_pipeline_cacheable() {
    // zfb#939 contract (replaces the pre-#939 "resolve-links
    // invalidates" pin): the plugin resolves against a prebuilt
    // source_map — it never reads the filesystem at compile time — so
    // wiring it EXTENDS the fingerprint with a digest of the map. The
    // remaining per-file state (source_dir, broken-link diagnostics) is
    // handled by the compile cache itself (per-call key context +
    // diagnostics replay).
    let mut p = baseline();
    let without = p.config_fingerprint().expect("baseline is fingerprinted");
    p.add_resolve_links(std::collections::HashMap::new());
    let with = p
        .config_fingerprint()
        .expect("resolveMarkdownLinks wired => fingerprint must stay Some (zfb#939)");
    assert_ne!(
        without, with,
        "wiring resolve-links changes emitted JSX for md links — the          fingerprint must move"
    );
}

#[test]
fn resolve_links_source_map_content_changes_the_fingerprint() {
    // The source map is rebuilt from the content tree each dev tick; a
    // content add/remove/rename must change the digest so stale link
    // resolutions can never be served from the cache.
    fn fp(entries: &[(&str, &str)]) -> String {
        let map: std::collections::HashMap<std::path::PathBuf, String> = entries
            .iter()
            .map(|(p, u)| (std::path::PathBuf::from(p), (*u).to_string()))
            .collect();
        let mut p = baseline();
        p.add_resolve_links(map);
        p.config_fingerprint().expect("fingerprinted")
    }

    let base = fp(&[("/c/docs/a.mdx", "/docs/a/"), ("/c/docs/b.mdx", "/docs/b/")]);

    // Same map, different construction order → identical fingerprint
    // (entries are sorted before digesting; HashMap order is random
    // anyway, so this pins determinism across rebuilds).
    let same = fp(&[("/c/docs/b.mdx", "/docs/b/"), ("/c/docs/a.mdx", "/docs/a/")]);
    assert_eq!(base, same, "equal maps must share one fingerprint");

    // File added.
    let added = fp(&[
        ("/c/docs/a.mdx", "/docs/a/"),
        ("/c/docs/b.mdx", "/docs/b/"),
        ("/c/docs/c.mdx", "/docs/c/"),
    ]);
    // File removed.
    let removed = fp(&[("/c/docs/a.mdx", "/docs/a/")]);
    // File renamed (path key changes, URL follows).
    let renamed = fp(&[
        ("/c/docs/a2.mdx", "/docs/a2/"),
        ("/c/docs/b.mdx", "/docs/b/"),
    ]);
    // URL remapped (same paths, different route).
    let remapped = fp(&[
        ("/c/docs/a.mdx", "/elsewhere/a/"),
        ("/c/docs/b.mdx", "/docs/b/"),
    ]);

    let all = [&base, &added, &removed, &renamed, &remapped];
    for (i, a) in all.iter().enumerate() {
        for b in all.iter().skip(i + 1) {
            assert_ne!(a, b, "distinct source maps must never share a fingerprint");
        }
    }
}

#[test]
fn resolve_links_source_map_digest_has_no_record_ambiguity() {
    // Length-delimited records: entry boundaries cannot be forged by
    // moving characters between a path and its URL (the classic
    // `key=value\n` concatenation ambiguity).
    fn fp(entries: &[(&str, &str)]) -> String {
        let map: std::collections::HashMap<std::path::PathBuf, String> = entries
            .iter()
            .map(|(p, u)| (std::path::PathBuf::from(p), (*u).to_string()))
            .collect();
        let mut p = baseline();
        p.add_resolve_links(map);
        p.config_fingerprint().expect("fingerprinted")
    }
    assert_ne!(
        fp(&[("/a/bc.mdx", "/x/")]),
        fp(&[("/a/b", "c.mdx/x/")]),
        "shifting bytes across the path/URL boundary must change the digest"
    );
}

#[test]
fn resolve_links_path_spelling_does_not_split_the_fingerprint() {
    // The digest normalises map keys with the shared lexical helper,
    // whose canonical form mirrors `Path` equality — i.e. exactly the
    // runtime `HashMap<PathBuf, _>` lookup semantics. Spellings the
    // lookup merges must digest identically (a split costs every cache
    // hit), while spellings the lookup DISTINGUISHES (`..`) must stay
    // distinct — collapsing them would let two maps that resolve links
    // differently share a fingerprint and serve a stale hit.
    fn fp(path: &str) -> String {
        let mut map = std::collections::HashMap::new();
        map.insert(std::path::PathBuf::from(path), "/docs/a/".to_string());
        let mut p = baseline();
        p.add_resolve_links(map);
        p.config_fingerprint().expect("fingerprinted")
    }
    let canonical = fp("/c/docs/a.mdx");
    assert_eq!(canonical, fp("/c/./docs/a.mdx"));
    assert_eq!(canonical, fp("/c//docs/a.mdx"));
    assert_eq!(canonical, fp("/c/docs/a.mdx/."));
    // `..` keys are runtime-distinct (Path equality keeps them), so the
    // digest must keep them distinct too.
    assert_ne!(canonical, fp("/c/x/../docs/a.mdx"));
    assert_ne!(canonical, fp("/c/docs2/a.mdx"));
}

#[test]
fn filesystem_reading_features_keep_the_pipeline_cacheable() {
    // zfb#944 contract (replaces the pre-#944 "filesystem features
    // invalidate" pin): these plugins read OTHER files at compile time,
    // but every read is reported through the per-pipeline ReadRecorder
    // the constructor now wires, and the compile cache validates the
    // recorded dependency manifest before serving any hit — so the
    // pipeline keeps a config fingerprint. Each feature must still
    // SPLIT the fingerprint (the canonical features JSON covers it),
    // and the recorder must actually be attached.
    let baseline_fp = baseline().config_fingerprint().expect("fingerprinted");
    let mut fps = vec![baseline_fp];
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
        let fp = p
            .config_fingerprint()
            .unwrap_or_else(|| panic!("features.{label} must keep a config fingerprint (zfb#944)"));
        assert!(
            p.read_recorder().is_some(),
            "features.{label} must wire a read-recorder so its reads join \
             the dependency manifest (zfb#944)"
        );
        fps.push(fp);
    }
    let distinct: HashSet<&String> = fps.iter().collect();
    assert_eq!(
        distinct.len(),
        fps.len(),
        "baseline + each filesystem feature must produce pairwise distinct \
         fingerprints: {fps:?}"
    );
}

#[test]
fn non_filesystem_features_do_not_wire_a_read_recorder() {
    // The recorder (and the per-source-dir cache-key segment that comes
    // with it) is reserved for pipelines whose plugins actually read
    // other files — a plain feature set keeps the pre-#942 key shape.
    let p = baseline();
    assert!(p.read_recorder().is_none());
    let feats = features(json!({ "githubAlerts": true }));
    let p = full_config(
        None,
        ResolvedGfmConstructs::CONSERVATIVE,
        true,
        false,
        Some(&feats),
    );
    assert!(p.read_recorder().is_none());
}

#[test]
fn build_context_roots_join_the_fingerprint() {
    // zfb#944: the BuildContext roots shape emitted JSX (containment
    // checks, `/`-absolute image resolution), so arming them must split
    // the fingerprint — and equal roots must agree across constructions.
    let unarmed = baseline().config_fingerprint().expect("fingerprinted");

    let mut a = baseline();
    a.set_build_context_roots("/proj".into(), "/proj/public".into());
    let a = a
        .config_fingerprint()
        .expect("armed pipeline stays fingerprinted");

    let mut b = baseline();
    b.set_build_context_roots("/proj".into(), "/proj/public".into());
    let b = b.config_fingerprint().expect("fingerprinted");
    assert_eq!(a, b, "equal roots must share one fingerprint");
    assert_ne!(
        a, unarmed,
        "arming context roots must split the fingerprint"
    );

    let mut c = baseline();
    c.set_build_context_roots("/other".into(), "/other/public".into());
    let c = c.config_fingerprint().expect("fingerprinted");
    assert_ne!(a, c, "different roots must split the fingerprint");

    // A second call REPLACES the roots and the segment — no stale
    // segment may linger (mirrors the add_resolve_links contract).
    let mut d = baseline();
    d.set_build_context_roots("/other".into(), "/other/public".into());
    d.set_build_context_roots("/proj".into(), "/proj/public".into());
    let d = d.config_fingerprint().expect("fingerprinted");
    assert_eq!(a, d, "re-arming must replace the previous roots segment");
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
    let fp_none = Pipeline::with_defaults()
        .config_fingerprint()
        .expect("fingerprinted");
    assert_ne!(fp_none, fp_a1);
    assert_ne!(fp_none, fp_b);
}
