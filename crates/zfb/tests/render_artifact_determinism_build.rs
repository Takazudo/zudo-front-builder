//! Issue #2427 (epic #2421, Wave 3 — CONFIRM), Test 2: double-build
//! determinism for the render-artifact export.
//!
//! Shape: `build_package_routes_consumer.rs`'s
//! `pages_only_build_is_byte_identical_across_two_runs` — two
//! independent `zfb build` runs (fresh tempdir, fresh symlinked
//! `node_modules` per that test's pattern) over the SAME fixture must
//! produce a byte-identical `dist/` tree, `dist/__zfb/render/**`
//! artifact files included.
//!
//! The fixture is a `docs` collection of SIBLING documents (all in one
//! directory, alphabetically adjacent names) so slug allocation is
//! exercised across the raw on-disk file set each run — the WalkDir
//! traversal-order trap `crates/zfb-build/src/bundler.rs` documents at
//! its sibling-sorting call sites (zfb#187): without sorting, the raw
//! per-OS `readdir` order could allocate a different specifier hash (or,
//! pre-fix, a different colliding-slug suffix) across two otherwise
//! identical runs. Each entry gets its own render artifact, so this is
//! also a direct proof that the `RenderMetadataIndex` join is stable
//! across runs, not just the collection snapshot.
//!
//! ## Level / tier
//!
//! Level 4 (real `zfb build` process e2e), tier T1. Self-skip convention
//! (no `#[ignore]`) — registered in nextest's `e2e-heavy` build-only
//! group in `.config/nextest.toml` per `crates/CLAUDE.md`.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use zfb_test_utils::{locate_esbuild, zfb_binary};

/// Symlink `node_modules/` to the extracted embedded `@takazudo` tree so
/// `@takazudo/zfb` / `@takazudo/zfb-runtime` resolve. Returns the
/// `TempDir` handle that must outlive the build. Mirrors
/// `build_package_routes_consumer.rs`'s `link_embedded_node_modules`.
fn link_embedded_node_modules(root: &Path) -> tempfile::TempDir {
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, root.join("node_modules"))
        .expect("symlink node_modules");
    nm_handle
}

fn run_zfb_build(root: &Path, esbuild: &Path) -> std::process::Output {
    Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn `zfb build`")
}

/// `true` when the non-zero build is a known-skip (no embedded V8 / no
/// esbuild).
fn is_known_skip(combined: &str) -> bool {
    combined.contains("embed_v8") || combined.contains("no esbuild")
}

/// Recursively collect every regular file under `dir` into a
/// `BTreeMap<relative-path-string, file-bytes>` — order-stable, fully
/// content-addressed (no timestamps). Mirrors
/// `build_package_routes_consumer.rs`'s `collect_all_files`.
fn collect_all_files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    if !dir.is_dir() {
        return out;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p
                    .strip_prefix(dir)
                    .expect("strip_prefix")
                    .to_string_lossy()
                    .into_owned();
                let bytes = fs::read(&p).unwrap_or_default();
                out.insert(rel, bytes);
            }
        }
    }
    out
}

/// Run `zfb build` in `root`, returning `Some(dist_tree)` on success or
/// `None` for a known-skip environment.
fn build_and_collect(
    root: &Path,
    esbuild: &Path,
    label: &str,
) -> Option<BTreeMap<String, Vec<u8>>> {
    let output = run_zfb_build(root, esbuild);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    if !output.status.success() && is_known_skip(&combined) {
        eprintln!("[{label}] known-skip indicator; skipping.\nstdout: {stdout}\nstderr: {stderr}");
        return None;
    }
    assert!(
        output.status.success(),
        "[{label}] expected `zfb build` to succeed; status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );
    Some(collect_all_files(&root.join("dist")))
}

/// Sibling document names, alphabetically adjacent, all in one directory —
/// the shape zfb#187 needs to exercise WalkDir traversal-order-dependent
/// slug allocation.
const DOC_NAMES: [&str; 6] = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"];

fn write_multi_page_fixture(root: &Path) {
    fs::write(
        root.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "emitRenderArtifacts": true,
  "collections": [{ "name": "docs", "path": "content/docs" }]
}
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("content/docs")).unwrap();
    for name in DOC_NAMES {
        fs::write(
            root.join(format!("content/docs/{name}.md")),
            format!(
                "---\ntitle: {name} doc\n---\n\n## {name} heading\n\nSibling body for {name}.\n"
            ),
        )
        .unwrap();
    }

    fs::create_dir_all(root.join("pages/docs")).unwrap();
    fs::write(
        root.join("pages/docs/[slug].tsx"),
        r#"export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const entries = await getCollection("docs");
  return entries.map((entry) => ({ params: { slug: entry.slug }, props: { entry } }));
}

export default function DocPage({ entry }) {
  return (
    <html lang="en">
      <head>
        <title>{entry.data.title}</title>
      </head>
      <body>
        <main>
          <entry.Content />
        </main>
      </body>
    </html>
  );
}
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"export default function Home() {
  return (
    <html lang="en">
      <head>
        <title>Determinism fixture</title>
      </head>
      <body>
        <p>RENDER_ARTIFACT_DETERMINISM_HOME</p>
      </body>
    </html>
  );
}
"#,
    )
    .unwrap();
}

#[test]
fn render_artifact_double_build_is_byte_identical() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[render_artifact_determinism] no esbuild; skipping.");
        return;
    };

    // Build A — first clean run, fresh tempdir + fresh symlinked
    // node_modules.
    let tmp_a = tempfile::tempdir().expect("tempdir A");
    let root_a = tmp_a.path();
    let _nm_a = link_embedded_node_modules(root_a);
    write_multi_page_fixture(root_a);
    let Some(tree_a) = build_and_collect(root_a, &esbuild, "render_artifact_determinism_A") else {
        return;
    };

    // Build B — second independent run, its own tempdir + symlink.
    let tmp_b = tempfile::tempdir().expect("tempdir B");
    let root_b = tmp_b.path();
    let _nm_b = link_embedded_node_modules(root_b);
    write_multi_page_fixture(root_b);
    let Some(tree_b) = build_and_collect(root_b, &esbuild, "render_artifact_determinism_B") else {
        return;
    };

    // Whole-tree byte identity — the general invariant this shape pins
    // in `build_package_routes_consumer.rs`.
    let keys_a: Vec<&str> = tree_a.keys().map(String::as_str).collect();
    let keys_b: Vec<&str> = tree_b.keys().map(String::as_str).collect();
    assert_eq!(
        keys_a, keys_b,
        "two independent builds must emit identical file sets; A={keys_a:#?} B={keys_b:#?}"
    );
    let mismatches: Vec<&String> = tree_a
        .iter()
        .filter(|(rel, bytes)| tree_b.get(*rel) != Some(*bytes))
        .map(|(rel, _)| rel)
        .collect();
    assert!(
        mismatches.is_empty(),
        "byte-identical parity FAILED for {} file(s) out of {} total — a nondeterministic \
         build regression, not to be normalized away:\n{mismatches:#?}",
        mismatches.len(),
        tree_a.len(),
    );

    // Pin the render-artifact files specifically — the sub-issue's
    // literal ask, and proof the metadata-index join (keyed on the
    // per-entry module specifier) is itself order-independent.
    let render_files: Vec<&String> = tree_a
        .keys()
        .filter(|k| k.starts_with("__zfb/render/"))
        .collect();
    assert_eq!(
        render_files.len(),
        DOC_NAMES.len(),
        "expected exactly one render artifact per sibling document; got {render_files:#?}"
    );
    for name in DOC_NAMES {
        let key = format!("__zfb/render/docs/{name}/index.json");
        let bytes = tree_a.get(&key).unwrap_or_else(|| {
            panic!("missing artifact for sibling `{name}`: {key}\nfound: {render_files:#?}")
        });
        let json: serde_json::Value =
            serde_json::from_slice(bytes).unwrap_or_else(|e| panic!("parse artifact {key}: {e}"));
        assert_eq!(
            json["route"],
            format!("/docs/{name}"),
            "route must use the bare filename-stem slug — a WalkDir-order-dependent alias \
             (zfb#187) would surface here as a numeric suffix"
        );
    }

    eprintln!(
        "[render_artifact_determinism] byte-identical across {} files, {} render artifacts.",
        tree_a.len(),
        render_files.len()
    );
}
