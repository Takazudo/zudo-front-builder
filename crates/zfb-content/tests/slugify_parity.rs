//! Cross-language parity test: asserts that the Rust `slugify` and
//! `SlugAllocator` in `zfb_content::plugins::heading_links` produce the
//! exact output recorded in the shared fixture at
//! `tests/fixtures/slugify-parity.json`.
//!
//! The SAME fixture is consumed by the JS vitest suite in
//! `packages/zfb/src/__tests__/slugify.test.ts` — any drift between the
//! Rust implementation and the TypeScript port is caught by one side or
//! the other failing.

use std::path::PathBuf;

use serde::Deserialize;
use zfb_content::plugins::heading_links::{slugify, SlugAllocator};
use zfb_md_ast::HeadingIdStrategy;

// ---------------------------------------------------------------------------
// Fixture schema
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Fixture {
    slugify: Vec<SlugifyCase>,
    allocate: Vec<AllocateCase>,
}

#[derive(Deserialize)]
struct SlugifyCase {
    input: String,
    expected: String,
}

#[derive(Deserialize)]
struct AllocateCase {
    strategy: String,
    headings: Vec<Heading>,
    expected: Vec<String>,
}

#[derive(Deserialize)]
struct Heading {
    depth: u8,
    text: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_fixture() -> Fixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("slugify-parity.json");
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {path:?}: {e}"));
    serde_json::from_str(&data).unwrap_or_else(|e| panic!("cannot parse fixture {path:?}: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn slugify_parity() {
    let fixture = load_fixture();
    for case in &fixture.slugify {
        let got = slugify(&case.input);
        assert_eq!(
            got, case.expected,
            "slugify({:?}): expected {:?}, got {:?}",
            case.input, case.expected, got
        );
    }
}

#[test]
fn allocate_parity() {
    let fixture = load_fixture();
    for case in &fixture.allocate {
        let strategy = match case.strategy.as_str() {
            "flat" => HeadingIdStrategy::Flat,
            "hierarchical" => HeadingIdStrategy::Hierarchical,
            other => panic!("unknown strategy {other:?} in fixture"),
        };
        let mut alloc = SlugAllocator::new(strategy);
        let mut results = Vec::new();
        for heading in &case.headings {
            let base = slugify(&heading.text);
            results.push(alloc.allocate(heading.depth, &base));
        }
        assert_eq!(
            results, case.expected,
            "allocate (strategy={:?}): expected {:?}, got {:?}",
            case.strategy, case.expected, results
        );
    }
}
