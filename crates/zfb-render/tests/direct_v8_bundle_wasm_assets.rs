//! Direct-V8 port of the bundle Wasm-asset import tests from
//! `src/embedded_v8/module_loader.rs` (#4393, S6a), run against the default
//! host. The resolver contract test from the same module moved to
//! `zfb-js-host/src/modules.rs`, which owns `BundleModuleLoader`.

#![cfg(all(feature = "direct-v8", not(feature = "embed_v8")))]

use std::path::Path;

use tempfile::TempDir;
use zfb_render::{BundleModuleLoader, EmbeddedV8RenderHost, HttpRequestLike};

// A valid, empty Wasm module: magic, version, and no sections.
const EMPTY_WASM_MODULE: &[u8] = b"\0asm\x01\0\0\0";

fn write_wasm_asset(dir: &TempDir, name: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, EMPTY_WASM_MODULE).expect("write Wasm fixture");
    path
}

fn bundle_importing_wasm(specifier: &str) -> String {
    format!(
        r#"
        import wasm from "{specifier}";
        export default {{
          fetch() {{
            return new Response(
              wasm instanceof WebAssembly.Module ? "module" : "not-module",
              {{ status: 200 }},
            );
          }},
        }};
        "#
    )
}

fn host_for_asset_root(asset_root: &Path) -> EmbeddedV8RenderHost {
    let loader = BundleModuleLoader::new().with_bundle_asset_root(asset_root);
    EmbeddedV8RenderHost::with_loader(loader).expect("host boot")
}

async fn load_failure(asset_root: &Path, source: &str) -> String {
    let mut host = host_for_asset_root(asset_root);
    host.execute_module("bundle.mjs", source)
        .await
        .expect_err("untrusted Wasm import must fail")
        .to_string()
}

#[tokio::test]
async fn bundle_wasm_import_exports_a_webassembly_module() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_wasm_asset(&dir, "tiny.wasm");
    let mut host = host_for_asset_root(dir.path());

    host.execute_module("bundle.mjs", &bundle_importing_wasm("./tiny.wasm"))
        .await
        .expect("bundle should load its copied Wasm asset");
    let response = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");

    assert_eq!(response.status, 200);
    assert_eq!(response.body_utf8(), Some("module"));
}

#[tokio::test]
async fn bundle_wasm_import_percent_decodes_asset_filename() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_wasm_asset(&dir, "tiny module.wasm");
    let mut host = host_for_asset_root(dir.path());

    host.execute_module("bundle.mjs", &bundle_importing_wasm("./tiny%20module.wasm"))
        .await
        .expect("URL-escaped asset filename should load");
    let response = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");

    assert_eq!(response.body_utf8(), Some("module"));
}

#[tokio::test]
async fn in_memory_wasm_module_takes_precedence_over_bundle_assets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let loader = BundleModuleLoader::new().with_bundle_asset_root(dir.path());
    loader.register_module("file:///zfb/in-memory.wasm", r#"export default "memory";"#);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");
    let bundle = r#"
        import wasm from "./in-memory.wasm";
        export default {
          fetch() {
            return new Response(wasm, { status: 200 });
          },
        };
        "#;

    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("in-memory Wasm module should load before a disk lookup");
    let response = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");

    assert_eq!(response.body_utf8(), Some("memory"));
}

#[tokio::test]
async fn bundle_wasm_import_rejects_encoded_traversal_and_absolute_file_urls() {
    let fixture = tempfile::tempdir().expect("tempdir");
    let asset_root = fixture.path().join("assets");
    std::fs::create_dir(&asset_root).expect("asset root");
    let outside = write_wasm_asset(&fixture, "outside.wasm");

    let traversal_error =
        load_failure(&asset_root, &bundle_importing_wasm("./..%2Foutside.wasm")).await;
    assert!(
        traversal_error.contains("escapes the bundle asset root"),
        "percent-decoded traversal must hit the containment guard: {traversal_error}"
    );

    // `url` is not a zfb-render dependency; the fixture path is absolute,
    // so this is the `file://` URL `Url::from_file_path` produces on Unix.
    let absolute_specifier = format!("file://{}", outside.display());
    let absolute_error =
        load_failure(&asset_root, &bundle_importing_wasm(&absolute_specifier)).await;
    assert!(
        absolute_error.contains("no in-memory source"),
        "absolute file URL must retain the no-arbitrary-file-read failure: {absolute_error}"
    );

    let loader = BundleModuleLoader::new().with_bundle_asset_root(&asset_root);
    // An absolute specifier resolves to itself, parsed as a module URL.
    let hosted_file_url = loader
        .resolve("file://asset-host/zfb/outside.wasm", "")
        .expect("hosted file URL");
    assert!(
        loader
            .resolve_bundle_wasm_asset(&hosted_file_url)
            .expect("hosted file URL is not malformed")
            .is_none(),
        "only the exact synthetic file:///zfb/ namespace may load copied Wasm assets"
    );
}

#[tokio::test]
async fn bundle_wasm_import_missing_asset_names_expected_disk_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let expected = dir.path().join("missing.wasm");

    let error = load_failure(dir.path(), &bundle_importing_wasm("./missing.wasm")).await;
    assert!(
        error.contains(&expected.display().to_string()),
        "missing asset error must name the expected disk path `{}`: {error}",
        expected.display()
    );
}
