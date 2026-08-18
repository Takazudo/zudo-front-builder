//! Cross-language parity test: asserts that
//! `zfb_types::render_region::render_region_marker` produces the exact
//! output recorded in the shared fixture at
//! `tests/fixtures/render-region-marker-parity.json` (embedded at compile
//! time as `RENDER_REGION_MARKER_PARITY_FIXTURE`).
//!
//! The SAME fixture is consumed by `crates/zfb/src/commands/render_artifact.rs`'s
//! `parse_marker` round-trip test (via the same embedded const) and by the
//! TS vitest suite in
//! `packages/zfb/src/__tests__/render-region-marker-cases.ts` — any drift
//! between a producer and the matcher is caught by one side or the other
//! failing.

use serde::Deserialize;
use zfb_types::render_region::{
    render_region_marker, RenderRegionEdge, RENDER_REGION_MARKER_PARITY_FIXTURE,
};

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

fn load_fixture() -> Fixture {
    serde_json::from_str(RENDER_REGION_MARKER_PARITY_FIXTURE)
        .unwrap_or_else(|e| panic!("cannot parse embedded parity fixture: {e}"))
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
