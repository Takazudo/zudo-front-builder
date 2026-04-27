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
fn cache_clear_drops_all_entries() {
    let cache = MdxModuleCache::new();
    compile_mdx_to_jsx_module_cached("a\n", &fixture_path("a"), Some(&cache), None).unwrap();
    compile_mdx_to_jsx_module_cached("b\n", &fixture_path("b"), Some(&cache), None).unwrap();
    assert_eq!(cache.len(), 2);
    cache.clear();
    assert!(cache.is_empty());
}
