//! Snapshot test for the precise per-collection `.d.ts` emitter.
//!
//! Pins the emitted output for the canonical "two collections, both
//! with schemas" fixture. Drives the same code path that the build
//! pipeline uses, so any drift in the textual contract surfaces here.
//!
//! The fixture lives at `tests/fixtures/two-collections-schemas/`:
//! - `schemas.json` — JSON Schema for each collection's frontmatter.
//! - `expected.d.ts` — the canonical emitter output (UPDATE_GOLDEN
//!   regenerates it).
//!
//! Set the env var `UPDATE_GOLDEN=1` to overwrite `expected.d.ts` with
//! whatever the emitter produces today (use this when intentionally
//! changing the contract).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use zfb_content::collection::{emit_types_dts_with_schemas, CollectionTypeInfo};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("two-collections-schemas")
}

#[test]
fn snapshot_two_collections_with_schemas() {
    let dir = fixture_dir();
    let schemas_text = std::fs::read_to_string(dir.join("schemas.json"))
        .expect("schemas.json fixture must exist");
    // Use BTreeMap so iteration over the schemas is alphabetical and
    // deterministic across runs. The fixture file order is also
    // alphabetical (`blog`, `docs`), but we want to emit in a stable
    // order regardless of how serde_json deserializes the top-level
    // map.
    let schemas: BTreeMap<String, JsonValue> =
        serde_json::from_str(&schemas_text).expect("schemas.json must be valid JSON");

    // Emit in the canonical order documented in the brief: docs, then blog.
    let docs = schemas.get("docs").expect("`docs` schema present");
    let blog = schemas.get("blog").expect("`blog` schema present");
    let collections = vec![
        CollectionTypeInfo {
            name: "docs",
            schema: Some(docs),
        },
        CollectionTypeInfo {
            name: "blog",
            schema: Some(blog),
        },
    ];

    let tmp_path = std::env::temp_dir().join(format!(
        "zfb-content-dts-snapshot-{}-{}.d.ts",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    emit_types_dts_with_schemas(&tmp_path, &collections).expect("emit ok");
    let actual = std::fs::read_to_string(&tmp_path).expect("read emitted dts");
    let _ = std::fs::remove_file(&tmp_path);

    let expected_path = dir.join("expected.d.ts");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&expected_path, &actual).expect("update golden");
        eprintln!(
            "[dts_snapshot] wrote new golden to {}",
            expected_path.display()
        );
        return;
    }

    let expected = std::fs::read_to_string(&expected_path)
        .expect("expected.d.ts golden must exist (run with UPDATE_GOLDEN=1 to refresh)");
    assert_eq!(
        actual,
        expected,
        ".d.ts emitter output drifted from {}; rerun with UPDATE_GOLDEN=1 if intentional",
        Path::new("tests/fixtures/two-collections-schemas/expected.d.ts").display(),
    );
}
