//! Cargo build script for the `zfb` binary.
//!
//! Each top-level function addresses a distinct concern so that Sub 197
//! (binary download) and Sub 198 (runtime embedding) can land in the same
//! file without merge conflicts:
//!
//! - `embed_runtime()` — copies `packages/zfb` and `packages/zfb-runtime`
//!   source files into `$OUT_DIR/vendor/@takazudo/` and emits the env-var
//!   `ZFB_VENDOR_DIR` so the `include_dir!` macro can embed them at compile
//!   time.
//!
//! Sub 197 will add its own function here (e.g. `download_binaries()`) and
//! call it from `main()`. Keep a single blank line between each call in
//! `main()` for readability.

use std::path::{Path, PathBuf};

fn main() {
    embed_runtime();
}

// ---------------------------------------------------------------------------
// Sub 198 — embed @takazudo/zfb and @takazudo/zfb-runtime
// ---------------------------------------------------------------------------

/// Copy the TypeScript source of `@takazudo/zfb` and `@takazudo/zfb-runtime`
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
                std::fs::copy(&entry.path(), &dst_file).unwrap_or_else(|e| {
                    panic!("failed to copy {}: {e}", entry.path().display());
                });
            }
        }
    }
}
