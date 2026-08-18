//! Cross-language parity test: asserts that
//! `zfb_types::render_region::render_region_marker` produces the exact
//! output recorded in the shared fixture at
//! `tests/fixtures/render-region-marker-parity.json`.
//!
//! The SAME fixture is consumed by `crates/zfb/src/commands/render_artifact.rs`'s
//! `parse_marker` round-trip test and by the TS vitest suite in
//! `packages/zfb/src/__tests__/render-region-marker-cases.ts` — any drift
//! between a producer and the matcher is caught by one side or the other
//! failing.

use std::path::PathBuf;

use serde::Deserialize;
use zfb_types::render_region::{render_region_marker, RenderRegionEdge};

// ---------------------------------------------------------------------------
// Fixture schema
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    start: String,
    end: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_fixture() -> Fixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("render-region-marker-parity.json");
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {path:?}: {e}"));
    serde_json::from_str(&data).unwrap_or_else(|e| panic!("cannot parse fixture {path:?}: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn render_region_marker_parity() {
    let fixture = load_fixture();
    for case in &fixture.cases {
        let start = render_region_marker(RenderRegionEdge::Start, &case.id);
        assert_eq!(
            start, case.start,
            "render_region_marker(Start, {:?}): expected {:?}, got {:?}",
            case.id, case.start, start
        );

        let end = render_region_marker(RenderRegionEdge::End, &case.id);
        assert_eq!(
            end, case.end,
            "render_region_marker(End, {:?}): expected {:?}, got {:?}",
            case.id, case.end, end
        );
    }
}
