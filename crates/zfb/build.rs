//! Cargo build script for the `zfb` binary crate.
//!
//! Stages the embedded vendor snapshot (`$OUT_DIR/vendor/`) that
//! `include_dir!` bakes into the binary so `cargo install zfb` works without
//! pnpm or Node.js: the `@takazudo/*` TypeScript sources and Hono. Bundling
//! is native (Rolldown, in-process), so no external bundler binary is
//! downloaded or staged.

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Runtime dependency version pin (sub #209 — embed framework dependencies)
//
// `hono` is not downloaded by `build.rs`; zfb's own `pnpm install` resolves
// its version in `pnpm-lock.yaml`, and `embed_framework_packages()` copies
// its resolved tree from `node_modules/.pnpm/<name>@<ver>*/node_modules/<name>`
// into `$OUT_DIR/vendor/<name>/`. This constant must match the lockfile.
//
// Source of truth: zfb's own `pnpm-lock.yaml`. To verify alignment, run:
//
//   pnpm list hono --depth 0 -r
//
// from the workspace root and confirm the output matches the constant below.
// ---------------------------------------------------------------------------

/// Pinned `hono` version (transitive dep of `@takazudo/zfb-runtime`).
const HONO_VERSION: &str = "4.12.25";

fn main() {
    // Re-run this build script (and thus recompile the binary) whenever the
    // release version env var changes. Without this directive, cargo will NOT
    // rebuild when only ZFB_RELEASE_VERSION changes, causing stale --version
    // output across local experiments or repeated CI builds.
    println!("cargo:rerun-if-env-changed=ZFB_RELEASE_VERSION");

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    let vendor_dir = out_dir.join("vendor");
    if vendor_dir.exists() {
        fs::remove_dir_all(&vendor_dir).expect("failed to purge OUT_DIR/vendor");
    }
    fs::create_dir_all(&vendor_dir).expect("failed to create OUT_DIR/vendor");

    // Sub 198 — embed @takazudo/zfb + zfb-runtime TypeScript source so the
    // installed binary works on a consumer with no node_modules.
    embed_runtime();

    // Sub 209 — embed Hono so a consumer with no node_modules can still
    // bundle the runtime dependency.
    embed_framework_packages();
}

// ---------------------------------------------------------------------------
// Embed the SDK, runtime, and slugify helper for Node-free builds.
// ---------------------------------------------------------------------------

/// Copy the TypeScript source of `@takazudo/zfb`, `@takazudo/zfb-runtime`,
/// and `@takazudo/zfb-slugify`
/// from `packages/` into `$OUT_DIR/vendor/@takazudo/` so `include_dir!` can
/// embed them in the binary at compile time.
///
/// Only `src/` (non-test files) and `package.json` are copied — `node_modules`,
/// `tsconfig.json`, `vitest.config.ts`, etc. are dev-only artefacts that must
/// not bloat the binary.
///
/// The function emits `cargo:rustc-env=ZFB_VENDOR_DIR=<path>` so the macro
/// invocation `include_dir!(env!("ZFB_VENDOR_DIR"))` resolves at compile time.
fn embed_runtime() {
    // Locate workspace root: two levels up from `crates/zfb/` (the manifest dir).
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .parent() // crates/
        .and_then(|p| p.parent()) // workspace root
        .expect("could not resolve workspace root from CARGO_MANIFEST_DIR");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let vendor_dir = out_dir.join("vendor").join("@takazudo");
    std::fs::create_dir_all(&vendor_dir).expect("failed to create vendor dir");

    // Packages to embed: (source package dir, dest package name)
    let packages = [
        ("packages/zfb", "zfb"),
        ("packages/zfb-runtime", "zfb-runtime"),
        ("packages/zfb-slugify", "zfb-slugify"),
    ];

    for (pkg_rel, pkg_name) in &packages {
        let src_pkg = workspace_root.join(pkg_rel);
        let dst_pkg = vendor_dir.join(pkg_name);

        // Copy package.json.
        let src_json = src_pkg.join("package.json");
        let dst_json = dst_pkg.join("package.json");
        std::fs::create_dir_all(&dst_pkg).expect("failed to create vendor package dir");
        std::fs::copy(&src_json, &dst_json).unwrap_or_else(|e| {
            panic!("failed to copy {}: {e}", src_json.display());
        });

        // Copy src/ — only non-test .ts files (skip __tests__ directory).
        let src_src = src_pkg.join("src");
        let dst_src = dst_pkg.join("src");
        copy_ts_src(&src_src, &dst_src);

        // Re-run if any source file changes.
        println!("cargo:rerun-if-changed={}", src_pkg.display());
    }

    // Emit vendor dir path as env var for the include_dir! macro.
    // include_dir! resolves env!(...) at macro-expansion time when the
    // env var is set via cargo:rustc-env.
    let vendor_root = out_dir.join("vendor");
    println!("cargo:rustc-env=ZFB_VENDOR_DIR={}", vendor_root.display());
}

/// Recursively copy `.ts` source files from `src` to `dst`, skipping any
/// directory named `__tests__` and any file whose name starts with `__`.
fn copy_ts_src(src: &Path, dst: &Path) {
    let rd = match std::fs::read_dir(src) {
        Ok(rd) => rd,
        Err(e) => panic!("failed to read dir {}: {e}", src.display()),
    };
    std::fs::create_dir_all(dst).expect("failed to create dst dir");

    for entry in rd.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip __tests__ and hidden dirs.
        if name_str.starts_with("__") || name_str.starts_with('.') {
            continue;
        }

        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if ft.is_dir() {
            copy_ts_src(&entry.path(), &dst.join(&name));
        } else if ft.is_file() {
            // Only embed .ts source files.
            if name_str.ends_with(".ts") {
                let dst_file = dst.join(&name);
                std::fs::copy(entry.path(), &dst_file).unwrap_or_else(|e| {
                    panic!("failed to copy {}: {e}", entry.path().display());
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Sub 209 — embed Hono so the native bundler can resolve it from the embedded
// extraction with no consumer-side node_modules.
// ---------------------------------------------------------------------------

/// Copy the published tree of `hono` from zfb's pnpm-installed
/// `node_modules/.pnpm/` store into
/// `$OUT_DIR/vendor/<pkg>/` so they ride along inside `EMBEDDED_VENDOR` (the
/// `include_dir!` snapshot in `crates/zfb/src/render_pipeline.rs`). The
/// extraction at runtime then produces `node_modules/<pkg>/` siblings beside
/// `node_modules/@takazudo/`, giving the bundler a complete tree to resolve from.
///
/// pnpm's content-addressable layout puts each direct dep at
/// `node_modules/.pnpm/<name>@<version>/node_modules/<name>/`. When a package
/// has injected peer deps, pnpm appends a suffix to the directory name, so
/// we match on the `<name>@<version>` prefix and accept the first match.
///
/// Only files needed at bundle time are copied — `node_modules/` (no nested
/// deps), `__tests__/`, hidden dirs, and source-map files are filtered out to
/// keep the embedded snapshot lean. Hono has no nested deps that need
/// following.
///
/// Source of truth for versions: `pnpm-lock.yaml` (mirrored by the
/// `HONO_VERSION` constant near the top of this file).
fn embed_framework_packages() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("could not resolve workspace root from CARGO_MANIFEST_DIR");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let vendor_dir = out_dir.join("vendor");
    std::fs::create_dir_all(&vendor_dir).expect("failed to create vendor dir");

    let pnpm_store = workspace_root.join("node_modules").join(".pnpm");

    let packages: [(&str, &str); 1] = [("hono", HONO_VERSION)];

    for (name, version) in &packages {
        let src_pkg = locate_pnpm_pkg(&pnpm_store, name, version).unwrap_or_else(|| {
            panic!(
                "runtime dependency `{name}@{version}` not found under {}.\n\
                 Run `pnpm install --frozen-lockfile` from the workspace root, \
                 then re-run cargo build.\n\
                 If the package has been bumped in pnpm-lock.yaml, also update \
                 the *_VERSION constant in crates/zfb/build.rs.",
                pnpm_store.display()
            )
        });

        let dst_pkg = vendor_dir.join(name);
        copy_pkg_published(&src_pkg, &dst_pkg);

        // Re-run if the source package contents change (e.g. a `pnpm install`
        // bump or a deliberate edit during local hacking).
        println!("cargo:rerun-if-changed={}", src_pkg.display());
    }

    // The cargo:rustc-env=ZFB_VENDOR_DIR=... line is emitted by
    // `embed_runtime()` and points at the same `$OUT_DIR/vendor` dir, so the
    // framework packages we just staged are picked up by the existing
    // `include_dir!("$ZFB_VENDOR_DIR")` macro in `render_pipeline.rs`.
}

/// Find the pnpm-store directory for `<name>@<version>` under
/// `node_modules/.pnpm/`. Accepts either the bare directory name
/// `<name>@<version>` or any peer-suffixed variant `<name>@<version>_*`.
fn locate_pnpm_pkg(pnpm_store: &Path, name: &str, version: &str) -> Option<PathBuf> {
    let exact_prefix = format!("{name}@{version}");
    let entries = std::fs::read_dir(pnpm_store).ok()?;
    for entry in entries.flatten() {
        let entry_name = entry.file_name();
        let entry_str = entry_name.to_string_lossy();
        // Accept exact match or `<name>@<version>_<peer-suffix>` variants.
        let matches = entry_str == exact_prefix
            || entry_str
                .strip_prefix(&exact_prefix)
                .is_some_and(|rest| rest.starts_with('_'));
        if !matches {
            continue;
        }
        let pkg_dir = entry.path().join("node_modules").join(name);
        if pkg_dir.is_dir() {
            return Some(pkg_dir);
        }
    }
    None
}

/// Recursively copy `src` to `dst`, skipping directories and files that are
/// dev-only or recursive (so we do not pull a vendored package's own
/// `node_modules/` into the binary). The filter is intentionally conservative:
/// we keep `package.json`, every `dist/`, `src/`, and any sibling published
/// JS / TS / d.ts entry files; we drop nested `node_modules/`, `__tests__/`,
/// hidden dirs, and `*.map` source maps.
fn copy_pkg_published(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("failed to create vendor pkg dir");
    let rd = std::fs::read_dir(src)
        .unwrap_or_else(|e| panic!("failed to read framework pkg dir {}: {e}", src.display()));
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Skip nested node_modules (recursive copy hazard) and hidden files.
        if name_str == "node_modules" || name_str.starts_with('.') {
            continue;
        }
        // Skip test directories.
        if name_str == "__tests__" || name_str == "test" || name_str == "tests" {
            continue;
        }
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            copy_pkg_published(&entry.path(), &dst.join(&name));
        } else if ft.is_file() {
            // Drop source maps to keep the embedded snapshot lean. The bundle
            // does not need them at consumer build time.
            if name_str.ends_with(".map") {
                continue;
            }
            let dst_file = dst.join(&name);
            std::fs::copy(entry.path(), &dst_file).unwrap_or_else(|e| {
                panic!("failed to copy {}: {e}", entry.path().display());
            });
        }
        // Symlinks and other non-regular entries are skipped — the pnpm store
        // never uses symlinks for the actual published files (only for the
        // top-level `node_modules/<name>` aliasing, which we don't touch).
    }
}
