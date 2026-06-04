//! Integration tests for `zfb build` outdir-wipe behaviour (issue #805).
//!
//! ## What is tested
//!
//! Each test scaffolds a minimal fixture, runs `zfb build` via the real binary,
//! and asserts the wipe properties described in the issue acceptance criteria:
//!
//! 1. **Stale-file removal** — a stale asset + orphan HTML planted in `dist/`
//!    before the build are gone after the build; fresh outputs are present.
//! 2. **dist-as-symlink** — when `dist/` itself is a symlink pointing at a real
//!    directory the symlink survives the wipe and the target's contents are
//!    replaced by the fresh build.
//! 3. **Child symlink unlinked, target untouched** — a symlink planted INSIDE
//!    `dist/` pointing outside is removed as a link; its target file is not
//!    touched.
//! 4. **preBuild ordering** — a file emitted into `outdir` by a `preBuild`
//!    plugin survives the wipe and is present in the final `dist/` (the wipe
//!    runs before preBuild, so plugin-emitted files are safe).
//! 5. **Safety rejection** — passing `outdir == project_root` (or an ancestor
//!    thereof) causes `zfb build` to exit non-zero before touching the
//!    filesystem.
//!
//! All tests drive the real `zfb` binary via `Command::new(zfb_binary!())` so
//! the full async `pub async fn run()` path — including the new
//! `validate_outdir_safety` + `wipe_outdir_contents` calls — is exercised.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

use zfb_test_utils::{locate_esbuild, zfb_binary};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Write the minimal project fixture: `zfb.config.json` + `pages/index.tsx`
/// (no prerender export → defaults to SSG, no V8 / adapter needed).
fn write_minimal_fixture(root: &Path) {
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact" }
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"export default function Page() {
  return (
    <html lang="en">
      <head><title>test</title></head>
      <body><p>hello</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();
}

/// Run `zfb build` in `root` with the supplied esbuild path.
/// Returns the output struct so callers can inspect stdout/stderr/status.
fn run_zfb_build(root: &Path, esbuild: &Path) -> std::process::Output {
    Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn `zfb build`")
}

/// Run `zfb build` in `root` with the supplied esbuild path, also passing
/// `--outdir <outdir_arg>`.
fn run_zfb_build_with_outdir(root: &Path, esbuild: &Path, outdir_arg: &str) -> std::process::Output {
    Command::new(zfb_binary!())
        .arg("build")
        .arg("--outdir")
        .arg(outdir_arg)
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn `zfb build`")
}

fn dump(output: &std::process::Output) -> String {
    format!(
        "status={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

// ---------------------------------------------------------------------------
// Test 1 — stale files are removed on rebuild
// ---------------------------------------------------------------------------

/// A stale asset and an orphan HTML page planted in `dist/` before the build
/// are gone after the build; fresh outputs are present.
#[test]
fn stale_assets_and_orphan_html_are_removed_on_rebuild() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[build_cleans_outdir] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_minimal_fixture(root);

    // First build — creates dist/.
    let out = run_zfb_build(root, &esbuild);
    assert!(out.status.success(), "first build failed\n{}", dump(&out));

    let dist = root.join("dist");
    assert!(dist.is_dir(), "dist/ must exist after first build");

    // Plant stale files that should not survive the next build.
    let stale_js = dist.join("assets").join("islands-old-stale.js");
    fs::create_dir_all(dist.join("assets")).unwrap();
    fs::write(&stale_js, b"// stale island bundle").unwrap();

    let orphan_html = dist.join("blog").join("dead").join("index.html");
    fs::create_dir_all(dist.join("blog").join("dead")).unwrap();
    fs::write(&orphan_html, b"<html><body>dead page</body></html>").unwrap();

    assert!(stale_js.exists(), "stale JS must exist before rebuild");
    assert!(orphan_html.exists(), "orphan HTML must exist before rebuild");

    // Second build — wipe must remove the stale files.
    let out2 = run_zfb_build(root, &esbuild);
    assert!(out2.status.success(), "second build failed\n{}", dump(&out2));

    assert!(
        !stale_js.exists(),
        "stale asset must be removed by wipe; found: {}",
        stale_js.display()
    );
    assert!(
        !orphan_html.exists(),
        "orphan HTML must be removed by wipe; found: {}",
        orphan_html.display()
    );

    // Fresh output must be present after the second build.
    assert!(
        dist.join("index.html").exists(),
        "dist/index.html must be emitted after clean rebuild"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — dist itself as a symlink survives with fresh target contents
// ---------------------------------------------------------------------------

/// `dist/` is itself a symlink pointing at a real directory. The wipe must
/// keep the symlink intact and replace the target's contents with fresh output.
#[test]
fn dist_as_symlink_survives_with_fresh_target_contents() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[build_cleans_outdir] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_minimal_fixture(root);

    // Create the real directory and a stale file inside it.
    let real_dist = tmp.path().join("real_dist_dir");
    fs::create_dir_all(&real_dist).unwrap();
    fs::write(real_dist.join("stale.js"), b"// stale").unwrap();

    // dist/ is a symlink pointing at real_dist_dir.
    let dist_link = root.join("dist");
    symlink(&real_dist, &dist_link).unwrap();

    let out = run_zfb_build(root, &esbuild);
    assert!(out.status.success(), "build with dist-as-symlink failed\n{}", dump(&out));

    // The symlink must still be a symlink (not replaced by a directory).
    let meta = fs::symlink_metadata(&dist_link).expect("symlink_metadata on dist link");
    assert!(
        meta.file_type().is_symlink(),
        "dist/ symlink must survive the wipe; it was replaced"
    );

    // The stale file inside the real directory must be gone.
    assert!(
        !real_dist.join("stale.js").exists(),
        "stale.js inside the symlink target must be removed"
    );

    // Fresh output must be present inside the real directory.
    assert!(
        real_dist.join("index.html").exists(),
        "real_dist/index.html must be emitted into the symlink target"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — child symlink inside dist is removed; target untouched
// ---------------------------------------------------------------------------

/// A symlink planted inside `dist/` pointing outside must be unlinked by the
/// wipe. Its target file must remain untouched.
#[test]
fn child_symlink_in_dist_is_unlinked_target_untouched() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[build_cleans_outdir] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_minimal_fixture(root);

    // First build — creates dist/.
    let out = run_zfb_build(root, &esbuild);
    assert!(out.status.success(), "first build failed\n{}", dump(&out));

    let dist = root.join("dist");

    // Plant a symlink inside dist pointing at an external file.
    let external_target = tmp.path().join("external.txt");
    fs::write(&external_target, b"keep me intact").unwrap();
    let child_link = dist.join("external-link.txt");
    symlink(&external_target, &child_link).unwrap();

    // Second build — the wipe must remove the link, not follow it.
    let out2 = run_zfb_build(root, &esbuild);
    assert!(out2.status.success(), "second build failed\n{}", dump(&out2));

    assert!(
        !child_link.exists(),
        "child symlink inside dist must be removed by the wipe"
    );
    assert!(
        // Check with symlink_metadata: link itself must not exist.
        fs::symlink_metadata(&child_link).is_err(),
        "child symlink entry must not exist (not even as a dangling link) after wipe"
    );
    assert!(
        external_target.exists(),
        "external target of the removed symlink must be untouched"
    );
    assert_eq!(
        fs::read_to_string(&external_target).unwrap(),
        "keep me intact",
        "content of the external target must be unmodified"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — preBuild plugin-emitted file survives the wipe (ordering check)
// ---------------------------------------------------------------------------

/// A file emitted into `outdir` by a `preBuild` plugin must be present in the
/// final `dist/` after the build. This verifies that the wipe runs BEFORE
/// `preBuild`, not after (see build.rs ordering comment for zfb#805).
#[test]
fn prebuild_plugin_emitted_file_survives_wipe() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[build_cleans_outdir] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    if Command::new("node")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("[build_cleans_outdir] node not on PATH; skipping preBuild ordering test.");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // Write a plugin that emits a sentinel file into outDir during preBuild.
    let plugin_path = root.join("prebuild-marker-plugin.mjs");
    fs::write(
        &plugin_path,
        r#"import { writeFileSync, mkdirSync } from "node:fs";
export default {
  name: "prebuild-marker-plugin",
  preBuild(ctx) {
    mkdirSync(ctx.outDir, { recursive: true });
    writeFileSync(ctx.outDir + "/prebuild-marker.txt", "emitted-by-prebuild");
  },
};
"#,
    )
    .unwrap();

    // Config referencing the local plugin by relative path.
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact", "plugins": [{ "name": "./prebuild-marker-plugin.mjs" }] }
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"export default function Page() {
  return (
    <html lang="en">
      <head><title>test</title></head>
      <body><p>hello</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();

    let out = run_zfb_build(root, &esbuild);
    assert!(
        out.status.success(),
        "build with preBuild plugin failed\n{}",
        dump(&out)
    );

    let dist = root.join("dist");
    let marker = dist.join("prebuild-marker.txt");
    assert!(
        marker.exists(),
        "preBuild-emitted file must survive the wipe and be present in dist/; \
         if missing the wipe runs AFTER preBuild (ordering regression).\n\
         dist/ contents: {:#?}",
        fs::read_dir(&dist)
            .ok()
            .map(|rd| rd.flatten().map(|e| e.path()).collect::<Vec<_>>())
    );
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        "emitted-by-prebuild",
        "preBuild marker content must be as written by the plugin"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — outdir == project root is rejected before any deletion
// ---------------------------------------------------------------------------

/// Passing `--outdir .` (i.e. the project root) must cause `zfb build` to
/// exit non-zero before touching anything. A canary file planted at the root
/// must still be present.
#[test]
fn outdir_equal_to_project_root_is_rejected() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[build_cleans_outdir] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_minimal_fixture(root);

    // Plant a canary file that must survive (no deletion must occur).
    let canary = root.join("canary.txt");
    fs::write(&canary, b"do not delete").unwrap();

    // `--outdir .` resolves to the project root — must be rejected.
    let out = run_zfb_build_with_outdir(root, &esbuild, ".");
    assert!(
        !out.status.success(),
        "`zfb build --outdir .` must exit non-zero; got success\n{}",
        dump(&out)
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("project root or an ancestor") || stderr.contains("outdir safety"),
        "expected a safety-rejection message in stderr; got:\n{stderr}"
    );

    // Canary must be intact — no deletion occurred.
    assert!(
        canary.exists(),
        "canary.txt must not be deleted when the safety check fires"
    );
}

// ---------------------------------------------------------------------------
// Test 6 — outdir as ancestor of project root is rejected
// ---------------------------------------------------------------------------

/// Passing `--outdir ..` (parent of the project root) must be rejected with a
/// clear error. Uses a nested directory so the ancestor check fires cleanly.
#[test]
fn outdir_as_ancestor_of_project_root_is_rejected() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[build_cleans_outdir] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    // project root is nested one level down so `..` is a real existing ancestor.
    let root = tmp.path().join("project");
    fs::create_dir_all(&root).unwrap();
    write_minimal_fixture(&root);

    // Plant a canary at the ancestor level that must survive.
    let canary = tmp.path().join("canary-ancestor.txt");
    fs::write(&canary, b"ancestor-level do not delete").unwrap();

    // `--outdir ..` resolves to the parent of the project root — must be rejected.
    let out = run_zfb_build_with_outdir(&root, &esbuild, "..");
    assert!(
        !out.status.success(),
        "`zfb build --outdir ..` must exit non-zero; got success\n{}",
        dump(&out)
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("project root or an ancestor") || stderr.contains("outdir safety"),
        "expected a safety-rejection message in stderr; got:\n{stderr}"
    );

    // Canary must be intact — no deletion occurred.
    assert!(
        canary.exists(),
        "canary-ancestor.txt must not be deleted when the safety check fires"
    );
}
