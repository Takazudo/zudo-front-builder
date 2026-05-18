//! Parity tests for the snapshot-walker ↔ bundler-walker contract
//! introduced when `CollectionDef.include` / `.exclude` / `.idStripSuffix`
//! landed.
//!
//! The snapshot side (this crate's `walk_collection_with_cache_and_filter`
//! → `build_snapshot_with_config` → `EntrySnapshot.module_specifier`)
//! and the bundler side (`crates/zfb-build/src/bundler.rs`'s
//! `materialise_collection` → `ContentImport.specifier`) MUST agree
//! on the final specifier byte-for-byte. Any divergence makes every
//! `globalThis.__zfb.content.get(spec)` lookup miss and dumps the page
//! into a `<pre data-zfb-content-fallback>` block.
//!
//! These tests exercise the SHARED helpers
//! (`CollectionFilter::matches_relative` and
//! `maybe_strip_specifier_suffix`) end-to-end against the snapshot
//! pipeline so the contract is locked at this layer. The bundler's
//! parallel walk is covered by `crates/zfb-build/tests/bundler_*.rs`
//! integration tests that gate on esbuild.

use std::fs;
use std::path::{Path, PathBuf};

use zfb_content::collection::{maybe_strip_specifier_suffix, CollectionFilter};
use zfb_content::{build_snapshot_with_config, CollectionConfig, SnapshotPipelineConfig};
use zfb_content::{compile_mdx_to_jsx_module_cached, MdxJsxOptions};

fn tmp_root(label: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "zfb-content-parity-{label}-{n}-{pid}",
        pid = std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create tmp dir");
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(p, contents).expect("write file");
}

/// `idStripSuffix` rewrites the snapshot's `module_specifier` slug
/// segment. Pin this so the bundler-side rewrite (which uses the same
/// shared helper) can be cross-checked against snapshot output even
/// without running esbuild.
#[test]
fn snapshot_module_specifier_has_id_strip_suffix_applied() {
    let root = tmp_root("strip");
    let col_dir = root.join("notes-en");
    write(
        &col_dir,
        "col003-mixers.en.mdx",
        "---\ntitle: Mixers EN\n---\nbody\n",
    );
    write(
        &col_dir,
        "col004-osc.en.mdx",
        "---\ntitle: Osc EN\n---\nbody\n",
    );
    // Non-EN sibling that should be excluded by the include glob.
    write(
        &col_dir,
        "col003-mixers.mdx",
        "---\ntitle: Mixers JA\n---\nbody\n",
    );

    let cfg = CollectionConfig {
        name: "notes-en".into(),
        root: col_dir.clone(),
        include: Some(vec!["*.en.mdx".into()]),
        exclude: None,
        id_strip_suffix: Some(".en".into()),
    };
    let snap = build_snapshot_with_config(&[cfg], &SnapshotPipelineConfig::default())
        .expect("snapshot build");
    let entries = snap.collections.get("notes-en").expect("collection");

    // Two EN entries survive (JA sibling excluded). Snapshot is sorted
    // by slug ascending, and slugs have the `.en` suffix stripped.
    assert_eq!(entries.len(), 2, "entries: {entries:?}");
    assert_eq!(entries[0].slug, "col003-mixers");
    assert_eq!(entries[1].slug, "col004-osc");

    // Specifier slug segment also has `.en` stripped — the consumer's
    // bridge map key MUST be in this form for `getEntry('notes-en',
    // 'col003-mixers')` to resolve.
    assert!(
        entries[0]
            .module_specifier
            .starts_with("mdx://notes-en/col003-mixers#"),
        "spec: {}",
        entries[0].module_specifier,
    );
    assert!(
        !entries[0].module_specifier.contains("col003-mixers.en"),
        "spec still has pre-strip slug: {}",
        entries[0].module_specifier,
    );

    // ----- Bundler-side simulation -------------------------------------
    //
    // Reproduce what `materialise_collection` does for the same file:
    // compile MDX with `compile_mdx_to_jsx_module_cached`, then run
    // the shared `maybe_strip_specifier_suffix` helper. The result
    // must equal the snapshot's `module_specifier` byte-for-byte —
    // that's the invariant the bridge map relies on.
    let file = col_dir.join("col003-mixers.en.mdx");
    let raw = fs::read_to_string(&file).unwrap();
    // Snapshot walker calls `frontmatter::extract` then compiles the
    // post-frontmatter body. Mirror that here so the JSX hash agrees.
    let uf = zfb_content::extract_tsx_frontmatter; // unused; just import probe
    let _ = uf;
    let body = {
        // Use the public re-export `zfb_content::FrontmatterError` only
        // to keep imports tight; we inline the YAML strip here.
        let after_open = raw.strip_prefix("---\n").unwrap_or(&raw);
        if let Some(close_idx) = after_open.find("\n---\n") {
            after_open[(close_idx + 5)..].to_string()
        } else {
            raw.clone()
        }
    };
    let compiled = compile_mdx_to_jsx_module_cached(&body, &file, None, None).expect("compile mdx");
    let bundler_specifier = maybe_strip_specifier_suffix(&compiled.specifier, Some(".en"));
    assert_eq!(
        bundler_specifier, entries[0].module_specifier,
        "snapshot↔bundler specifier mismatch — bridge lookups would miss",
    );

    // Touch a default-construction helper so the public-API surface
    // for downstream consumers stays compile-checked.
    let _ = MdxJsxOptions::default();
}

/// When `idStripSuffix` is unset, the helper is a no-op and the
/// snapshot specifier carries the pre-strip slug exactly as before.
/// Pinned so a future refactor can't accidentally start stripping.
#[test]
fn snapshot_specifier_unchanged_when_id_strip_suffix_absent() {
    let root = tmp_root("nostrip");
    let col_dir = root.join("notes");
    write(
        &col_dir,
        "col003-mixers.mdx",
        "---\ntitle: Mixers JA\n---\nbody\n",
    );

    let cfg = CollectionConfig::new("notes", col_dir.clone());
    let snap = build_snapshot_with_config(&[cfg], &SnapshotPipelineConfig::default())
        .expect("snapshot build");
    let entries = snap.collections.get("notes").expect("collection");
    assert_eq!(entries.len(), 1);
    assert!(
        entries[0]
            .module_specifier
            .starts_with("mdx://notes/col003-mixers#"),
        "spec: {}",
        entries[0].module_specifier,
    );
}

/// `CollectionFilter::matches_relative` is the public surface the
/// bundler calls. The walker uses the same matcher under the hood, so
/// any path that the snapshot drops MUST also be the path the bundler
/// drops (and vice versa).
#[test]
fn matcher_verdicts_agree_across_surfaces() {
    let filter = CollectionFilter::new(
        Some(&["*.mdx".to_string()]),
        Some(&["*.en.mdx".to_string()]),
        None,
    )
    .unwrap();
    assert!(filter.matches_relative("col003-mixers.mdx"));
    assert!(!filter.matches_relative("col003-mixers.en.mdx"));
    assert!(
        !filter.matches_relative("readme.md"),
        "non-mdx falls through include"
    );
}
