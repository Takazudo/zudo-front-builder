//! Oracle generator for the wasm-vs-native parity gate (zfb#1578, epic
//! zfb#1572, wave 4).
//!
//! Regenerate command (run from the repo root):
//!
//! ```sh
//! cargo run -p zfb-md-wasm --bin generate_parity_oracle --features parity-oracle
//! ```
//!
//! The `--features parity-oracle` flag is required: this bin's `[[bin]]`
//! entry in `Cargo.toml` carries `required-features = ["parity-oracle"]` so
//! it is excluded from every DEFAULT build of this package -- notably
//! `pnpm build`'s `cargo build --target wasm32-unknown-unknown ... -p
//! zfb-md-wasm` (no `--lib`/`--bin` filter), which would otherwise
//! cross-compile this native-only, filesystem-touching bin for
//! wasm32-unknown-unknown too.
//!
//! ## Why this bin's output is the correct oracle (fancy-regex, not onig)
//!
//! This bin links `zfb-md-wasm`'s **rlib** (`crate-type = ["cdylib",
//! "rlib"]` in `Cargo.toml`) on the host target triple -- the exact same
//! source (`src/lib.rs`) the `wasm32-unknown-unknown` cdylib build compiles,
//! just for a different target. `zfb-md-wasm`'s `Cargo.toml` pulls
//! `zfb-content`/`zfb-render` with `default-features = false, features =
//! ["syntect-fancy"]` -- and because `-p zfb-md-wasm` only builds this
//! crate's own dependency subgraph (`zfb-content` reached solely through
//! `zfb-render`, never through the `zfb`/`zfb-build` binary crates whose
//! dev-dependency cycle re-unifies the default `syntect-onig` backend), that
//! feature selection is never re-widened back to onig. In other words: in
//! this specific `-p zfb-md-wasm` graph there is no path that could turn
//! onig back on, so this oracle is guaranteed fancy-regex-backed --
//! verifiable independently with `cargo tree -p zfb-md-wasm -e normal | grep
//! -i onig` (empty). Oracle and the wasm build under test share this same
//! highlighter, which is what makes the downstream byte-exact gate
//! (`crates/zfb-md-wasm/npm/test/parity.test.ts`) winnable at all -- see
//! that test file's header comment for the full parity contract.
//!
//! ## What this bin does
//!
//! Reads `tests/fixtures/parity/manifest.json` (the single source of truth
//! shared with the vitest parity suite), and for every listed fixture:
//! 1. Reads the markdown/MDX source from `tests/fixtures/parity/<input>`.
//! 2. Calls `zfb_md_wasm::compile` or `zfb_md_wasm::render_html` (selected
//!    by the fixture's `tier`) with the fixture's `options` re-serialized
//!    to a JSON string.
//! 3. Writes the raw JSON string returned (unmodified) to
//!    `tests/fixtures/parity/expected/<slug>.json`. The parity gate parses
//!    this JSON and compares the re-serialized value (see that suite's
//!    header for why parse-then-compare is the right boundary), so the
//!    on-disk formatting is not load-bearing.
//!
//! IMPORTANT: the committed `expected/*.json` are prettier-formatted (the
//! repo's `format:check` gate covers `**/*.json`), but this bin writes
//! compact single-line JSON. After regenerating, run `pnpm format` (or
//! `pnpm --filter docs exec prettier --write <dir>`) so the committed files
//! stay `format:check`-clean. Key order is identical either way (the same
//! serde struct order the wasm build emits), so this is formatting only.
//!
//! Not a `#[test]` on purpose: this is a one-shot regeneration tool, not a
//! regression check (the regression check is the vitest parity suite that
//! reads this bin's committed output).

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
    tier: String,
    input: String,
    options: Value,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parity")
}

fn main() {
    let dir = fixtures_dir();
    let manifest_path = dir.join("manifest.json");
    let manifest_text = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", manifest_path.display()));
    let manifest: Manifest = serde_json::from_str(&manifest_text)
        .unwrap_or_else(|e| panic!("parsing {}: {e}", manifest_path.display()));

    let expected_dir = dir.join("expected");
    fs::create_dir_all(&expected_dir)
        .unwrap_or_else(|e| panic!("creating {}: {e}", expected_dir.display()));

    let mut count = 0usize;
    for fixture in &manifest.fixtures {
        let input_path = dir.join(&fixture.input);
        let source = fs::read_to_string(&input_path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", input_path.display()));
        let options_json = serde_json::to_string(&fixture.options)
            .unwrap_or_else(|e| panic!("serializing options for {}: {e}", fixture.slug));

        let result = match fixture.tier.as_str() {
            "compile" => zfb_md_wasm::compile(&source, &options_json),
            "renderHtml" => zfb_md_wasm::render_html(&source, &options_json),
            other => panic!("fixture {}: unknown tier {other:?}", fixture.slug),
        };

        // Fail loudly if a corpus fixture produces a diagnostic where none
        // was intended (only `malformed-mdx-diagnostic` is allowed to carry
        // one) -- an accidental parse failure would make the oracle assert
        // the wrong thing without this bin ever noticing.
        let parsed: Value = serde_json::from_str(&result)
            .unwrap_or_else(|e| panic!("fixture {} produced invalid JSON: {e}", fixture.slug));
        let diagnostics_len = parsed["diagnostics"].as_array().map_or(0, Vec::len);
        if fixture.slug != "malformed-mdx-diagnostic" && diagnostics_len != 0 {
            panic!(
                "fixture {} produced {diagnostics_len} unexpected diagnostic(s): {parsed}",
                fixture.slug
            );
        }

        let out_path = expected_dir.join(format!("{}.json", fixture.slug));
        fs::write(&out_path, &result)
            .unwrap_or_else(|e| panic!("writing {}: {e}", out_path.display()));
        println!("wrote {}", relative(&out_path, &dir));
        count += 1;
    }

    println!(
        "\ngenerated {count} oracle fixture(s) into {}",
        expected_dir.display()
    );
}

fn relative(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}
