//! Regression tests for CSS source discovery and imported-file invalidation
//! (bug #1284, epic #1285), covering the CSS engine without V8.
//!
//! - **Symptom C** — a utility class in a component under `src/**` must reach
//!   `/assets/styles.css`. `DEFAULT_CONTENT_ROOTS` lists the directories wind
//!   scans for utility candidates; the regression verifies `src/` is included.
//!   The rescan-on-edit half is covered by the orchestrator test
//!   `component_edit_triggers_css_rescan`.
//!
//! - **Symptom B** — `zfb-css` resolves local CSS `@import` dependencies so the
//!   dev layer can watch their canonical paths. A legacy `@import
//!   "tailwindcss"` remains a virtual entry and is intentionally ignored by
//!   the resolver; authored local and workspace imports still resolve to files.
//!
//! #1288's fixed-behaviour tests below cover source discovery and recursive
//! import resolution through the `node_modules` workspace symlink.

use std::fs;

use zfb_css::engine::DEFAULT_CONTENT_ROOTS;
use zfb_css::resolve_css_imports;

/// SYMPTOM C (fixed) — `src/` is a scan root so a new utility class authored in
/// a `src/**` component is emitted. (Migrated from the now-removed
/// `current_bug_src_root_is_not_a_scan_root`, per the fix-author note above.)
#[test]
fn src_root_is_scanned_for_utility_classes() {
    assert!(
        DEFAULT_CONTENT_ROOTS.contains(&"src"),
        "fix adds src/ to the wind scan roots"
    );
}

/// SYMPTOM B (engine half, fixed) — the `@import` resolver (D2) surfaces the
/// local/workspace `@import` targets as canonicalised real paths so the dev
/// layer can register them as watch targets. Exercises the real resolver:
/// a relative `@import` and a workspace package consumed through a
/// `node_modules` symlink both resolve to their real on-disk files, recursively.
#[test]
fn local_css_imports_are_resolved_to_real_paths() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let proj = base.join("proj");
    let real_ds = base.join("real/design-system");
    fs::create_dir_all(proj.join("styles")).unwrap();
    fs::create_dir_all(proj.join("node_modules/@scope")).unwrap();
    fs::create_dir_all(real_ds.join("dist")).unwrap();

    fs::write(
        proj.join("styles/styles.css"),
        "@import \"tailwindcss\";\n@import './tokens.css';\n@import '@scope/design-system';\n",
    )
    .unwrap();
    fs::write(proj.join("styles/tokens.css"), "body{}\n").unwrap();
    fs::write(
        real_ds.join("package.json"),
        r#"{"name":"@scope/design-system","style":"dist/tokens.css"}"#,
    )
    .unwrap();
    fs::write(real_ds.join("dist/tokens.css"), "body{}\n").unwrap();

    // pnpm/workspace shape: the package is a symlink into node_modules.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&real_ds, proj.join("node_modules/@scope/design-system"))
            .unwrap();

        let resolved = resolve_css_imports(&proj.join("styles/styles.css"), &proj);

        let tokens_real = fs::canonicalize(proj.join("styles/tokens.css")).unwrap();
        let ds_real = fs::canonicalize(real_ds.join("dist/tokens.css")).unwrap();

        assert!(
            resolved.contains(&tokens_real),
            "relative @import resolves to its real path; got {resolved:?}"
        );
        assert!(
            resolved.contains(&ds_real),
            "workspace dep resolves through the node_modules symlink to its real path; got {resolved:?}"
        );
        // The legacy Tailwind entry is virtual — never resolved.
        assert_eq!(
            resolved.len(),
            2,
            "only the two real CSS deps, got {resolved:?}"
        );
    }
}
