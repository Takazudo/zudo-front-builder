//! Issue #1698 — `crates/zfb` command-layer regression test for sibling
//! `.module.css` (issue #1696: CSS discovery + emission wiring through
//! `zfb_build::SiblingMirrorPlan`).
//!
//! `crates/zfb-build/tests/bundler_sibling_mirror_esbuild_regression.rs`'s
//! case (e) proves the BUNDLER'S sibling `.module.css` rewrite reaches
//! esbuild correctly, but it pre-supplies `BundlerInput::css_module_class_maps`
//! directly (the shortcut documented on
//! `crates/zfb-build/tests/bundler_css_modules.rs`) — it cannot exercise
//! `discover_css_source_files` / `compute_css_module_class_maps`, which only
//! exist at THIS crate's command layer (`crates/zfb/src/commands/build.rs`).
//! This test drives the real `zfb build` binary end-to-end so a sibling
//! `.module.css`'s class map is DISCOVERED and HASHED by the real
//! command-layer scan — not hand-supplied — proving #1696's
//! `discover_css_source_files` (walking every claimed `SiblingMirrorPlan`
//! mirror root) and `compute_css_module_class_maps` / `run_css_emitter`
//! (gating a resolved sibling module on `plan.claims_path` and hashing via
//! the workspace-aware `CssModulesConfig`) are actually wired into `zfb
//! build`, mirroring `css_modules_components_build.rs`'s project-level
//! pattern for issue #553 but for a WORKSPACE SIBLING reached through a
//! wildcard tsconfig alias under an UNRELATED non-empty `bundle.exclude`
//! (the exact combination issue #1685 broke).
//!
//! ## Regression criterion
//!
//! Verified to FAIL when cherry-picked onto the epic's parent
//! (`base/sweep-260718`, pre-epic — no `SiblingMirrorPlan`, no sibling CSS
//! discovery) and PASS on the epic branch; see the PR/issue description for
//! both run transcripts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use zfb_test_utils::{locate_esbuild, zfb_binary};

fn collect_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
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
            } else if p.extension().and_then(|s| s.to_str()) == Some(ext) {
                out.push(p);
            }
        }
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let cut = (0..=n).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    format!("{}…[truncated]", &s[..cut])
}

/// A pnpm workspace whose `sub-packages/host` member imports a sibling
/// component (`<ws_root>/lib/shared/Widget.tsx`) through a wildcard
/// tsconfig alias (`@shared/*`). The sibling component imports its own
/// `Widget.module.css` — the class map for it can ONLY be produced by the
/// real command-layer scan discovering the sibling through the claimed
/// `SiblingMirrorPlan` mirror root, since nothing under `project_root`
/// itself references `Widget.module.css`.
///
/// Returns `(project_root, node_modules_tempdir_handle)` — the handle keeps
/// the workspace-hoisted `node_modules` (symlinked from the SAME
/// embedded-vendor tree the `zfb` binary itself would extract, per
/// `css_modules_components_build.rs`'s `corp_shape_with_real_node_modules_...`
/// variant) alive for the test's duration. The SIBLING's own automatic-JSX
/// import (`preact/jsx-runtime`) resolves via `<work>/node_modules`, which
/// the bundler symlinks to `<ws_root>/node_modules` regardless of
/// `bundle.exclude` (issue #1693) — so the workspace root needs a real
/// `node_modules/preact`, not just the project's own embedded fallback.
fn write_fixture(ws_root: &Path) -> (PathBuf, tempfile::TempDir) {
    fs::write(
        ws_root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'sub-packages/*'\n",
    )
    .unwrap();
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, ws_root.join("node_modules"))
        .expect("symlink workspace node_modules");

    let project = ws_root.join("sub-packages/host");
    fs::create_dir_all(project.join("pages")).unwrap();

    // No `tailwind` key -> CSS enabled by default (matches
    // `css_modules_components_build.rs`'s corp-shape fixture). An UNRELATED
    // non-empty `bundle.exclude` arms the shadow-only / no-live-fallback
    // regime issue #1685 broke under.
    fs::write(
        project.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "bundle": { "exclude": ["components/never-matches/**"] }
}
"#,
    )
    .unwrap();

    fs::write(
        project.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@shared/*": ["../../lib/shared/*"] }
  }
}
"#,
    )
    .unwrap();

    fs::write(
        project.join("pages/index.tsx"),
        r#"import Widget from "@shared/Widget";

export default function HomePage() {
  return (
    <main>
      <Widget />
    </main>
  );
}
"#,
    )
    .unwrap();

    let sibling = ws_root.join("lib/shared");
    fs::create_dir_all(&sibling).unwrap();
    fs::write(
        sibling.join("Widget.module.css"),
        ".box { color: #101010; }\n.label { color: #202020; }\n",
    )
    .unwrap();
    fs::write(
        sibling.join("Widget.tsx"),
        r#"import styles from "./Widget.module.css";

export default function Widget() {
  return (
    <section class={styles.box}>
      <span class={styles.label}>widget</span>
    </section>
  );
}
"#,
    )
    .unwrap();

    (project, nm_handle)
}

/// Issue #1696/#1698: a workspace-sibling `.module.css` reached through a
/// claimed tsconfig alias gets a REAL, command-layer-computed class map
/// (not a manually-supplied one) that reaches both the emitted HTML and the
/// emitted stylesheet, under an UNRELATED non-empty `bundle.exclude`.
#[test]
fn sibling_module_css_class_map_is_discovered_and_emitted_by_real_zfb_build() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[sibling_css_module_command_layer_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, _nm_handle) = write_fixture(tmp.path());

    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(&project)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected `zfb build` to succeed under an UNRELATED non-empty \
         bundle.exclude; got status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );

    let dist = project.join("dist");
    let html_paths = collect_files(&dist, "html");
    assert!(
        !html_paths.is_empty(),
        "no HTML files emitted under dist/; expected at least dist/index.html"
    );
    let html_blob = html_paths
        .iter()
        .map(|p| fs::read_to_string(p).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    for local in ["box", "label"] {
        let needle = format!("_{local}");
        assert!(
            html_blob.contains(&needle),
            "expected hashed class containing `{needle}` in emitted HTML — a \
             raw `{local}` class (or a missing one) would mean the sibling's \
             class map was never discovered by the command layer.\n--- html ---\n{}",
            truncate(&html_blob, 1200)
        );
    }

    let assets_dir = dist.join("assets");
    let css_paths = collect_files(&assets_dir, "css");
    let styles_css = css_paths
        .iter()
        .find(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.starts_with("styles-") && n.ends_with(".css"))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| {
            panic!("expected dist/assets/styles-<hash>.css to be emitted; got: {css_paths:#?}")
        });
    let css_body = fs::read_to_string(styles_css).unwrap();
    for local in ["box", "label"] {
        let needle = format!("_{local}");
        assert!(
            css_body.contains(&needle),
            "expected scoped selector containing `{needle}` in {}; the \
             sibling's scoped CSS must reach the emitted stylesheet, not just \
             the class-map shim.\n--- css ---\n{}",
            styles_css.display(),
            truncate(&css_body, 1200),
        );
    }
}
