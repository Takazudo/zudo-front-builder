//! Level-3 build-output regression test for #1164 / #1154 — the
//! `<ClientRouter />` auto-include reachability of a **named** runtime-barrel
//! import in the **production build path**.
//!
//! ## What this proves
//!
//! zfb auto-ships the client-router runtime when a page transitively reaches
//! `<ClientRouter />` (#289 / #307). A *named* import from the runtime barrel
//!
//! ```tsx
//! import { ClientRouter } from "@takazudo/zfb-runtime";
//! ```
//!
//! is a detection trigger: the scanner's `collect_client_router_facts` sets
//! `direct_hit`, which sets `ScanMeta::uses_client_router`, which makes
//! `build.rs` skip the empty-islands short-circuit (build.rs:1076) and thread
//! `BundleConfig::client_router = true` into esbuild (build.rs:1238), so the
//! synthetic islands entry emits `import "@takazudo/zfb-runtime/client-router";`
//! (esbuild.rs:1367) and the dist HTML loads `assets/islands-<hash>.js`.
//!
//! ## Why this test exists at Level 3 (and not only as an InMemoryResolver unit
//! test)
//!
//! Issue #1154 asked whether a *page-reachable* named `ClientRouter` import can
//! be MISSED by the production build path. The scanner already has an
//! `InMemoryResolver` unit test for this shape
//! (`client_router_detected_via_named_import_from_runtime_barrel` in
//! `scanner.rs`), and it passes — but that proves only the in-memory scanner
//! logic. The reported gap is reachability in the **real** build path:
//! `FsResolver`, a real `node_modules/@takazudo/zfb-runtime` tree, the
//! `pages/`-rooted walkdir seeding in `collect_islands_asset`, and esbuild.
//! An isolated unit test could stay green while `zfb build` regresses. So this
//! test drives the actual `zfb` binary as a subprocess against an on-disk
//! fixture — the exact path a downstream consumer (#1152's reporter) hits.
//!
//! ## Determination (#1164)
//!
//! **CASE 1 — it ships.** Auto-include works as designed end-to-end: the
//! zero-islands fixture (a page → SSR-only layout with the named barrel import,
//! NO `"use client"` anywhere) emits `assets/islands-<hash>.js`, the bundle
//! contains the client-router runtime's `init()`, and the dist HTML loads it.
//! No scanner change was needed. This test locks that behaviour in as a
//! build-output regression guard.
//!
//! The complementary InMemoryResolver unit test added for the specific named
//! shape lives in `scanner.rs`
//! (`client_router_named_import_from_barrel_in_ssr_layout_via_resolver`).
//!
//! ## Reachability shapes still NOT covered (documented out-of-scope, by design)
//!
//! `uses_client_router` is computed from the `pages/`-rooted **static import
//! graph**. The following reach `<ClientRouter />` through edges the scanner
//! deliberately does not follow, and remain out of scope (working as designed,
//! consistent with the islands scanner's contract):
//!
//! - **Dynamic `import()`** of a module that pulls in `ClientRouter` — the
//!   scanner walks only static `import` / `export` declarations.
//! - **Plugin-injected entries** — a layout wired in through a plugin's
//!   alias/virtual-module registry rather than a page import.
//! - **Config-referenced entries** — modules referenced only from
//!   `zfb.config.*`, not reachable from any `pages/` file.
//! - **Namespace (`import * as rt`) member access** and **type-only** imports —
//!   intentionally NOT triggers (issue #289 acceptance criterion 2: zero new
//!   bytes for non-users); see the doc comment on `collect_client_router_facts`.
//!
//! ## Fixture shape & node_modules wiring
//!
//! Mirrors `build_terminates.rs` / `css_modules_components_build.rs`: a real
//! `node_modules/` is symlinked to the binary-embedded `@takazudo` tree so
//! `FsResolver`/esbuild can resolve `@takazudo/zfb-runtime` and its
//! `client-router` subpath. Unix-only because the embedded-tree symlink wiring
//! is the established Unix pattern in the sibling tests.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use zfb_test_utils::{locate_esbuild, zfb_binary};

/// Scaffold the zero-islands ClientRouter fixture into `root`.
///
/// - `zfb.config.json` — preact framework.
/// - `pages/index.tsx` — renders the SSR-only layout. NO `"use client"`.
/// - `layouts/main.tsx` — SSR-only layout that does
///   `import { ClientRouter } from "@takazudo/zfb-runtime"` and renders it in
///   `<head>`. NO `"use client"`.
/// - `node_modules/` — symlinked to the extracted embedded `@takazudo` tree so
///   `@takazudo/zfb-runtime` + `/client-router` resolve.
///
/// Returns the `TempDir` handle for the extracted embedded node_modules — it
/// must be kept alive for the duration of the test run.
fn scaffold_client_router_fixture(root: &Path) -> tempfile::TempDir {
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact" }
"#,
    )
    .unwrap();

    // package.json at project root (matches the sibling fixtures; harmless and
    // keeps tooling that sniffs for a workspace root happy).
    fs::write(
        root.join("package.json"),
        r#"{ "name": "client-router-autoinclude-fixture", "version": "0.0.1", "private": true }
"#,
    )
    .unwrap();

    // The page imports the SSR layout. NO `"use client"` anywhere — this is the
    // zero-islands case from #1154.
    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"import { Layout } from "../layouts/main";

export default function HomePage() {
  return (
    <Layout>
      <p>hello from a static, island-free page</p>
    </Layout>
  );
}
"#,
    )
    .unwrap();

    // SSR-only layout: the realistic bootstrap shape #1152's reporter used —
    // a plain NAMED import of ClientRouter from the runtime barrel, rendered in
    // <head>. No `"use client"` directive: detection must fire on a plain
    // reachable SSR module.
    fs::create_dir_all(root.join("layouts")).unwrap();
    fs::write(
        root.join("layouts/main.tsx"),
        r#"import { ClientRouter } from "@takazudo/zfb-runtime";

export function Layout({ children }: { children: unknown }) {
  return (
    <html lang="en">
      <head>
        <title>client-router autoinclude fixture</title>
        <ClientRouter />
      </head>
      <body>{children}</body>
    </html>
  );
}
"#,
    )
    .unwrap();

    // Symlink node_modules/ to the extracted embedded @takazudo tree, exactly
    // as build_terminates.rs / css_modules_components_build.rs do.
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, root.join("node_modules"))
        .expect("symlink node_modules");

    nm_handle
}

/// Recursively collect files under `dir` with the given extension.
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

/// #1164 / #1154 — a zero-islands project whose SSR layout does
/// `import { ClientRouter } from "@takazudo/zfb-runtime"` MUST auto-ship the
/// client-router runtime through the production `zfb build` path.
#[test]
fn named_client_router_import_in_ssr_layout_ships_runtime_with_zero_islands() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[client_router_autoinclude_build] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("create tempdir for client-router fixture");
    let root = tmp.path();
    // `_nm_handle` keeps the extracted embedded node_modules tempdir alive for
    // the duration of the build.
    let _nm_handle = scaffold_client_router_fixture(root);

    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    // Skip gracefully when the embedded runtime is unavailable (e.g. a build
    // without the embed_v8 feature) — same skip pattern as
    // content_snapshot_no_deferred.rs.
    if !output.status.success()
        && (combined.contains("embed_v8") || combined.contains("no esbuild"))
    {
        eprintln!(
            "[client_router_autoinclude_build] zfb build exited non-zero with a \
             known-skip indicator; skipping.\nstdout: {stdout}\nstderr: {stderr}"
        );
        return;
    }

    assert!(
        output.status.success(),
        "expected `zfb build` to succeed for the zero-islands ClientRouter fixture; \
         got status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status,
    );

    let dist = root.join("dist");

    // --- Assertion 1: the islands asset was emitted despite ZERO islands. ---
    //
    // This is the load-bearing #1154 assertion. If the build.rs:1076 empty-
    // islands short-circuit had fired (i.e. `uses_client_router` was false),
    // NO `assets/islands-<hash>.js` would exist.
    let assets_dir = dist.join("assets");
    let js_assets = collect_files(&assets_dir, "js");
    let islands_js: Vec<&PathBuf> = js_assets
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.starts_with("islands-") && n.ends_with(".js"))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        !islands_js.is_empty(),
        "expected an emitted dist/assets/islands-<hash>.js — the named \
         `import {{ ClientRouter }}` in the SSR layout must set \
         `uses_client_router`, bypassing the build.rs:1076 empty-islands \
         short-circuit. None found; js assets present: {js_assets:#?}\n\
         --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
    );

    // --- Assertion 2: the emitted bundle contains the client-router runtime. ---
    //
    // The synthetic islands entry emits `import "@takazudo/zfb-runtime/client-router"`
    // (esbuild.rs:1367), and esbuild bundles that subpath's `init()` source
    // into the chunk. We assert the runtime's distinctive markers are present,
    // proving the runtime actually shipped (not just an empty header-only stub).
    //
    // Both tokens survive esbuild `--minify` (production): `startViewTransition`
    // is a property access on the global `document` (esbuild does not rename DOM
    // property names), and `data-zfb-transition` is a string literal unique to
    // the client-router runtime. Comment-only tokens (e.g. "zfb-router") are
    // stripped by minify, so they are intentionally NOT used here.
    let bundle_blob = islands_js
        .iter()
        .map(|p| fs::read_to_string(p).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        bundle_blob.contains("startViewTransition") || bundle_blob.contains("data-zfb-transition"),
        "the emitted islands bundle must contain the client-router runtime \
         (View Transitions registration / `data-zfb-transition` attribute) — \
         proves the `@takazudo/zfb-runtime/client-router` side-effect import was \
         bundled, not just declared.\n--- bundle (truncated) ---\n{}",
        truncate(&bundle_blob, 1500),
    );

    // --- Assertion 3: the dist HTML loads the islands bundle. ---
    //
    // The renderer references the stable `/assets/islands.js` URL, which the
    // asset-hash rewrite step replaces with the hashed form. So the HTML must
    // carry a `assets/islands-<hash>.js` reference.
    let html_paths = collect_files(&dist, "html");
    assert!(
        !html_paths.is_empty(),
        "no HTML emitted under dist/; expected dist/index.html"
    );
    let html_blob = html_paths
        .iter()
        .map(|p| fs::read_to_string(p).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        html_blob.contains("assets/islands-"),
        "dist HTML must load `assets/islands-<hash>.js` — the client-router \
         runtime is shipped via that bundle. Without auto-include the HTML \
         carries no islands script.\n--- html (truncated) ---\n{}",
        truncate(&html_blob, 1500),
    );
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let cut = (0..=n).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    format!("{}…[truncated]", &s[..cut])
}
