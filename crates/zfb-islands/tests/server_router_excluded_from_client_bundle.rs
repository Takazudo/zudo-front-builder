//! Real-esbuild build-output regression test for issue #1298.
//!
//! ## The bug
//!
//! In a pnpm workspace, adding the first `"use client"` island made
//! `zfb build` fail with `esbuild: Could not resolve "hono"`. Root cause: the
//! client-facing root barrel of `@takazudo/zfb-runtime` (`src/index.ts`)
//! *value*-re-exported the SSR-only `createPageRouter`, whose module
//! (`router.ts`) does `import { Hono } from "hono"`. So any island importing
//! the bare `@takazudo/zfb-runtime` dragged the Hono server router into the
//! `--platform=browser` graph, and esbuild had to *resolve* `hono` during
//! graph construction — which pnpm does not hoist into the consumer's
//! `node_modules` (it lives only in the nested virtual store).
//!
//! ## The fix these tests guard
//!
//! `createPageRouter` moved to the server-only subpath
//! `@takazudo/zfb-runtime/server`; the root barrel is now client-safe (types
//! only from `./router.js`). An island importing the bare root barrel must
//! bundle **without esbuild ever needing to resolve `hono`**.
//!
//! ## Why Level-3 (build output), not a string assertion
//!
//! zfb is a code generator; a string-equality unit test proves the emit
//! *template* is right, but only running esbuild over a realistic
//! `node_modules` proves the module graph actually excludes the server
//! router. Two cases:
//!
//! - [`root_barrel_import_excludes_server_router_and_hono`] — a stub where
//!   `hono` is *unresolvable* (no `node_modules/hono` anywhere). If the
//!   client graph touched the server router at all, esbuild would fail. The
//!   build succeeding — plus the output containing no `Hono` / `"hono"` /
//!   `createPageRouter` — proves exclusion. These negatives also reject a
//!   hypothetical `--external:hono` "fix" (which would leave `"hono"` in the
//!   bundle as an external import).
//! - [`literal_1298_pnpm_store_layout_islands_build_succeeds`] — the LITERAL
//!   #1298 layout: `hono` present only under the nested pnpm store
//!   (`node_modules/.pnpm/@takazudo+zfb-runtime@x/node_modules/hono`), the
//!   package symlinked to the store realpath, and NO hoisted top-level
//!   `node_modules/hono`. Pre-fix this failed with the exact
//!   `Could not resolve "hono"`; post-fix the islands build succeeds because
//!   the client graph never even attempts to resolve the buried `hono`.
//!
//! Esbuild gating mirrors the other islands integration tests: locate a
//! binary via `zfb_test_utils::locate_esbuild()` and skip with a printed note
//! when none is present (NOT `#[ignore]`).
//!
//! ## RED-before-green (verified manually during implementation)
//!
//! Restoring the pre-fix barrel shape in the stub — a runtime
//! `export { createPageRouter } from "./router.js"` on the barrel with the
//! package's `sideEffects` removed — makes both tests fail with the exact
//! `Could not resolve "hono"` #1298 error. Re-applying the fix makes them
//! pass.

use std::ffi::OsString;
use std::fs;
use std::path::Path;

use zfb_islands::{
    BundleConfig, ClientBundler, EsbuildSubprocessBundler, EsbuildSubprocessConfig, Island,
};
use zfb_test_utils::locate_esbuild;

/// The post-fix `@takazudo/zfb-runtime` stub bodies, shared by both cases.
///
/// The barrel (`index.js`) is the **fixed** shape: it re-exports only the
/// client-safe `navigate` from `./client-router.js`. `createPageRouter` and
/// the `hono` import live behind `./router.js`, reachable ONLY via the
/// `./server` subpath — which an island importing the bare root barrel never
/// touches. `package.json` carries the post-fix `exports` (`.`,
/// `./client-router`, `./server`) and the scoped `sideEffects` allowlist.
struct RuntimeStub;

impl RuntimeStub {
    const PACKAGE_JSON: &'static str = r#"{
  "name": "@takazudo/zfb-runtime",
  "version": "0.0.0-test-stub",
  "type": "module",
  "sideEffects": ["./client-router.js"],
  "exports": {
    ".": "./index.js",
    "./server": "./server.js",
    "./client-router": "./client-router.js"
  }
}
"#;

    /// FIXED root barrel: client-safe re-export only. The pre-fix bug was an
    /// additional `export { createPageRouter } from "./router.js";` here.
    const INDEX_JS: &'static str = "export { navigate } from \"./client-router.js\";\n";

    /// Client-safe surface — no `hono`.
    const CLIENT_ROUTER_JS: &'static str = "export function navigate(_url) {}\n";

    /// SSR-only router — pulls `hono`. Reachable only via `./server`.
    const ROUTER_JS: &'static str =
        "import { Hono } from \"hono\";\nexport function createPageRouter() { return new Hono(); }\n";

    /// The `./server` subpath backing `createPageRouter`.
    const SERVER_JS: &'static str = "export { createPageRouter } from \"./router.js\";\n";

    /// Write the four stub source files + `package.json` under `pkg_dir`.
    fn write_into(pkg_dir: &Path) {
        fs::create_dir_all(pkg_dir).unwrap();
        fs::write(pkg_dir.join("package.json"), Self::PACKAGE_JSON).unwrap();
        fs::write(pkg_dir.join("index.js"), Self::INDEX_JS).unwrap();
        fs::write(pkg_dir.join("client-router.js"), Self::CLIENT_ROUTER_JS).unwrap();
        fs::write(pkg_dir.join("router.js"), Self::ROUTER_JS).unwrap();
        fs::write(pkg_dir.join("server.js"), Self::SERVER_JS).unwrap();
    }
}

/// An island that imports the **bare root barrel** (`@takazudo/zfb-runtime`)
/// and uses its client-safe surface. Because the islands bundler wraps the
/// island with `import * as Mod from "<island>"`, every export (and therefore
/// the barrel edge) is retained.
fn write_island(root: &Path) -> std::path::PathBuf {
    fs::create_dir_all(root.join("components")).unwrap();
    let island_path = root.join("components/counter.tsx");
    fs::write(
        &island_path,
        "import { navigate } from \"@takazudo/zfb-runtime\";\n\
         export const Counter = () => null;\n\
         // Keep the root-barrel edge live for esbuild.\n\
         export function goHome() { return navigate(\"/\"); }\n",
    )
    .unwrap();
    island_path
}

/// Externals scoped to `@takazudo/zfb` (and subpaths) + preact — NOT
/// `@takazudo/zfb-runtime`, which must be bundled so esbuild walks into the
/// stub barrel. Mirrors `preact_jsx_runtime_alias.rs::shared_externals` and
/// the real islands `shared_externals()`.
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

/// Assert the bundle body carries none of the server-router poison. Also
/// rejects an `--external:hono` "fix" (which would leave `"hono"` behind).
fn assert_no_server_router(body: &str) {
    assert!(
        !body.contains("Hono"),
        "client bundle must not contain the Hono class (server router leaked); got:\n{body}"
    );
    assert!(
        !body.contains("hono"),
        "client bundle must not reference \"hono\" — not even as an external import \
         (rejects an --external:hono workaround); got:\n{body}"
    );
    assert!(
        !body.contains("createPageRouter"),
        "client bundle must not contain createPageRouter (server factory leaked); got:\n{body}"
    );
}

/// **#1298 exclusion — no resolvable `hono` at all.**
///
/// The stub has NO `node_modules/hono`. If the client graph reached the
/// server router, esbuild would abort with `Could not resolve "hono"`. The
/// build succeeding, with a bundle free of `Hono` / `"hono"` /
/// `createPageRouter`, proves the root barrel excludes the server router.
#[test]
fn root_barrel_import_excludes_server_router_and_hono() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[server_router_excluded] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // Flat regular-package layout; deliberately NO node_modules/hono.
    RuntimeStub::write_into(&root.join("node_modules/@takazudo/zfb-runtime"));
    let island_path = write_island(root);

    let cfg = EsbuildSubprocessConfig {
        extra_args: shared_externals(),
        ..EsbuildSubprocessConfig::default()
    }
    .with_binary_path(esbuild)
    .with_working_dir(root);

    let bundle_cfg = BundleConfig::default().with_outdir(root.join("dist"));

    let bundler = EsbuildSubprocessBundler::new(cfg);
    let out = bundler
        .bundle(&[Island::new("Counter", &island_path)], &bundle_cfg)
        .expect(
            "issue #1298: an island importing the bare @takazudo/zfb-runtime root barrel \
             must bundle without resolving the server-only hono dependency",
        );

    assert!(!out.bytes.is_empty(), "bundle output should be non-empty");
    let body = String::from_utf8(out.bytes).expect("bundled bytes are valid UTF-8");
    assert_no_server_router(&body);
}

/// **#1298 literal pnpm layout — permanent CI coverage.**
///
/// Reproduces the exact bug-report shape: `hono` reachable only via the
/// nested pnpm store, `node_modules/@takazudo/zfb-runtime` a symlink to the
/// store realpath, and NO hoisted top-level `node_modules/hono`. Pre-fix this
/// failed with `Could not resolve "hono"`; post-fix the islands build
/// succeeds because the client graph never attempts to resolve the buried
/// `hono`. Symlink shape mirrors
/// `npm_dist_islands.rs::pnpm_symlinked_regular_package_use_client_module_is_registered`
/// (#999) and `bundler_workspace_pkg_alias.rs` (#443).
#[cfg(unix)]
#[test]
fn literal_1298_pnpm_store_layout_islands_build_succeeds() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[server_router_excluded/pnpm] no esbuild binary available; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // The real package lives in the pnpm content store (realpath).
    let store = root
        .join("node_modules/.pnpm/@takazudo+zfb-runtime@0.0.0/node_modules/@takazudo/zfb-runtime");
    RuntimeStub::write_into(&store);

    // `hono` is present ONLY nested inside the same store dir — exactly where
    // pnpm isolates a transitive dep, unreachable from the consumer's
    // top-level node_modules. No hoisted `node_modules/hono` is created.
    let hono_dir = root.join("node_modules/.pnpm/@takazudo+zfb-runtime@0.0.0/node_modules/hono");
    fs::create_dir_all(&hono_dir).unwrap();
    fs::write(
        hono_dir.join("package.json"),
        "{ \"name\": \"hono\", \"version\": \"4.0.0-test\", \"type\": \"module\", \"main\": \"./index.js\" }\n",
    )
    .unwrap();
    fs::write(hono_dir.join("index.js"), "export class Hono {}\n").unwrap();

    // node_modules/@takazudo/zfb-runtime -> the store realpath (pnpm's shape).
    let link = root.join("node_modules/@takazudo/zfb-runtime");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&store, &link).expect("symlink pnpm package");

    let island_path = write_island(root);

    let cfg = EsbuildSubprocessConfig {
        extra_args: shared_externals(),
        ..EsbuildSubprocessConfig::default()
    }
    .with_binary_path(esbuild)
    .with_working_dir(root);

    let bundle_cfg = BundleConfig::default().with_outdir(root.join("dist"));

    let bundler = EsbuildSubprocessBundler::new(cfg);
    let out = bundler
        .bundle(&[Island::new("Counter", &island_path)], &bundle_cfg)
        .expect(
            "issue #1298 (literal pnpm layout): the islands build must succeed even though \
             hono is reachable only via the nested pnpm store — the client graph must not \
             attempt to resolve it",
        );

    assert!(!out.bytes.is_empty(), "bundle output should be non-empty");
    let body = String::from_utf8(out.bytes).expect("bundled bytes are valid UTF-8");
    assert_no_server_router(&body);
}
