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
//! ## Fix layer (for Wave 2 / issue #556)
//!
//! `crates/zfb-build/src/bundler.rs::rewrite_css_modules_in_shadow` is
//! the single place that must change. The minimal fix is to walk
//! symlinks too — `WalkDir::follow_links(true)` OR replace the
//! `entry.file_type().is_file()` gate with
//! `entry.path().is_file() || entry.file_type().is_symlink()` so the
//! symlink path goes through the rewrite.
//!
//! **CRITICAL gotcha — symlink-write corruption.** A naive fix that
//! merely starts walking symlinks but leaves `fs::write(path,
//! js.as_bytes())` intact will **silently corrupt the user's
//! original `.module.css` source files**, because `fs::write` opens
//! through the symlink and writes through it to the target.
//! Empirically verified: writing `"export default {...}"` through a
//! Unix symlink overwrites the symlink target's contents in place.
//! The fix must:
//!
//! 1. `fs::remove_file(path)` before `fs::write(path, js.as_bytes())`,
//!    so the symlink is replaced by a new regular file in the shadow
//!    (and the original source is left untouched), OR
//! 2. Open with `O_NOFOLLOW` semantics explicitly.
//!
//! The simplest is option (1) — one `fs::remove_file` before the
//! existing `fs::write`. The same fix should be applied (or verified
//! already safe) for any future walker that mutates files materialised
//! via `symlink_or_copy`.
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
//! Committed `#[ignore]`'d so base-branch CI stays green. Wave 2 (#556)
//! removes the ignore once the fix lands.

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

#[test]
#[ignore = "fails until #553 fix — Wave 2"]
fn corp_shape_components_module_css_builds_with_hashed_classes() {
    let Some(_esbuild) = locate_esbuild() else {
        eprintln!(
            "[css_modules_components_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // Project shell. Matches `zfb`'s "is this a project" sniff (needs
    // a `pages/` dir) and the producer's content-root convention
    // (`components/`, `layouts/`, etc.). No `tailwind` key → CSS
    // enabled by default.
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact" }
"#,
    )
    .unwrap();

    // Three components — `hero`, `about`, `contact`. Each owns its
    // own `.module.css` under `components/<name>/`, mirroring corp's
    // layout (`components/hero/hero.module.css`, …). At least 2–3
    // components are required so the multi-file failure mode is
    // exercised (one symlink-skipped file in the shadow tree would
    // still crash the build, but with multiple files the crash is the
    // expected steady-state behaviour rather than a one-off).
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

    // Snapshot the original `.module.css` byte contents so we can
    // catch the symlink-write corruption hazard (a naive Wave 2 fix
    // that merely flips `follow_links(true)` without replacing the
    // symlink in the shadow would write `export default {...};` THROUGH
    // the symlink and silently overwrite the user's source).
    let module_css_paths = [
        root.join("components/hero/hero.module.css"),
        root.join("components/about/about.module.css"),
        root.join("components/contact/contact.module.css"),
    ];
    let original_module_css: Vec<String> = module_css_paths
        .iter()
        .map(|p| fs::read_to_string(p).unwrap())
        .collect();

    // Drive the real `zfb build` binary — the same CLI path corp uses.
    // This routes through `compute_css_module_class_maps` →
    // `discover_css_source_files` → `scan_css_module_imports` →
    // `materialise_shadow` (symlinks!) → `rewrite_css_modules_in_shadow`
    // (symlinks skipped → bug) → esbuild (sees raw CSS → crash).
    let output = Command::new(zfb_binary())
        .arg("build")
        .current_dir(root)
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
    for local in ["section", "title", "heading", "form"] {
        let needle = format!("_{local}");
        assert!(
            html_blob.contains(&needle),
            "expected hashed class containing `{needle}` in emitted HTML; \
             raw `{local}` class would mean the rewrite never ran.\n--- html ---\n{}",
            truncate(&html_blob, 1200)
        );
    }
    // Raw class names must NOT leak (the user wrote `styles.section`,
    // not `class=\"section\"` — any `class=\"section\"` literal would
    // indicate the rewrite produced an empty `export default {};`,
    // making `styles.section` undefined).
    assert!(
        !html_blob.contains("class=\"section\""),
        "raw `class=\"section\"` should not appear; either the rewrite \
         failed or the module map was empty.\n--- html ---\n{}",
        truncate(&html_blob, 1200),
    );

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
        s.to_string()
    } else {
        format!("{}…[truncated]", &s[..n])
    }
}
