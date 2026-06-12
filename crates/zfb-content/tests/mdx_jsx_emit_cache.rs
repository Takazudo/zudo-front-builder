//! Integration tests for Sub 2: hash-based caching + module ID
//! derivation in `mdx_jsx_emit`.
//!
//! Covers (per #29 acceptance criteria):
//! - Identical input bodies produce the same `content_hash` (deterministic).
//! - Distinct input bodies produce distinct hashes (cache safety).
//! - Specifier `mdx://<collection>/<slug>#<hash8>` round-trips through
//!   the parse helper.
//! - The cache is opt-in: `None` recompiles, `Some(&cache)` dedupes.

use std::path::PathBuf;

use zfb_content::pipeline::Pipeline;
use zfb_content::{
    compile_mdx_to_jsx_module, compile_mdx_to_jsx_module_cached, parse_mdx_specifier,
    MdxModuleCache, MdxModuleSpecifier, SpecifierError,
};

fn fixture_path(slug: &str) -> PathBuf {
    // Use a synthetic absolute path so the helper extracts a stable
    // collection ("blog") and slug from the conventional layout. We
    // never actually read this file — `compile_mdx_to_jsx_module` only
    // consults the path's parent + stem.
    PathBuf::from(format!("/virtual/blog/{slug}.mdx"))
}

#[test]
fn deterministic_hashing_same_input_same_hash() {
    let src = "# hello\n\nworld\n";
    let a = compile_mdx_to_jsx_module(src, &fixture_path("post")).unwrap();
    let b = compile_mdx_to_jsx_module(src, &fixture_path("post")).unwrap();

    assert_eq!(a.content_hash, b.content_hash, "hash must be deterministic");
    assert_eq!(a.jsx_source, b.jsx_source, "JSX must be deterministic");
    assert_eq!(a.specifier, b.specifier, "specifier must be deterministic");

    // Hash is exactly 8 lowercase hex chars.
    assert_eq!(a.content_hash.len(), 8);
    assert!(a
        .content_hash
        .chars()
        .all(|c| c.is_ascii_hexdigit() && (!c.is_ascii_alphabetic() || c.is_ascii_lowercase())));
}

#[test]
fn distinct_inputs_yield_distinct_hashes() {
    let a = compile_mdx_to_jsx_module("# alpha\n", &fixture_path("a")).unwrap();
    let b = compile_mdx_to_jsx_module("# beta\n", &fixture_path("b")).unwrap();

    assert_ne!(
        a.content_hash, b.content_hash,
        "different bodies must hash differently"
    );
    assert_ne!(a.specifier, b.specifier);
}

#[test]
fn specifier_format_matches_collection_and_slug() {
    let compiled = compile_mdx_to_jsx_module("hi\n", &fixture_path("intro")).unwrap();
    let expected_prefix = format!("mdx://blog/intro#{}", compiled.content_hash);
    assert_eq!(compiled.specifier, expected_prefix);
}

#[test]
fn parse_specifier_round_trip() {
    let original = MdxModuleSpecifier {
        collection: "blog".to_string(),
        slug: "first-post".to_string(),
        content_hash: "deadbeef".to_string(),
    };
    let url = original.to_url();
    assert_eq!(url, "mdx://blog/first-post#deadbeef");

    let parsed = parse_mdx_specifier(&url).expect("parse ok");
    assert_eq!(parsed, original);
    assert_eq!(parsed.to_url(), url);
}

#[test]
fn parse_specifier_round_trip_from_compile_output() {
    let compiled = compile_mdx_to_jsx_module("body\n", &fixture_path("entry-x")).unwrap();
    let parsed = parse_mdx_specifier(&compiled.specifier).expect("parse ok");
    assert_eq!(parsed.collection, "blog");
    assert_eq!(parsed.slug, "entry-x");
    assert_eq!(parsed.content_hash, compiled.content_hash);
    assert_eq!(parsed.to_url(), compiled.specifier);
}

#[test]
fn parse_specifier_rejects_bad_inputs() {
    assert!(matches!(
        parse_mdx_specifier("https://example/x/y#deadbeef"),
        Err(SpecifierError::BadScheme(_))
    ));
    assert!(matches!(
        parse_mdx_specifier("mdx://only-collection#deadbeef"),
        Err(SpecifierError::MissingPath(_))
    ));
    assert!(matches!(
        parse_mdx_specifier("mdx:///slug#deadbeef"),
        Err(SpecifierError::MissingPath(_))
    ));
    assert!(matches!(
        parse_mdx_specifier("mdx://blog/post"),
        Err(SpecifierError::MissingHash(_))
    ));
    // 7 hex chars: too short.
    assert!(matches!(
        parse_mdx_specifier("mdx://blog/post#1234567"),
        Err(SpecifierError::BadHash(_))
    ));
    // Uppercase hex: rejected (we emit lowercase only).
    assert!(matches!(
        parse_mdx_specifier("mdx://blog/post#DEADBEEF"),
        Err(SpecifierError::BadHash(_))
    ));
    // Non-hex char.
    assert!(matches!(
        parse_mdx_specifier("mdx://blog/post#zzzzzzzz"),
        Err(SpecifierError::BadHash(_))
    ));
}

#[test]
fn cache_opt_in_dedupes_identical_input() {
    let cache = MdxModuleCache::new();
    assert!(cache.is_empty());

    let src = "# cached\n\nbody\n";
    let path = fixture_path("cached-post");

    let first = compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), None).unwrap();
    assert_eq!(cache.len(), 1, "first call populates the cache");

    let second = compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), None).unwrap();
    assert_eq!(cache.len(), 1, "second call must hit (no new entry)");
    assert_eq!(first, second, "cached value must match the original");

    // Distinct input → distinct cache entry.
    let other = compile_mdx_to_jsx_module_cached("# other\n", &path, Some(&cache), None).unwrap();
    assert_eq!(cache.len(), 2);
    assert_ne!(other.content_hash, first.content_hash);
}

#[test]
fn cache_opt_out_recompiles_every_call() {
    // Without a cache, every call should be a fresh compilation. We
    // can't observe "freshness" directly, but we can confirm the API
    // shape: passing None doesn't error and returns a value identical
    // to a cached lookup.
    let src = "hello\n";
    let path = fixture_path("p1");

    let cache = MdxModuleCache::new();
    let cached_first = compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), None).unwrap();
    let uncached = compile_mdx_to_jsx_module_cached(src, &path, None, None).unwrap();
    let convenience = compile_mdx_to_jsx_module(src, &path).unwrap();

    assert_eq!(cached_first, uncached);
    assert_eq!(uncached, convenience);
}

#[test]
fn config_built_pipeline_uses_the_cache() {
    // zfb#905 contract (replaces the pre-#905 "pipeline bypasses the
    // cache" pin): a pipeline built from config carries a fingerprint
    // that joins the cache key, so `Some(&cache) + Some(&mut pipeline)`
    // caches — and a SECOND pipeline built from the SAME config hits
    // the entry the first one wrote.
    let cache = MdxModuleCache::new();
    assert!(cache.is_empty(), "precondition: empty cache");

    let src = "# heading\n\nbody\n";
    let path = fixture_path("with-pipeline");

    let mut pipeline = Pipeline::with_defaults();
    let first =
        compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut pipeline)).unwrap();
    assert_eq!(
        cache.len(),
        1,
        "config-built pipeline must populate the cache (zfb#905)"
    );

    let mut equally_configured = Pipeline::with_defaults();
    let second =
        compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut equally_configured))
            .unwrap();
    assert_eq!(
        cache.len(),
        1,
        "same config + same input must hit, not re-insert"
    );
    assert_eq!(first, second, "cached value must match the original");
}

#[test]
fn manually_mutated_pipeline_bypasses_cache() {
    // zfb#905 contract: appending a raw trait-object visitor after
    // construction invalidates the config fingerprint — the cache can
    // no longer tell this pipeline apart from any other manual wiring,
    // so the compile must bypass the cache entirely (never grow it,
    // never read from it).
    struct NoopHastVisitor;
    impl zfb_content::pipeline::HastVisitor for NoopHastVisitor {
        fn visit(&mut self, _node: &mut zfb_content::pipeline::HastNode) {}
    }

    let cache = MdxModuleCache::new();
    let mut pipeline = Pipeline::with_defaults();
    assert!(
        pipeline.config_fingerprint().is_some(),
        "precondition: config-built pipeline is fingerprinted"
    );
    pipeline.add_hast_visitor(Box::new(NoopHastVisitor));
    assert!(
        pipeline.config_fingerprint().is_none(),
        "manual mutation must invalidate the fingerprint"
    );

    let src = "# heading\n\nbody\n";
    let path = fixture_path("mutated-pipeline");

    let _ =
        compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut pipeline)).unwrap();
    assert_eq!(
        cache.len(),
        0,
        "mutated pipeline must NOT grow the cache (was {})",
        cache.len()
    );
    let _ =
        compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut pipeline)).unwrap();
    assert_eq!(cache.len(), 0, "second call must also bypass the cache");

    // Control: an un-mutated pipeline DOES grow the cache, guarding
    // against a degenerate "cache never grows" failure mode.
    let mut fresh = Pipeline::with_defaults();
    let _ = compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut fresh)).unwrap();
    assert_eq!(
        cache.len(),
        1,
        "control: a config-built pipeline must let the cache grow"
    );
}

#[test]
fn different_pipeline_configs_do_not_alias_one_entry() {
    // zfb#905 acceptance: same input compiled under two different
    // configs must produce two distinct cache entries with distinct
    // JSX — a shared entry would serve stale strikethrough markup to
    // whichever config compiled second.
    use zfb_content::pipeline::ResolvedGfmConstructs;

    let cache = MdxModuleCache::new();
    let src = "plain ~~gone~~ here\n";
    let path = fixture_path("gfm-split");

    let mut all_on = Pipeline::with_defaults_and_gfm(ResolvedGfmConstructs::ALL_ON);
    let on = compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut all_on)).unwrap();
    let mut all_off = Pipeline::with_defaults_and_gfm(ResolvedGfmConstructs::ALL_OFF);
    let off =
        compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut all_off)).unwrap();

    assert_eq!(cache.len(), 2, "two configs → two entries");
    assert_ne!(on.jsx_source, off.jsx_source, "two configs → two outputs");
}

#[test]
fn cache_clear_drops_all_entries() {
    let cache = MdxModuleCache::new();
    compile_mdx_to_jsx_module_cached("a\n", &fixture_path("a"), Some(&cache), None).unwrap();
    compile_mdx_to_jsx_module_cached("b\n", &fixture_path("b"), Some(&cache), None).unwrap();
    assert_eq!(cache.len(), 2);
    cache.clear();
    assert!(cache.is_empty());
}

// ---------------------------------------------------------------------------
// resolveMarkdownLinks caching (zfb#939)
//
// Pre-#939, wiring `ResolveLinksPlugin` invalidated the config
// fingerprint and every compile bypassed the cache. The plugin never
// reads the filesystem at compile time, so the map is digested into the
// fingerprint, the per-file `source_dir` joins the key as a per-call
// context segment, and broken-link diagnostics are stored with the
// entry and replayed on hits.
// ---------------------------------------------------------------------------

/// Build a default pipeline with `ResolveLinksPlugin` wired from the
/// given `(absolute path, url)` entries.
fn resolve_links_pipeline(entries: &[(&str, &str)]) -> Pipeline {
    let map: std::collections::HashMap<PathBuf, String> = entries
        .iter()
        .map(|(p, u)| (PathBuf::from(p), (*u).to_string()))
        .collect();
    let mut pipeline = Pipeline::with_defaults();
    pipeline.add_resolve_links(map);
    pipeline
}

#[test]
fn resolve_links_unchanged_file_recompile_is_a_cache_hit() {
    // zfb#939 acceptance: with resolveMarkdownLinks wired, recompiling
    // an unchanged file under an identically-configured pipeline (the
    // dev-tick shape: fresh pipeline per tick, same config + map) is a
    // cache hit, not a recompile-and-overwrite.
    let cache = MdxModuleCache::new();
    let src = "[good](./other.mdx)\n";
    let path = PathBuf::from("/content/docs/index.mdx");

    let mut p1 = resolve_links_pipeline(&[("/content/docs/other.mdx", "/docs/other/")]);
    p1.set_resolve_links_source_dir(PathBuf::from("/content/docs"));
    let first = compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut p1)).unwrap();
    assert_eq!(
        cache.len(),
        1,
        "resolve-links pipeline must populate the cache"
    );
    assert!(
        first.jsx_source.contains("/docs/other/"),
        "link must resolve via the source map; got: {}",
        first.jsx_source
    );

    let mut p2 = resolve_links_pipeline(&[("/content/docs/other.mdx", "/docs/other/")]);
    p2.set_resolve_links_source_dir(PathBuf::from("/content/docs"));
    let second = compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut p2)).unwrap();
    assert_eq!(
        cache.len(),
        1,
        "unchanged file + same config must hit, not re-insert"
    );
    assert_eq!(first, second, "cached value must match the original");
}

#[test]
fn resolve_links_broken_link_diagnostics_replay_on_cache_hit() {
    // zfb#939 acceptance (explicit): diagnostics are a side channel —
    // call sites compile, then drain `take_broken_links`. On a hit the
    // plugin never runs, so the cache must replay the stored vec or
    // the bundler's onBrokenLinks policy silently stops firing for
    // every cached file.
    let cache = MdxModuleCache::new();
    let src = "[good](./other.mdx)\n\n[broken](./missing.mdx)\n";
    let path = PathBuf::from("/content/docs/index.mdx");
    let map: &[(&str, &str)] = &[("/content/docs/other.mdx", "/docs/other/")];

    let mut p1 = resolve_links_pipeline(map);
    p1.set_resolve_links_source_dir(PathBuf::from("/content/docs"));
    let _ = compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut p1)).unwrap();
    let fresh_diags: Vec<String> = p1.take_broken_links().into_iter().map(|d| d.url).collect();
    assert_eq!(
        fresh_diags,
        vec!["./missing.mdx".to_string()],
        "precondition: fresh compile reports the broken link"
    );
    assert_eq!(cache.len(), 1);

    let mut p2 = resolve_links_pipeline(map);
    p2.set_resolve_links_source_dir(PathBuf::from("/content/docs"));
    let _ = compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut p2)).unwrap();
    assert_eq!(cache.len(), 1, "second compile must be a hit");
    let replayed: Vec<String> = p2.take_broken_links().into_iter().map(|d| d.url).collect();
    assert_eq!(
        replayed, fresh_diags,
        "cache hit must replay the same broken-link diagnostics a fresh compile produces"
    );
    assert!(
        p2.take_broken_links().is_empty(),
        "drain semantics unchanged: a second drain after the hit is empty"
    );
}

#[test]
fn resolve_links_source_map_change_invalidates_the_entry() {
    // zfb#939 acceptance: the map is rebuilt from the content tree each
    // tick — renaming/removing a file changes the digest, so the same
    // input recompiles under a new key instead of serving the stale
    // resolution.
    let cache = MdxModuleCache::new();
    let src = "[link](./other.mdx)\n";
    let path = PathBuf::from("/content/docs/index.mdx");

    let mut before = resolve_links_pipeline(&[("/content/docs/other.mdx", "/docs/other/")]);
    before.set_resolve_links_source_dir(PathBuf::from("/content/docs"));
    let resolved =
        compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut before)).unwrap();
    assert!(resolved.jsx_source.contains("/docs/other/"));
    assert!(before.take_broken_links().is_empty());
    assert_eq!(cache.len(), 1);

    // `other.mdx` renamed away: the link is now broken. A stale hit
    // would still emit `/docs/other/` and report nothing.
    let mut after = resolve_links_pipeline(&[("/content/docs/renamed.mdx", "/docs/renamed/")]);
    after.set_resolve_links_source_dir(PathBuf::from("/content/docs"));
    let broken =
        compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut after)).unwrap();
    assert_eq!(cache.len(), 2, "changed map => new key => fresh compile");
    assert!(
        !broken.jsx_source.contains("/docs/other/"),
        "stale resolution must not be served; got: {}",
        broken.jsx_source
    );
    let diags: Vec<String> = after
        .take_broken_links()
        .into_iter()
        .map(|d| d.url)
        .collect();
    assert_eq!(diags, vec!["./other.mdx".to_string()]);

    // Same file remapped to a new route: also a fresh compile with the
    // new URL.
    let mut remapped = resolve_links_pipeline(&[("/content/docs/other.mdx", "/v2/other/")]);
    remapped.set_resolve_links_source_dir(PathBuf::from("/content/docs"));
    let rerouted =
        compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut remapped)).unwrap();
    assert_eq!(cache.len(), 3);
    assert!(rerouted.jsx_source.contains("/v2/other/"));
}

#[test]
fn resolve_links_source_dir_separates_cache_entries() {
    // zfb#939 acceptance: the per-file source_dir changes how relative
    // links resolve, so two files with identical BODIES but different
    // dirs must never share an entry — a shared one would hand file B
    // file A's resolved URLs.
    let cache = MdxModuleCache::new();
    let src = "[link](./other.mdx)\n";
    let map: &[(&str, &str)] = &[
        ("/content/a/other.mdx", "/a-route/other/"),
        ("/content/b/other.mdx", "/b-route/other/"),
    ];

    let mut in_a = resolve_links_pipeline(map);
    in_a.set_resolve_links_source_dir(PathBuf::from("/content/a"));
    let from_a = compile_mdx_to_jsx_module_cached(
        src,
        &PathBuf::from("/content/a/index.mdx"),
        Some(&cache),
        Some(&mut in_a),
    )
    .unwrap();
    assert!(from_a.jsx_source.contains("/a-route/other/"));
    assert_eq!(cache.len(), 1);

    let mut in_b = resolve_links_pipeline(map);
    in_b.set_resolve_links_source_dir(PathBuf::from("/content/b"));
    let from_b = compile_mdx_to_jsx_module_cached(
        src,
        &PathBuf::from("/content/b/index.mdx"),
        Some(&cache),
        Some(&mut in_b),
    )
    .unwrap();
    assert_eq!(
        cache.len(),
        2,
        "different source_dir must compile into a separate entry"
    );
    assert!(
        from_b.jsx_source.contains("/b-route/other/"),
        "file in /content/b must get ITS dir's resolution, not /content/a's; got: {}",
        from_b.jsx_source
    );

    // Different SPELLING of an already-cached dir is the same key: a
    // re-walk that spells the dir with a redundant `./` must hit.
    let mut respelled = resolve_links_pipeline(map);
    respelled.set_resolve_links_source_dir(PathBuf::from("/content/./a/"));
    let hit = compile_mdx_to_jsx_module_cached(
        src,
        &PathBuf::from("/content/a/index.mdx"),
        Some(&cache),
        Some(&mut respelled),
    )
    .unwrap();
    assert_eq!(cache.len(), 2, "respelled dir must hit the existing entry");
    assert_eq!(hit, from_a);
}

#[test]
fn resolve_links_unset_source_dir_does_not_alias_a_set_one() {
    // With source_dir unset the plugin only performs direct map
    // lookups, so `./other.mdx` is broken; with the dir set it
    // resolves. The two compiles must key separately or whichever runs
    // second silently inherits the other's output.
    let cache = MdxModuleCache::new();
    let src = "[link](./other.mdx)\n";
    let path = PathBuf::from("/content/docs/index.mdx");
    let map: &[(&str, &str)] = &[("/content/docs/other.mdx", "/docs/other/")];

    let mut without_dir = resolve_links_pipeline(map);
    let unresolved =
        compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut without_dir)).unwrap();
    assert!(!unresolved.jsx_source.contains("/docs/other/"));
    assert_eq!(
        without_dir.take_broken_links().len(),
        1,
        "no source_dir => relative link cannot resolve"
    );

    let mut with_dir = resolve_links_pipeline(map);
    with_dir.set_resolve_links_source_dir(PathBuf::from("/content/docs"));
    let resolved =
        compile_mdx_to_jsx_module_cached(src, &path, Some(&cache), Some(&mut with_dir)).unwrap();
    assert_eq!(
        cache.len(),
        2,
        "unset vs set source_dir must not share a key"
    );
    assert!(resolved.jsx_source.contains("/docs/other/"));
    assert!(with_dir.take_broken_links().is_empty());
}

#[test]
fn resolve_links_index_and_non_index_in_same_dir_do_not_alias() {
    // zfb#1030: the URL-space fallback makes `section/index.mdx` and
    // `section/article.mdx` resolve the SAME body differently — the
    // dir-style `../other-article/` href misses in file space from both,
    // but only the non-index page falls back to its route directory.
    // Identical bodies + identical source_dir must therefore still key
    // separately, or the index page would inherit the article's
    // fallback-resolved URL (and vice versa).
    let cache = MdxModuleCache::new();
    let src = "[link](../other-article/)\n";
    let map: &[(&str, &str)] = &[(
        "/content/docs/section/other-article.mdx",
        "/docs/section/other-article/",
    )];

    let mut as_article = resolve_links_pipeline(map);
    as_article.set_resolve_links_source_file(PathBuf::from("/content/docs/section/article.mdx"));
    let from_article = compile_mdx_to_jsx_module_cached(
        src,
        &PathBuf::from("/content/docs/section/article.mdx"),
        Some(&cache),
        Some(&mut as_article),
    )
    .unwrap();
    assert!(
        from_article.jsx_source.contains("/docs/section/other-article/"),
        "non-index page must fallback-resolve the URL-space href; got: {}",
        from_article.jsx_source
    );
    assert_eq!(cache.len(), 1);

    let mut as_index = resolve_links_pipeline(map);
    as_index.set_resolve_links_source_file(PathBuf::from("/content/docs/section/index.mdx"));
    let from_index = compile_mdx_to_jsx_module_cached(
        src,
        &PathBuf::from("/content/docs/section/index.mdx"),
        Some(&cache),
        Some(&mut as_index),
    )
    .unwrap();
    assert_eq!(
        cache.len(),
        2,
        "index vs non-index file in one dir must compile into separate entries"
    );
    assert!(
        !from_index.jsx_source.contains("/docs/section/other-article/"),
        "index page must NOT inherit the article's fallback resolution; got: {}",
        from_index.jsx_source
    );
}
