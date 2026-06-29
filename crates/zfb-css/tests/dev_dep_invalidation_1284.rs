//! Reproduction tests for bug #1284 (epic #1285), Wave-1 diagnosis #1286 —
//! CSS engine level (Level 1, pure logic, no V8).
//!
//! - **Symptom C** — a NEW Tailwind utility class authored inside a component
//!   under `src/**` is never emitted into `/assets/styles.css`. Root cause
//!   pinned here: `DEFAULT_CONTENT_ROOTS` (the `@source` scan roots Tailwind
//!   walks for utility classes) is `["pages","components","layouts","content"]`
//!   — it OMITS `src/`. So a class in `src/components/Foo.tsx` is outside every
//!   scanned `@source` glob and never reaches the generated stylesheet. (The
//!   second half of symptom C — the scan not re-running on a `.tsx` Module edit
//!   — is pinned in the orchestrator test `component_edit_triggers_css_rescan`.)
//!
//! - **Symptom B (engine half)** — `zfb-css` does NOT resolve local CSS
//!   `@import` file dependencies. `build_synthesised_entry_css` only prepends
//!   `@import "tailwindcss";` + `@source` directives and passes the user CSS
//!   through verbatim; a user `@import './local.css'` / `@import
//!   '@scope/design-system'` is left for the Tailwind CLI to resolve invisibly,
//!   so zfb never learns the real (canonicalised) dependency path to watch.
//!
//! #1288 has landed: the original `current_bug_*` tests (which locked TODAY's
//! broken behaviour) were migrated to their fixed-behaviour siblings below —
//! `src_root_is_scanned_for_utility_classes` and
//! `local_css_imports_are_resolved_to_real_paths` (now exercising the real D2
//! `@import` resolver). Both are un-ignored and green.

use std::fs;
use std::path::Path;

use zfb_css::engine::{default_source_directives, DEFAULT_CONTENT_ROOTS};
use zfb_css::resolve_css_imports;

/// SYMPTOM C (fixed) — `src/` is a scan root so a new utility class authored in
/// a `src/**` component is emitted. (Migrated from the now-removed
/// `current_bug_src_root_is_not_a_scan_root`, per the fix-author note above.)
#[test]
fn src_root_is_scanned_for_utility_classes() {
    assert!(
        DEFAULT_CONTENT_ROOTS.contains(&"src"),
        "fix adds src/ to the Tailwind scan roots"
    );
    let dirs = default_source_directives(Path::new("/proj"));
    assert!(
        dirs.contains("/proj/src"),
        "after the fix the emitted @source directives cover /proj/src"
    );
    // Sanity: the roots it already covered are still present.
    assert!(dirs.contains("/proj/components"));
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
        // tailwindcss is virtual — never resolved.
        assert_eq!(
            resolved.len(),
            2,
            "only the two real CSS deps, got {resolved:?}"
        );
    }
}
