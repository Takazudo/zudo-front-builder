use std::{fs, path::PathBuf};

use serde_json::Value;

const REGENERATION_COMMAND: &str = "cargo run -p zudo-wind --example export_catalog, then pnpm exec prettier --write crates/zudo-wind/catalog/zudo-wind-catalog.v1.json";

#[test]
fn committed_catalog_export_is_current() {
    let catalog_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("catalog/zudo-wind-catalog.v1.json");
    let committed = fs::read_to_string(&catalog_path).unwrap_or_else(|error| {
        panic!(
            "could not read {}: {error}; regenerate it with {REGENERATION_COMMAND}",
            catalog_path.display()
        )
    });
    let committed_value: Value = serde_json::from_str(&committed).unwrap_or_else(|error| {
        panic!(
            "{} is not valid JSON: {error}; regenerate it with {REGENERATION_COMMAND}",
            catalog_path.display()
        )
    });
    let generated =
        zudo_wind::catalog::export::export_json_pretty().expect("the in-memory catalog serializes");
    let generated_value: Value =
        serde_json::from_str(&generated).expect("the generated catalog is valid JSON");
    if committed_value != generated_value {
        panic!(
            "the committed zudo-wind catalog is stale; regenerate it with {REGENERATION_COMMAND}"
        );
    }
}
