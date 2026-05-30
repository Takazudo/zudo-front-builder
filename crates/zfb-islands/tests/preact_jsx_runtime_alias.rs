//! Real-esbuild end-to-end regression test for issue #633.
//!
//! Since v0.1.0-next.18 the islands esbuild bundler omitted the
//! `react/jsx-runtime` → `preact/jsx-runtime` alias that the main SSR bundler
//! applies (`crates/zfb-build/src/bundler.rs`, `Framework::Preact` arm). next.18 dist modules
//! mint VNodes via a framework-neutral `import { jsx } from "react/jsx-runtime"`;
//! when `clientRouter: true` the islands shared bundle side-effect-imports
//! `@takazudo/zfb-runtime/client-router` (`esbuild.rs::render_shared_bundle_entry_source`),
//! whose `react/jsx-runtime` import is unresolvable in a Preact project (no
//! `react` installed) — so `zfb build` aborts.
//!
//! This test reproduces the **real failing path** — the `client_router=true`
//! shared bundle pulling in `@takazudo/zfb-runtime/client-router` — using a
//! **local on-disk stub** of that dist module (so there is no dependency on the
//! published `@takazudo/zfb-runtime` dist). With the fix the alias rewrites
//! `react/jsx-runtime` to the externalized `preact/jsx-runtime` and the bundle
//! succeeds.
//!
//! ## RED-before-green (verified during implementation)
//!
//! Removing the alias block in `build_esbuild_args` makes this test fail with
//! `esbuild ... Could not resolve "react/jsx-runtime"` (the exact #633 error);
//! restoring it makes the test pass. The two arg-assertion unit tests in
//! `esbuild.rs` (`build_esbuild_args_aliases_react_jsx_runtime_for_preact` /
//! `..._omits_..._for_react`) are the always-run guard regardless of esbuild
//! availability; this test is the behavioral guard that actually exercises
//! esbuild's alias→external resolution.
//!
//! Esbuild gating mirrors the other islands integration tests: it locates a
//! binary via `zfb_test_utils::locate_esbuild()` (env `ZFB_ESBUILD_BIN` →
//! workspace `crates/zfb/binaries/esbuild/` slot → `which esbuild`) and skips
//! with a printed note when none is present.

use std::ffi::OsString;
use std::fs;
use std::path::Path;

use zfb_islands::{
    BundleConfig, ClientBundler, EsbuildSubprocessBundler, EsbuildSubprocessConfig, Island,
};
use zfb_test_utils::locate_esbuild;

/// Lay down a fixture project under `root`:
///
/// - `components/counter.tsx` — one trivial island module (the namespace the
///   shared bundle imports + registers).
/// - `node_modules/@takazudo/zfb-runtime/` — a **local stub** of the dist
///   package whose `client-router` subpath body carries the framework-neutral
///   `import { jsx } from "react/jsx-runtime"` that triggers #633. esbuild must
///   bundle *into* this stub (it is NOT externalized), so it actually
///   encounters — and must resolve — the `react/jsx-runtime` import.
///
/// Returns the absolute path to the island `.tsx` file.
fn write_client_router_fixture(root: &Path) -> std::path::PathBuf {
    fs::create_dir_all(root.join("components")).unwrap();
    let island_path = root.join("components/counter.tsx");
    fs::write(&island_path, "export const Counter = () => null;\n").unwrap();

    let pkg_dir = root.join("node_modules/@takazudo/zfb-runtime");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("package.json"),
        r#"{
  "name": "@takazudo/zfb-runtime",
  "version": "0.0.0-test-stub",
  "type": "module",
  "exports": { "./client-router": "./client-router.js" }
}
"#,
    )
    .unwrap();
    // Mirrors the real next.18 dist: a framework-neutral `react/jsx-runtime`
    // import used to mint a VNode. The top-level `init()` call is an
    // unconditional side effect so esbuild keeps the module (and therefore must
    // resolve its `react/jsx-runtime` import) rather than tree-shaking it away.
    fs::write(
        pkg_dir.join("client-router.js"),
        "import { jsx } from \"react/jsx-runtime\";\n\
         export function init() { return jsx(\"div\", { children: \"client-router\" }); }\n\
         init();\n",
    )
    .unwrap();

    island_path
}

/// Externals for the shared-bundle imports the synthetic entry emits, EXCEPT
/// `@takazudo/zfb-runtime` — that one is deliberately bundled (not external) so
/// esbuild walks into the stub and hits its `react/jsx-runtime` import.
///
/// Note the externals are scoped to `@takazudo/zfb` (and its subpaths), NOT
/// `@takazudo/*` — the latter would externalize the `@takazudo/zfb-runtime`
/// stub and the test would never exercise the failing import.
fn shared_externals() -> Vec<OsString> {
    [
        "--external:preact",
        "--external:preact/*",
        "--external:@takazudo/zfb",
        "--external:@takazudo/zfb/*",
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

/// **#633 regression — Preact + `clientRouter: true` + an island bundles cleanly.**
///
/// The shared islands bundle (`client_router = true`) side-effect-imports the
/// local `@takazudo/zfb-runtime/client-router` stub, whose body imports
/// `react/jsx-runtime`. With the fix the Preact alias rewrites it to the
/// externalized `preact/jsx-runtime`, so esbuild resolves it and the bundle
/// succeeds. Before the fix this errored with `Could not resolve "react/jsx-runtime"`.
#[test]
fn preact_client_router_islands_bundle_resolves_react_jsx_runtime() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[preact_jsx_runtime_alias] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let island_path = write_client_router_fixture(&root);

    let cfg = EsbuildSubprocessConfig {
        extra_args: shared_externals(),
        ..EsbuildSubprocessConfig::default()
    }
    .with_binary_path(esbuild)
    .with_working_dir(&root);

    // Default config is Preact (`jsx_import_source = "preact"`), so the fix's
    // Preact-only alias applies. `with_client_router(true)` makes the shared
    // bundle prepend `import "@takazudo/zfb-runtime/client-router";`.
    let bundle_cfg = BundleConfig::default()
        .with_outdir(root.join("dist"))
        .with_client_router(true);

    let bundler = EsbuildSubprocessBundler::new(cfg);
    let out = bundler
        .bundle(&[Island::new("Counter", &island_path)], &bundle_cfg)
        .expect(
            "issue #633: Preact clientRouter+islands bundle must resolve \
             react/jsx-runtime via the preact alias and succeed",
        );

    let body = fs::read_to_string(&out.asset_path).expect("read bundle");
    assert!(
        !body.is_empty(),
        "bundle output should be non-empty (client-router side-effect import + island)"
    );
    // Prove the alias actually rewrote the target (not just that the build
    // succeeded): the stub's `react/jsx-runtime` import surfaces in the output
    // as the externalized `preact/jsx-runtime`. NB: a naive
    // `!body.contains("react/jsx-runtime")` would be a false guard —
    // `preact/jsx-runtime` contains it as a substring.
    assert!(
        body.contains("preact/jsx-runtime"),
        "alias must rewrite react/jsx-runtime → preact/jsx-runtime in the bundle output"
    );
}
