//! Regression test for issue #553 — `zfb build` crashes on a real
//! consumer that uses `*.module.css` under `components/<name>/`.
//!
//! # Diagnosis (Wave 1 / issue #555)
//!
//! ## Proven root cause: H2 — symlinked `.module.css` is skipped by
//! `rewrite_css_modules_in_shadow`.
//!
//! Empirically verified:
//!
//! - `crates/zfb-build/src/bundler.rs::materialise_shadow` calls
//!   `symlink_or_copy` for every non-transformed file, which on Unix
//!   produces a **symlink** in the shadow tree (commit `a113fd3`,
//!   "perf(bundler): symlink non-transformed files in shadow walkers").
//! - `rewrite_css_modules_in_shadow` (`bundler.rs:1814`) walks with
//!   `WalkDir::follow_links(false)` then gates entries on
//!   `entry.file_type().is_file()`. With `follow_links(false)`, WalkDir
//!   reports a symlink-to-file as `is_symlink() == true,
//!   is_file() == false` (verified with a 30-line WalkDir reproducer).
//!   So every `.module.css` symlink in the shadow is **skipped** and
//!   **never rewritten** to a JS class-map module.
//! - `--loader:.module.css=js` is applied unconditionally
//!   (`ESBUILD_LOADER_ARGS`). esbuild canonicalises the source symlink
//!   to the original `.module.css` path and parses the raw CSS bytes
//!   as JS → `Unexpected "."` at the **original** project path. This
//!   matches corp's failure signature exactly.
//!
//! ## Reconciliation of the spec's "rule-out" premise
//!
//! Epic #554 / sub-issue #555 stated the existing
//! `crates/zfb-build/tests/bundler_css_modules.rs` (`pages/hero.module.css`)
//! **passes** with the rewrite applied, and asked us to explain the
//! discrepancy. **The premise is false** — that test ALSO fails today,
//! with the same `Unexpected "."` crash on a single flat
//! `pages/hero.module.css`. It silently regressed when commit
//! `a113fd3` introduced `symlink_or_copy`; nothing in CI noticed
//! because the test gracefully skips when no `esbuild` binary is
//! present. The unit test in `bundler.rs:3079`
//! (`rewrite_css_modules_in_shadow_rewrites_mapped_and_unmapped`) does
//! NOT catch the bug because it creates shadow files via direct
//! `fs::write` — they are **real files, not symlinks** — so its
//! `entry.file_type().is_file()` check returns `true`. The bug only
//! manifests when files reach the shadow via `materialise_shadow`,
//! which is the production path.
//!
//! H1 (producer empties class map) is not live for corp: corp uses
//! relative imports under the `components/` content root, which
//! `discover_css_source_files` + `scan_css_module_imports` resolve
//! correctly. H3 (tsconfig alias bypass) is not live either: corp's
//! `tsconfig.json` has no `paths` and uses `baseUrl: "."`, so imports
//! are plain relative paths — no alias to rewrite. (Verified against
//! `Takazudo/zfb-example-corporate-website` via `gh api`.)
//!
//! ## Fix layer (applied in Wave 2 / issue #556)
//!
//! Three interacting bugs — all in `crates/zfb-build/src/bundler.rs`:
//!
//! **Layer 1 — symlink-aware walk (`rewrite_css_modules_in_shadow`).**
//! The old `entry.file_type().is_file()` gate skipped every `.module.css`
//! symlink because WalkDir with `follow_links(false)` reports a
//! symlink-to-file as `is_symlink()==true, is_file()==false`. The fix
//! switches to `path.is_file()` — `Path::is_file()` follows symlinks,
//! returning `true` for symlinks-to-files and `false` for broken
//! symlinks. Note: `WalkDir::follow_links(true)` was not used here
//! because it raises an IO error for broken symlinks inside shadow
//! `node_modules` trees (pnpm-style dangling symlinks) — `path.is_file()`
//! handles those gracefully by returning `false`.
//!
//! **Layer 2 — symlink-write corruption guard (`rewrite_css_modules_in_shadow`).**
//! A naive fix that merely starts walking symlinks but leaves
//! `fs::write(path, js.as_bytes())` intact will **silently corrupt the
//! user's original `.module.css` source files**, because `fs::write`
//! opens through the symlink and writes through it to the target.
//! Empirically verified: writing `"export default {...}"` through a Unix
//! symlink overwrites the symlink target's contents in place. The fix
//! adds `fs::remove_file(path)` before `fs::write`, so the symlink is
//! replaced by a new regular file in the shadow. The source-untouched
//! assertion in this test explicitly guards against regression.
//!
//! **Layer 3 — anchor esbuild to the shadow (`run_esbuild`).**
//! Even after Layers 1 and 2, esbuild (without `--preserve-symlinks`)
//! canonicalises symlinked `.tsx` importers to their real project paths
//! and resolves relative imports (e.g. `./hero.module.css`) from *there*
//! — finding the original raw CSS, not the rewritten JS shim in the
//! shadow. The fix adds `--preserve-symlinks` when
//! `node_modules_dir.is_none()`, which anchors esbuild to the shadow.
//! Gated on `is_none()` to avoid the #443/#450 path-alias regression
//! that fires when `node_modules_dir` is set and workspace-package
//! importers have `node_modules` in their shadow path.
//!
//! ## Test scope
//!
//! The fixture mirrors corp's shape exactly:
//!
//! - `zfb.config.json` with NO `tailwind` key — `css_enabled` falls
//!   through to `true` (matches corp's `zfb.config.ts`).
//! - Relative imports (no `@/...` alias) — matches corp's `tsconfig.json`
//!   (no `paths`).
//! - Three components under `components/<name>/<name>.tsx` each importing
//!   `./<name>.module.css` — covers the multi-file failure mode and the
//!   `components/` content root (corp uses six).
//! - `pages/index.tsx` imports the components from `../components/<name>/<name>`,
//!   matching corp's `pages/index.tsx`.
//!
//! The test drives the real `zfb` binary as a subprocess (the same
//! path corp uses), going through
//! `compute_css_module_class_maps` → `discover_css_source_files` →
//! `scan_css_module_imports` → `materialise_shadow` →
//! `rewrite_css_modules_in_shadow` → esbuild end-to-end. It does NOT
//! pre-supply a class map via `BundlerInput::css_module_class_maps`
//! (which would bypass the failure point — see
//! `crates/zfb-build/tests/bundler_css_modules.rs` for that
//! shortcut).
//!
//! ## Why this test lives in `crates/zfb/tests/`
//!
//! It must exercise the full producer + bundler pipeline driven by
//! `zfb build`, which requires the `CARGO_BIN_EXE_zfb` test-time env
//! var Cargo only sets for integration tests under the `zfb` crate. A
//! sibling test in `crates/zfb-build/tests/` can only construct
//! `BundlerInput` directly — that path takes a pre-supplied class
//! map and bypasses `compute_css_module_class_maps`, which is the
//! exact step where the producer/consumer contract for `#553` lives.
//!
//! The dual assertion checks:
//!
//! 1. Built HTML under `dist/` carries **hashed** class names
//!    (e.g. `<hash>_hero`), not raw `class="hero"`.
//! 2. The emitted `dist/assets/styles-<hash>.css` contains the same
//!    matching scoped selector.
//! 3. **The original `components/<name>/<name>.module.css` source
//!    files still contain raw CSS** — guards against the symlink-write
//!    corruption hazard described above.
//!
//! ## Status
//!
//! Wave 1 (#555) committed the test `#[ignore]`'d so base-branch CI stayed
//! green between waves. Wave 2 (#556) landed the bundler fix and un-ignored
//! the test. A second corp-shape variant
//! (`corp_shape_with_real_node_modules_and_no_tsconfig_paths_builds`) was
//! added during Wave 2 deep-review to lock in the `--preserve-symlinks` gate
//! path 3 — both variants must stay green.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Locate the test esbuild binary the same way the other zfb / zfb-build
/// integration tests do: prefer `ZFB_ESBUILD_BIN`, then the workspace
/// `crates/zfb/binaries/esbuild/esbuild` slot, then a pnpm-installed
/// esbuild under `node_modules/.pnpm`, then `which esbuild`. Returns
/// `None` if nothing is available so the test can skip gracefully.
fn locate_esbuild() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZFB_ESBUILD_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace) = here.parent().and_then(|p| p.parent()) {
        let slot = workspace.join("crates/zfb/binaries/esbuild/esbuild");
        if slot.exists() {
            return Some(slot);
        }
        let store = workspace.join("node_modules/.pnpm");
        if let Ok(rd) = fs::read_dir(&store) {
            for entry in rd.flatten() {
                let cand = entry
                    .path()
                    .join("node_modules/@esbuild/linux-x64/bin/esbuild");
                if cand.exists() {
                    return Some(cand);
                }
            }
        }
    }
    if let Ok(out) = Command::new("which").arg("esbuild").output() {
        if out.status.success() {
            let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn zfb_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_zfb"))
}

/// Write a small component that imports its own `.module.css` and
/// references three class names from it. The component renders a
/// `<section>` with the scoped `<hash>_section` class.
fn write_component(root: &Path, name: &str, classes: &[&str]) {
    let dir = root.join("components").join(name);
    fs::create_dir_all(&dir).unwrap();

    // `.module.css` — at least three classes per component so the
    // hash-map has real content. Plain `.<class> { color: …; }` blocks
    // are enough; lightningcss treats them as locals.
    let css = classes
        .iter()
        .enumerate()
        .map(|(i, c)| format!(".{c} {{ color: #{:06x}; }}\n", 0x101010 * (i + 1)))
        .collect::<String>();
    fs::write(dir.join(format!("{name}.module.css")), css).unwrap();

    // `.tsx` — relative import only (matches corp's import style).
    let primary = classes[0];
    let secondary = classes.get(1).copied().unwrap_or(primary);
    fs::write(
        dir.join(format!("{name}.tsx")),
        format!(
            r#"import styles from "./{name}.module.css";

export default function {pascal}() {{
  return (
    <section class={{styles.{primary}}}>
      <span class={{styles.{secondary}}}>{name}</span>
    </section>
  );
}}
"#,
            pascal = pascal_case(name),
        ),
    )
    .unwrap();
}

fn pascal_case(s: &str) -> String {
    s.split(['-', '_'])
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Set up the corp-shape fixture (three components under `components/<name>/`,
/// each with a `.module.css` + `.tsx`, plus `pages/index.tsx` importing them).
/// Returns the snapshot of the original `.module.css` byte contents so the
/// caller can verify they are not corrupted by the build (symlink-write
/// hazard).
fn write_corp_shape_fixture(root: &Path) -> (Vec<PathBuf>, Vec<String>) {
    // No `tailwind` key → CSS enabled by default. `pages/` directory
    // satisfies `zfb`'s "is this a project" sniff.
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact" }
"#,
    )
    .unwrap();

    // Three components mirroring corp's `components/<name>/<name>.{tsx,module.css}`
    // layout. At least 2–3 components are required so the multi-file failure
    // mode is exercised (one symlink-skipped file in the shadow would still
    // crash the build, but with multiple files the crash is the expected
    // steady-state behaviour rather than a one-off).
    write_component(root, "hero", &["section", "title", "lede"]);
    write_component(root, "about", &["section", "heading", "body"]);
    write_component(root, "contact", &["section", "form", "field"]);

    // `pages/index.tsx` imports each component with the same relative
    // style corp uses (`../components/<name>/<name>`).
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"import Hero from "../components/hero/hero";
import About from "../components/about/about";
import Contact from "../components/contact/contact";

export default function HomePage() {
  return (
    <main>
      <Hero />
      <About />
      <Contact />
    </main>
  );
}
"#,
    )
    .unwrap();

    let module_css_paths = vec![
        root.join("components/hero/hero.module.css"),
        root.join("components/about/about.module.css"),
        root.join("components/contact/contact.module.css"),
    ];
    let original_module_css: Vec<String> = module_css_paths
        .iter()
        .map(|p| fs::read_to_string(p).unwrap())
        .collect();

    (module_css_paths, original_module_css)
}

/// Run `zfb build` against the corp-shape fixture and run the full assertion
/// suite: source files unchanged (symlink-write guard), build success,
/// hashed class names in HTML, scoped selectors in `dist/assets/styles-*.css`.
///
/// Drives the real `zfb` binary as a subprocess (the same CLI path corp uses):
/// `compute_css_module_class_maps` → `discover_css_source_files` →
/// `scan_css_module_imports` → `materialise_shadow` (symlinks!) →
/// `rewrite_css_modules_in_shadow` → esbuild.
///
/// Pass the resolved esbuild path through `ZFB_ESBUILD_BIN` so the subprocess
/// uses the same binary the test discovered. Without this, environments where
/// `locate_esbuild()` only found esbuild on `PATH` or under
/// `node_modules/.pnpm` would fail with an unrelated "esbuild not found"
/// error rather than reproducing the CSS modules crash. (codex review
/// finding, P2.)
fn build_and_assert_corp_shape(
    root: &Path,
    esbuild: &Path,
    module_css_paths: &[PathBuf],
    original_module_css: &[String],
) {
    let output = Command::new(zfb_binary())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // The originals MUST still be raw CSS, regardless of whether the
    // build succeeds. This guards against the symlink-write corruption
    // hazard (see the file-level //! doc comment).
    for (path, original) in module_css_paths.iter().zip(original_module_css.iter()) {
        let current = fs::read_to_string(path).unwrap_or_default();
        assert_eq!(
            &current, original,
            "the user's source file at {} was modified by the build — \
             this is the symlink-write corruption hazard described in the \
             file-level //! doc comment. Wave 2's fix MUST replace the symlink \
             in the shadow before writing, not write through it.",
            path.display()
        );
    }

    assert!(
        output.status.success(),
        "expected `zfb build` to succeed; got status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );

    // Assertion 1 — built HTML carries hashed class names.
    //
    // lightningcss's default class-name pattern is `[hash]_[local]`,
    // so a class like `.section` becomes `<hash>_section` in the
    // emitted HTML. We grep for a `<hash>_section` token in any
    // emitted HTML under `dist/`. The exact hash is non-deterministic
    // (content-derived), so we match by the suffix `_section` after
    // some characters; the leading `class="` rules out a stray match
    // in a JS bundle.
    let dist = root.join("dist");
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

    // The hashed class names must appear in the HTML. We accept any
    // non-empty prefix as the hash, matching lightningcss's
    // `[hash]_[local]` convention. If the bug is live the HTML never
    // gets emitted at all (build crashes), so the earlier
    // `output.status.success()` assertion already fails — this is
    // belt-and-braces.
    //
    // The chosen subset `[section, title, heading, form]` provides
    // multi-file coverage: `section` is shared across all three
    // components (so all three must have been rewritten), while
    // `title` is unique to hero, `heading` to about, and `form` to
    // contact — so a single-component build cannot satisfy all four
    // assertions. Other class names (`lede`, `body`, `field`) are
    // CSS-only and don't reach the rendered HTML, which is why they
    // aren't asserted here.
    for local in ["section", "title", "heading", "form"] {
        let needle = format!("_{local}");
        assert!(
            html_blob.contains(&needle),
            "expected hashed class containing `{needle}` in emitted HTML; \
             raw `{local}` class would mean the rewrite never ran.\n--- html ---\n{}",
            truncate(&html_blob, 1200)
        );
    }
    // Note: we don't add a negative `!html_blob.contains("class=\"section\"")`
    // assertion. The component sources use `class={styles.section}` (a JSX
    // expression), so if the rewrite produces `export default {};` then
    // `styles.section` is `undefined` and Preact omits the attribute
    // entirely — `class="section"` would never appear as a literal even
    // in the bug state. The positive `_section` check above already
    // catches the empty-map case (it would fail to find any hashed
    // suffix).

    // Assertion 2 — `styles-<hash>.css` contains the matching scoped
    // selectors.
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
            panic!(
                "expected dist/assets/styles-<hash>.css to be emitted; got: {css_paths:#?}"
            )
        });
    let css_body = fs::read_to_string(styles_css).unwrap();

    for local in ["section", "title", "heading", "form"] {
        let needle = format!("_{local}");
        assert!(
            css_body.contains(&needle),
            "expected scoped selector containing `{needle}` in {}; \
             raw `.{local}` would mean lightningcss did not scope the class.\n\
             --- css ---\n{}",
            styles_css.display(),
            truncate(&css_body, 1200),
        );
    }
}

/// Wave 2 / #553 regression — embedded-vendor path.
///
/// This variant has NO `node_modules/` in the fixture, so `zfb build` falls
/// back to `embedded_node_modules()` (the cargo-install scenario). That
/// path sets `BundlerInput::node_modules_preserve_symlinks = true` and the
/// `--preserve-symlinks` gate fires via path 1 of the comment block in
/// `bundler.rs::run_esbuild`.
#[test]
fn corp_shape_components_module_css_builds_with_hashed_classes() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[css_modules_components_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let (module_css_paths, original_module_css) = write_corp_shape_fixture(root);
    build_and_assert_corp_shape(root, &esbuild, &module_css_paths, &original_module_css);
}

/// Wave 2 / #553 regression — corp's actual `pnpm install` shape.
///
/// This variant stages a real project `node_modules/` (symlinked from the
/// embedded vendor) so `detect_project_node_modules` returns `Some(...)` and
/// `BundlerInput::node_modules_preserve_symlinks` defaults to `false`. With
/// the project also having NO `tsconfig.json` `paths`, the `--preserve-symlinks`
/// gate fires via path 3 of the comment block in `bundler.rs::run_esbuild`.
///
/// Without path 3, this test reproduces the exact `Unexpected "."` crash
/// corp reports in #553 (or, if the symlink-write hazard isn't guarded,
/// the source-file corruption assertion catches the regression first).
///
/// Unix-only because Windows symlinks need admin/developer mode and the
/// shadow-walk symlink hazard doesn't apply on the Windows code path in
/// `symlink_or_copy`.
#[cfg(unix)]
#[test]
fn corp_shape_with_real_node_modules_and_no_tsconfig_paths_builds() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[css_modules_components_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let (module_css_paths, original_module_css) = write_corp_shape_fixture(root);

    // Stage a real `node_modules/` so `detect_project_node_modules` returns
    // `Some(...)` instead of falling back to `embedded_node_modules()`. We
    // symlink it to the same embedded-vendor tree the `zfb` binary would
    // have extracted anyway — saving the extraction cost while still
    // exercising the corp-shape code path. The `_nm_handle` keeps the
    // tempdir alive for the test's duration.
    let (_nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, root.join("node_modules"))
        .expect("symlink node_modules");

    // No `tsconfig.json` written — corp also has no `compilerOptions.paths`.
    // This combination is exactly what gate path 3 is meant to handle.

    build_and_assert_corp_shape(root, &esbuild, &module_css_paths, &original_module_css);
}

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
    // Find the largest char boundary <= n so byte-slicing doesn't
    // panic on multi-byte UTF-8 (the fixture is ASCII today, but the
    // build output we format into assertion failures may not be).
    let cut = (0..=n).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    format!("{}…[truncated]", &s[..cut])
}
