//! Wave 2 regression tests for plugin alias / virtual-module
//! exact-match resolution in the islands esbuild bundler (#269).
//!
//! Mirrors `crates/zfb-build/tests/bundler_exact_match_resolution.rs`
//! for the islands path. The contract being pinned:
//!
//! 1. `EsbuildSubprocessConfig::alias_entries` with `("@/foo",
//!    "/abs/foo.tsx")` resolves `@/foo` exactly — `@/foo/bar` is NOT
//!    silently rewritten the way `--alias` would have done.
//! 2. `EsbuildSubprocessConfig::virtual_modules` with
//!    `("virtual:foo", "...")` behaves the same way —
//!    `virtual:foo/bar` is unresolved.
//!
//! Esbuild gating follows the precedence used by the zfb-build
//! integration tests: `ZFB_ESBUILD_BIN` env var → workspace slot →
//! `which esbuild`. Skips with a printed note when no binary is
//! present.

use std::fs;
use std::path::PathBuf;

use zfb_islands::{
    BundleConfig, ClientBundler, EsbuildSubprocessBundler, EsbuildSubprocessConfig, Island,
};
use zfb_test_utils::locate_esbuild;

/// Build a working_dir with a `components/` subtree holding one island
/// TSX file. `island_source` is the file body; `aliased_target_filename`
/// (when `Some`) is laid down at `<root>/src/<name>` so the caller can
/// register an alias pointing to it.
///
/// Returns the absolute path to the island TSX file (used as the
/// `Island::new` argument).
fn write_island_fixture(
    root: &std::path::Path,
    island_source: &str,
    aliased_target_filename: Option<&str>,
    aliased_target_body: &str,
) -> PathBuf {
    fs::create_dir_all(root.join("components")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    if let Some(name) = aliased_target_filename {
        fs::write(root.join("src").join(name), aliased_target_body).unwrap();
    }
    let island_path = root.join("components/counter.tsx");
    fs::write(&island_path, island_source).unwrap();
    island_path
}

/// **Exact-match regression test — alias prefix-with-slash MUST fail.**
///
/// Register `@/foo` → `<root>/src/foo.tsx` via `alias_entries`. The
/// island imports `@/foo/bar` (NOT the registered specifier). esbuild
/// must NOT rewrite this to `<root>/src/foo.tsx/bar` — bundling must
/// fail with an unresolved-import error referencing the bare specifier.
#[test]
fn alias_does_not_match_prefix_with_slash() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let island_path = write_island_fixture(
        &root,
        // Island imports `@/foo/bar` — prefix-with-slash form. Under
        // the old `--alias:@/foo=<target>` semantics esbuild would
        // silently rewrite this to `<target>/bar`. The Wave 2
        // contract is exact-match-only, so this MUST fail.
        "import Foo from \"@/foo/bar\";\n\
         export const Counter = () => <Foo />;\n",
        Some("foo.tsx"),
        "export default function Foo() { return null; }\n",
    );

    let cfg = EsbuildSubprocessConfig {
        extra_args: vec![
            std::ffi::OsString::from("--external:preact"),
            std::ffi::OsString::from("--external:preact/*"),
            std::ffi::OsString::from("--external:@takazudo/*"),
        ],
        ..EsbuildSubprocessConfig::default()
    }
    .with_binary_path(esbuild)
    .with_working_dir(&root)
    .with_alias_entries(vec![(
        "@/foo".to_string(),
        root.join("src/foo.tsx").to_string_lossy().into_owned(),
    )]);
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(root.join("dist"));
    let err = bundler
        .bundle(&[Island::new("Counter", &island_path)], &bundle_cfg)
        .expect_err(
            "Wave 2 contract: alias `@/foo` must NOT match `@/foo/bar`; \
             bundling must fail.",
        );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("@/foo/bar"),
        "esbuild error should mention the unresolved specifier `@/foo/bar`. \
         Got: {msg}"
    );
}

/// **Exact-match — happy path (alias).**
///
/// Register `@/foo` → `<root>/src/foo.tsx`. The island imports `@/foo`
/// (the registered specifier). Bundling must succeed and the aliased
/// component's identifier must reach the bundle.
#[test]
fn alias_matches_exact_specifier() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let island_path = write_island_fixture(
        &root,
        "import Foo from \"@/foo\";\n\
         export const Counter = () => <Foo />;\n",
        Some("foo.tsx"),
        "export default function AliasedFooMarker() { return null; }\n",
    );

    let cfg = EsbuildSubprocessConfig {
        extra_args: vec![
            std::ffi::OsString::from("--external:preact"),
            std::ffi::OsString::from("--external:preact/*"),
            std::ffi::OsString::from("--external:@takazudo/*"),
        ],
        ..EsbuildSubprocessConfig::default()
    }
    .with_binary_path(esbuild)
    .with_working_dir(&root)
    .with_alias_entries(vec![(
        "@/foo".to_string(),
        root.join("src/foo.tsx").to_string_lossy().into_owned(),
    )]);
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(root.join("dist"));
    let out = bundler
        .bundle(&[Island::new("Counter", &island_path)], &bundle_cfg)
        .expect("exact-match alias `@/foo` must resolve");
    // Bundler carries bytes in memory — no disk write.
    let body = String::from_utf8(out.bytes).expect("bundled bytes are valid UTF-8");
    assert!(
        body.contains("AliasedFooMarker"),
        "exact-match alias bundle should contain the aliased target's \
         marker symbol `AliasedFooMarker`."
    );
}

/// **Exact-match — virtual module prefix-with-slash MUST fail.**
#[test]
fn virtual_module_does_not_match_prefix_with_slash() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let island_path = write_island_fixture(
        &root,
        "import Foo from \"virtual:foo/bar\";\n\
         export const Counter = () => <Foo />;\n",
        None,
        "",
    );

    let cfg = EsbuildSubprocessConfig {
        extra_args: vec![
            std::ffi::OsString::from("--external:preact"),
            std::ffi::OsString::from("--external:preact/*"),
            std::ffi::OsString::from("--external:@takazudo/*"),
        ],
        ..EsbuildSubprocessConfig::default()
    }
    .with_binary_path(esbuild)
    .with_working_dir(&root)
    .with_virtual_modules(vec![(
        "virtual:foo".to_string(),
        "export default function Foo() { return null; }\n".to_string(),
    )]);
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(root.join("dist"));
    let err = bundler
        .bundle(&[Island::new("Counter", &island_path)], &bundle_cfg)
        .expect_err(
            "Wave 2 contract: virtual module `virtual:foo` must NOT match \
             `virtual:foo/bar`; bundling must fail.",
        );
    let msg = format!("{err:#}");
    assert!(
        msg.contains("virtual:foo/bar"),
        "esbuild error should mention the unresolved specifier `virtual:foo/bar`. \
         Got: {msg}"
    );
}

/// **Exact-match — virtual module happy path.**
#[test]
fn virtual_module_matches_exact_specifier() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[exact_match_resolution] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let island_path = write_island_fixture(
        &root,
        "import Foo from \"virtual:foo\";\n\
         export const Counter = () => <Foo />;\n",
        None,
        "",
    );

    let cfg = EsbuildSubprocessConfig {
        extra_args: vec![
            std::ffi::OsString::from("--external:preact"),
            std::ffi::OsString::from("--external:preact/*"),
            std::ffi::OsString::from("--external:@takazudo/*"),
        ],
        ..EsbuildSubprocessConfig::default()
    }
    .with_binary_path(esbuild)
    .with_working_dir(&root)
    .with_virtual_modules(vec![(
        "virtual:foo".to_string(),
        "export default function VirtualFooMarker() { return null; }\n".to_string(),
    )]);
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(root.join("dist"));
    let out = bundler
        .bundle(&[Island::new("Counter", &island_path)], &bundle_cfg)
        .expect("exact-match virtual module `virtual:foo` must resolve");
    // Bundler carries bytes in memory — no disk write.
    let body = String::from_utf8(out.bytes).expect("bundled bytes are valid UTF-8");
    assert!(
        body.contains("VirtualFooMarker"),
        "exact-match virtual-module bundle should contain the loader's \
         exported marker symbol `VirtualFooMarker`."
    );
}
