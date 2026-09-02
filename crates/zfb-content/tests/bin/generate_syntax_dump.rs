//! Regenerates the custom `SyntaxSet` dump (issue #1848) — syntect's bundled
//! defaults plus the extra grammars committed under `assets/syntaxes/`.
//!
//! `SyntaxSet` is immutable once built, so there is no way to merge extra
//! grammars into `syntect::parsing::SyntaxSet::load_defaults_newlines()` at
//! runtime. This bin instead builds a full replacement set ahead of time and
//! writes it to `assets/syntax-set.packdump`, which
//! `src/syntect_highlight.rs` loads via `include_bytes!` +
//! `SyntaxSet::from_uncompressed_data` in place of `load_defaults_newlines()`.
//! Dumps store regex patterns as strings with lazy per-backend compilation, so
//! the one committed dump serves both the native `onig` backend and the wasm
//! `fancy` backend.
//!
//! `required-features = ["generate-syntax-dump"]` (see `Cargo.toml`) keeps
//! this native filesystem tool, and the `syntect/yaml-load` dependency it
//! needs to parse `.sublime-syntax` YAML, out of every default build —
//! mirroring `zfb-md-wasm`'s `generate_parity_oracle` bin.
//!
//! Regenerate with:
//! `cargo run -p zfb-content --bin generate_syntax_dump --features generate-syntax-dump`

use std::path::Path;

use syntect::dumps::dump_to_uncompressed_file;
use syntect::parsing::SyntaxSet;

/// Grammars this dump exists to add, sanity-checked by name below before the
/// dump is written — a silently-failed `add_from_folder` would otherwise ship
/// a dump indistinguishable from the plain bundled defaults.
const EXPECTED_EXTRA_SYNTAXES: &[&str] = &["TOML", "TypeScript", "TypeScriptReact"];

fn main() {
    // `add_from_folder` records the path it walked (joined per-component)
    // into `SyntaxSet::path_syntaxes` — unused by any lookup in this repo,
    // but still serialized into the committed dump. Passing it a RELATIVE
    // path (by `cd`-ing into the crate root first) keeps that recorded path
    // — and therefore the dump's bytes — identical across machines/checkouts
    // instead of embedding whoever regenerated it last ran it (codex#1
    // review finding on this issue).
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR"))
        .expect("CARGO_MANIFEST_DIR must be a valid directory");
    let syntaxes_dir = Path::new("assets/syntaxes");
    let out_path = Path::new("assets/syntax-set.packdump");

    // Start from syntect's own bundled defaults (every grammar the project
    // already relies on) and layer the extras on top — additive, not a
    // from-scratch grammar set, per the epic's review-mandated design.
    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
    builder
        .add_from_folder(syntaxes_dir, true)
        .unwrap_or_else(|error| {
            panic!(
                "loading extra .sublime-syntax files from {}: {error}",
                syntaxes_dir.display()
            )
        });
    let syntax_set = builder.build();

    for expected in EXPECTED_EXTRA_SYNTAXES {
        assert!(
            syntax_set.find_syntax_by_name(expected).is_some(),
            "expected syntax {expected:?} missing after add_from_folder({}); available: {:?}",
            syntaxes_dir.display(),
            syntax_set
                .syntaxes()
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
        );
    }

    dump_to_uncompressed_file(&syntax_set, out_path)
        .unwrap_or_else(|error| panic!("writing {}: {error}", out_path.display()));
    println!("wrote {}", out_path.display());
}
