//! Oracle generator for the direct `highlightCode` wasm-vs-native parity gate.
//!
//! Regenerate with:
//!
//! ```sh
//! cargo run -p zfb-md-wasm --bin generate_highlight_parity_oracle --features highlight-parity-oracle
//! ```
//!
//! This native-only bin calls the crate's rlib directly with the focused
//! arbitrary-code corpus, then writes the exact JSON documents that the npm
//! suite compares against output from the built wasm artifact. It is separate
//! from `generate_parity_oracle`: that frozen corpus covers existing Markdown
//! compile/render behavior and must not grow for this new API.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Manifest {
    fixtures: Vec<FixtureEntry>,
}

#[derive(Debug, Deserialize)]
struct FixtureEntry {
    slug: String,
    code: String,
    options: Value,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/highlight-parity")
}

fn main() {
    let dir = fixtures_dir();
    let manifest_path = dir.join("manifest.json");
    let manifest_text = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", manifest_path.display()));
    let manifest: Manifest = serde_json::from_str(&manifest_text)
        .unwrap_or_else(|error| panic!("parsing {}: {error}", manifest_path.display()));

    let expected_dir = dir.join("expected");
    fs::create_dir_all(&expected_dir)
        .unwrap_or_else(|error| panic!("creating {}: {error}", expected_dir.display()));

    for fixture in &manifest.fixtures {
        let options_json = serde_json::to_string(&fixture.options)
            .unwrap_or_else(|error| panic!("serializing options for {}: {error}", fixture.slug));
        let result = zfb_md_wasm::highlight_code(&fixture.code, &options_json);
        let parsed: Value = serde_json::from_str(&result).unwrap_or_else(|error| {
            panic!("fixture {} produced invalid JSON: {error}", fixture.slug)
        });
        assert!(
            parsed["html"].is_string(),
            "fixture {} must demonstrate usable highlight markup: {parsed}",
            fixture.slug
        );

        let out_path = expected_dir.join(format!("{}.json", fixture.slug));
        fs::write(&out_path, result)
            .unwrap_or_else(|error| panic!("writing {}: {error}", out_path.display()));
        println!("wrote {}", relative(&out_path, &dir));
    }
}

fn relative(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}
