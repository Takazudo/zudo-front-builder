//! Integration-style tests for the public surface of `zfb-islands` (Sub 2).
//!
//! These exercise the bundler trait, the subprocess engine, and the URL
//! helper. Tests that need the real esbuild binary are gated by `#[ignore]`
//! (`env-gate:`, see CLAUDE.md's taxonomy) — `crates/zfb/build.rs` stages
//! the pinned esbuild binary at `crates/zfb/binaries/esbuild/esbuild` as a
//! side effect of building the `zfb` crate, and `.github/workflows/health.yml`
//! runs this suite with `--ignored` right after asserting that staging step
//! (issue #1337 / #638). Run locally with
//! `ZFB_ESBUILD_BIN=<absolute path to esbuild 0.25.12> cargo test -p \
//! zfb-islands -- --ignored` (the default relative binary_path only
//! resolves when the test process's CWD is the workspace root — it is not,
//! `cargo test` runs test binaries from the package directory).

use std::path::{Path, PathBuf};

use zfb_islands::{
    bundle_link_href, manifest_json, scan_islands, BundleConfig, BundleOutput, ClientBundler,
    EsbuildSubprocessBundler, EsbuildSubprocessConfig, FsResolver, Island, Manifest,
    NativeRustBundler,
};

fn island(name: &str, path: &str) -> Island {
    Island::new(name, PathBuf::from(path))
}

#[test]
fn native_bundler_returns_not_implemented_error() {
    let bundler = NativeRustBundler::new();
    let err = bundler
        .bundle(
            &[island("Counter", "components/counter.tsx")],
            &BundleConfig::default(),
        )
        .expect_err("NativeRustBundler must return an error");
    let msg = format!("{err}");
    assert!(
        msg.contains("not yet implemented"),
        "expected 'not yet implemented' in error, got: {msg}"
    );
}

#[test]
fn subprocess_bundler_mock_short_circuits_command() {
    // Use the mock-output escape hatch so this test does not require the
    // esbuild binary to be present.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg =
        EsbuildSubprocessConfig::default().with_mock_output("export const Counter = () => null;\n");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(tmp.path());
    let out: BundleOutput = bundler
        .bundle(&[island("Counter", "components/counter.tsx")], &bundle_cfg)
        .expect("mock bundler should succeed");
    // Bundler carries bytes in memory — asset_path is the canonical
    // write target but the bundler itself does NOT write to disk.
    assert!(
        out.asset_path.starts_with(tmp.path()),
        "asset_path must be rooted under outdir: {}",
        out.asset_path.display()
    );
    assert!(!out.asset_path.exists(), "bundler must not write to disk");
    let bytes = String::from_utf8(out.bytes.clone()).expect("bytes are valid UTF-8");
    assert!(bytes.contains("Counter"));
    assert_eq!(out.module_ids, vec!["Counter".to_string()]);
    // S0 contract: stable filename + URL, no hash. Production hashing
    // happens in `ProductionAssetPipeline`.
    assert_eq!(out.asset_url, "/assets/islands.js");
    assert_eq!(out.asset_path, tmp.path().join("assets").join("islands.js"));
}

#[test]
fn subprocess_bundler_reports_missing_binary_clearly() {
    let cfg = EsbuildSubprocessConfig::default()
        .with_binary_path("/nonexistent/zfb-esbuild-please-do-not-create");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let err = bundler
        .bundle(&[], &BundleConfig::default())
        .expect_err("missing binary must error");
    let msg = format!("{err}");
    assert!(msg.contains("not found"), "got: {msg}");
}

#[test]
fn bundle_filename_is_stable_regardless_of_payload() {
    // Under the S0 contract the bundler returns a stable `asset_path`
    // canonical form — the bytes vary with input, but the filename and
    // public URL never do. Hashing is the `ProductionAssetPipeline`'s
    // job. The bundler carries bytes in memory; no disk write occurs.
    let make = |payload: &str, root: &Path| {
        let cfg = EsbuildSubprocessConfig::default().with_mock_output(payload);
        let bundler = EsbuildSubprocessBundler::new(cfg);
        let bundle_cfg = BundleConfig::default().with_outdir(root);
        bundler
            .bundle(&[island("X", "components/x.tsx")], &bundle_cfg)
            .expect("bundle")
    };

    let tmp1 = tempfile::tempdir().expect("tempdir");
    let tmp2 = tempfile::tempdir().expect("tempdir");
    let a = make("export const X = 1;\n", tmp1.path());
    let b = make("export const X = 2;\n", tmp2.path());
    assert_eq!(a.asset_path.file_name(), b.asset_path.file_name());
    assert_eq!(a.asset_url, b.asset_url);
    assert_eq!(a.asset_url, "/assets/islands.js");

    // In-memory bytes still differ even though the asset paths share the
    // same filename shape.
    assert_ne!(a.bytes, b.bytes);
}

#[test]
fn bundle_output_layout_is_stable_assets_islands_js() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = EsbuildSubprocessConfig::default().with_mock_output("export {};\n");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(tmp.path());
    let out = bundler
        .bundle(&[island("X", "x.tsx")], &bundle_cfg)
        .expect("bundle");

    // Canonical path layout: {outdir}/assets/islands.js (stable; the
    // bundler does NOT write to disk — the caller owns the write).
    // Hashing is performed downstream by `ProductionAssetPipeline`.
    let parent = out
        .asset_path
        .parent()
        .expect("asset path has a parent")
        .to_path_buf();
    assert_eq!(parent, tmp.path().join("assets"));
    let filename = out
        .asset_path
        .file_name()
        .expect("asset path has a filename")
        .to_string_lossy()
        .into_owned();
    assert_eq!(filename, "islands.js");
    // No disk write — file must not exist.
    assert!(!out.asset_path.exists(), "bundler must not write to disk");
}

#[test]
fn bundle_link_href_derives_public_url_from_asset_path() {
    // The helper itself is content-agnostic — feed it the stable
    // asset path the bundler now emits.
    let p = PathBuf::from("dist/assets/islands.js");
    assert_eq!(bundle_link_href("/", &p), "/assets/islands.js");
    assert_eq!(
        bundle_link_href("https://cdn.example.com", &p),
        "https://cdn.example.com/assets/islands.js"
    );
}

#[test]
fn module_ids_list_preserves_island_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = EsbuildSubprocessConfig::default().with_mock_output("export {};\n");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(tmp.path());
    let out = bundler
        .bundle(
            &[
                island("Counter", "components/counter.tsx"),
                island("Tabs", "components/tabs.tsx"),
            ],
            &bundle_cfg,
        )
        .expect("bundle");
    assert_eq!(
        out.module_ids,
        vec!["Counter".to_string(), "Tabs".to_string()]
    );
}

#[test]
fn asset_url_uses_configured_base_url() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = EsbuildSubprocessConfig::default().with_mock_output("export {};\n");
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default()
        .with_outdir(tmp.path())
        .with_base_url("https://cdn.example.com");
    let out = bundler
        .bundle(&[island("X", "x.tsx")], &bundle_cfg)
        .expect("bundle");
    // Stable filename, configurable base URL.
    assert_eq!(
        out.asset_url, "https://cdn.example.com/assets/islands.js",
        "got: {}",
        out.asset_url
    );
}

#[test]
fn bundle_output_bytes_carries_js_in_memory() {
    // Acceptance criterion for zudolab/zzmod#497: the bundler returns the
    // entry JS in `BundleOutput::bytes` and does NOT write to disk.
    // The duplicate `islands.js` + `islands-<hash>.js` pair in production
    // was caused by the old `bundle()` writing to the stable path as a
    // side effect; Option A removes that write entirely.
    let tmp = tempfile::tempdir().expect("tempdir");
    let payload = "// bundled islands JS\nexport const x = 1;\n";
    let cfg = EsbuildSubprocessConfig::default().with_mock_output(payload);
    let bundler = EsbuildSubprocessBundler::new(cfg);
    let bundle_cfg = BundleConfig::default().with_outdir(tmp.path());
    let out = bundler
        .bundle(&[island("X", "x.tsx")], &bundle_cfg)
        .expect("bundle");

    // Bytes land in memory.
    assert_eq!(out.bytes, payload.as_bytes());
    // Nothing written to disk — the caller (dev server / prod pipeline)
    // owns all disk writes.
    assert!(!out.asset_path.exists(), "bundler must not write to disk");
    assert!(
        !tmp.path().join("assets").exists(),
        "dist/assets/ must not be created by the bundler"
    );
}

#[test]
#[ignore = "env-gate: esbuild binary — cargo test -p zfb-islands -- --ignored \
            (ZFB_ESBUILD_BIN, absolute path, or the staged \
            crates/zfb/binaries/esbuild/esbuild slot; wired into health.yml)"]
fn subprocess_bundler_against_real_binary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Production mode wraps every island in the shared-bundle entry, which
    // imports `mountIslands` from `@takazudo/zfb/runtime` and `h`/`hydrate`/
    // `render` from `preact` (see `shared_bundle_keeps_islands_with_no_top_level_side_effect`
    // below) — esbuild needs those specifiers resolvable via node_modules.
    stage_minimal_node_modules(tmp.path());
    let bundler = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(tmp.path()),
    );

    // The caller would normally pass real island source paths here. For
    // the gated smoke test we ship a one-line ESM file in a temp dir.
    let entry = tmp.path().join("entry.js");
    std::fs::write(&entry, "export const Counter = () => null;\n").expect("write entry");

    let bundle_cfg = BundleConfig::production().with_outdir(tmp.path());
    let out = bundler
        .bundle(&[Island::new("Counter", entry)], &bundle_cfg)
        .expect("real esbuild binary should produce a bundle");
    // Bundler carries bytes in memory — no disk write.
    assert!(!out.asset_path.exists(), "bundler must not write to disk");
    assert!(!out.bytes.is_empty(), "bundled bytes must be non-empty");
}

/// Regression for issue #144 (zudolab/zudo-doc#1355 Wave 5).
///
/// Pre-#144, the production shared-bundle entry was N pure side-effect
/// imports:
///
/// ```ignore
/// import "/abs/.../host-island.tsx";
/// ```
///
/// esbuild ran that with `--bundle --tree-shaking=true`. Modules whose
/// body had no top-level side-effecting statement (the common case for
/// `export default function ComponentName(...) { ... }` — including
/// host-side islands like `SidebarToggle`, `ThemeToggle`, `SidebarTree`
/// in the downstream zudo-doc consumer) got tree-shaken away in their
/// entirety, so the on-disk `dist/assets/islands.js` did not contain
/// the component code the runtime needed for `data-zfb-island="…"`
/// hydration.
///
/// The fix changes the synthesis to namespace-import each island and
/// reference the namespaces from a top-level `mountIslands(...)`
/// invocation; the call is a side effect esbuild MUST preserve, and
/// the namespace references inside the manifest descriptors keep every
/// export of every island alive. (Issue #146 / Wave 6 then leveraged
/// the same anchor as the actual hydration entry-point so SSR'd
/// markers get hydrated end-to-end — the previous
/// `(globalThis).__zfb_islands ??= [...]` shape only kept code alive,
/// it never ran the hydration glue.)
///
/// This test pins the contract end-to-end against the real esbuild
/// binary: write two synthetic island modules — one with a top-level
/// side effect (the v2 shape) and one without (the host shape) —
/// bundle them, and assert BOTH module bodies survive tree-shaking.
/// Pre-fix this test would have the second island missing from the
/// bundle.
#[test]
#[ignore = "env-gate: esbuild binary — cargo test -p zfb-islands -- --ignored \
            (ZFB_ESBUILD_BIN, absolute path, or the staged \
            crates/zfb/binaries/esbuild/esbuild slot; wired into health.yml)"]
fn shared_bundle_keeps_islands_with_no_top_level_side_effect() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Production mode wraps every island in the shared-bundle entry, which
    // imports `mountIslands` from `@takazudo/zfb/runtime` and `h`/`hydrate`/
    // `render` from `preact` — esbuild needs those specifiers resolvable via
    // node_modules.
    stage_minimal_node_modules(tmp.path());
    let bundler = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(tmp.path()),
    );

    // Island A: v2 shape — a top-level `Inner.displayName = "Foo"`
    // assignment. Pre-fix this survived tree-shaking and was the
    // behaviour the regression hid behind.
    let with_effect = tmp.path().join("with-effect.tsx");
    std::fs::write(
        &with_effect,
        r#"
export function WithEffectInner() { return null; }
WithEffectInner.displayName = "WithEffect";
export default WithEffectInner;
"#,
    )
    .expect("write with-effect");

    // Island B: host shape — bare `export default function Foo() {}`,
    // no top-level side effect. Pre-fix esbuild dropped its body
    // entirely from the bundle.
    let no_effect = tmp.path().join("no-effect.tsx");
    std::fs::write(
        &no_effect,
        r#"
export default function NoEffectFn() { return null; }
"#,
    )
    .expect("write no-effect");

    let bundle_cfg = BundleConfig::production()
        .with_outdir(tmp.path())
        // Disable minification so we can grep the output by source-name
        // identifiers rather than mangled symbols.
        .with_minify(false);
    let out = bundler
        .bundle(
            &[
                Island::new("WithEffect", with_effect),
                Island::new("NoEffectFn", no_effect),
            ],
            &bundle_cfg,
        )
        .expect("real esbuild bundle");
    // Bundler carries bytes in memory — no disk write.
    assert!(!out.asset_path.exists(), "bundler must not write to disk");

    let bundled = String::from_utf8(out.bytes).expect("bundled bytes are valid UTF-8");
    assert!(
        bundled.contains("WithEffectInner"),
        "v2-shape island lost from bundle (pre-existing regression?): {bundled}"
    );
    assert!(
        bundled.contains("NoEffectFn"),
        "host-shape island still tree-shaken — issue #144 fix did not land. Bundle:\n{bundled}"
    );
}

/// Stage a minimal `node_modules` under `root` that satisfies the bare
/// imports the synthesized shared-bundle entry emits (`@takazudo/zfb/runtime`
/// and `preact`), so the real-esbuild splitting tests below are hermetic
/// (no dependency on the workspace's own install). Only the symbols the
/// entry imports are stubbed.
fn stage_minimal_node_modules(root: &Path) {
    let nm = root.join("node_modules");

    let zfb_runtime = nm.join("@takazudo/zfb");
    std::fs::create_dir_all(&zfb_runtime).unwrap();
    std::fs::write(
        zfb_runtime.join("package.json"),
        r#"{"name":"@takazudo/zfb","version":"0.0.0","exports":{"./runtime":"./runtime.js"}}"#,
    )
    .unwrap();
    std::fs::write(
        zfb_runtime.join("runtime.js"),
        "export function mountIslands() {}\n",
    )
    .unwrap();

    let preact = nm.join("preact");
    std::fs::create_dir_all(&preact).unwrap();
    std::fs::write(
        preact.join("package.json"),
        r#"{"name":"preact","version":"10.0.0","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(
        preact.join("index.js"),
        "export function h() {}\nexport function hydrate() {}\nexport function render() {}\n",
    )
    .unwrap();
}

/// Acceptance (#806): an island with a dynamic `import()` of a local module
/// must split — the bundler output contains the stable `islands.js` entry
/// PLUS at least one self-hashed chunk, and the dynamically-imported code
/// lives in the chunk, NOT in the entry. Determinism: an identical rebuild
/// produces the same chunk filename(s).
#[test]
#[ignore = "env-gate: esbuild binary — cargo test -p zfb-islands -- --ignored \
            (ZFB_ESBUILD_BIN, absolute path, or the staged \
            crates/zfb/binaries/esbuild/esbuild slot; wired into health.yml)"]
fn splitting_emits_chunk_for_dynamic_import() {
    let tmp = tempfile::tempdir().expect("tempdir");
    stage_minimal_node_modules(tmp.path());

    // The dynamically-imported local module carries a unique marker token
    // we can grep for to prove it landed in a chunk, not the entry.
    std::fs::write(
        tmp.path().join("heavy.js"),
        "export const HEAVY_MARKER = \"zfb_heavy_split_marker\";\n",
    )
    .expect("write heavy");

    // The island source dynamic-imports the heavy module. esbuild resolves
    // `./heavy.js` relative to the island file (same temp dir).
    let island_src = tmp.path().join("island.tsx");
    std::fs::write(
        &island_src,
        "export default function Island() {\n  \
         return import(\"./heavy.js\").then((m) => m.HEAVY_MARKER);\n}\n",
    )
    .expect("write island");

    let bundler = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(tmp.path()),
    );
    let cfg = BundleConfig::production()
        .with_outdir(tmp.path().join("dist"))
        .with_minify(false);

    let out = bundler
        .bundle(&[Island::new("Island", island_src)], &cfg)
        .expect("real esbuild splitting bundle");

    // Entry JS arrives in memory — the bundler does not write to disk.
    assert!(!out.asset_path.exists(), "bundler must not write to disk");
    let entry = String::from_utf8(out.bytes.clone()).expect("entry bytes are valid UTF-8");

    // At least one chunk emitted.
    assert!(
        !out.chunks.is_empty(),
        "dynamic import must produce >=1 chunk; entry was:\n{entry}"
    );
    for chunk in &out.chunks {
        assert!(
            chunk.filename.starts_with("islands-chunk-"),
            "chunk filename must be self-hashed islands-chunk-*: {}",
            chunk.filename
        );
        assert!(
            !chunk.filename.contains('/') && !chunk.filename.contains('\\'),
            "chunk filename must be flat: {}",
            chunk.filename
        );
    }

    // The dynamically-imported code must live in a CHUNK, not the entry.
    assert!(
        !entry.contains("zfb_heavy_split_marker"),
        "dynamically-imported code leaked into the entry:\n{entry}"
    );
    let in_chunk = out
        .chunks
        .iter()
        .any(|c| String::from_utf8_lossy(&c.bytes).contains("zfb_heavy_split_marker"));
    assert!(in_chunk, "dynamically-imported code not found in any chunk");

    // The entry references the chunk via a relative import esbuild baked in.
    assert!(
        entry.contains("./islands-chunk-"),
        "entry must reference the chunk by a relative import:\n{entry}"
    );

    // Determinism: an identical rebuild yields the same chunk filename(s).
    let tmp2 = tempfile::tempdir().expect("tempdir2");
    stage_minimal_node_modules(tmp2.path());
    std::fs::write(
        tmp2.path().join("heavy.js"),
        "export const HEAVY_MARKER = \"zfb_heavy_split_marker\";\n",
    )
    .unwrap();
    let island_src2 = tmp2.path().join("island.tsx");
    std::fs::write(
        &island_src2,
        "export default function Island() {\n  \
         return import(\"./heavy.js\").then((m) => m.HEAVY_MARKER);\n}\n",
    )
    .unwrap();
    let bundler2 = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(tmp2.path()),
    );
    let cfg2 = BundleConfig::production()
        .with_outdir(tmp2.path().join("dist"))
        .with_minify(false);
    let out2 = bundler2
        .bundle(&[Island::new("Island", island_src2)], &cfg2)
        .expect("real esbuild rebuild");

    let mut names1: Vec<&str> = out.chunks.iter().map(|c| c.filename.as_str()).collect();
    let mut names2: Vec<&str> = out2.chunks.iter().map(|c| c.filename.as_str()).collect();
    names1.sort_unstable();
    names2.sort_unstable();
    assert_eq!(
        names1, names2,
        "chunk filenames must be content-hash stable across identical rebuilds"
    );
}

/// Acceptance (#806): an islands set with NO dynamic imports must produce
/// exactly one output file — the entry, zero chunks — so non-splitting
/// projects carry zero new complexity.
#[test]
#[ignore = "env-gate: esbuild binary — cargo test -p zfb-islands -- --ignored \
            (ZFB_ESBUILD_BIN, absolute path, or the staged \
            crates/zfb/binaries/esbuild/esbuild slot; wired into health.yml)"]
fn no_dynamic_import_yields_single_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    stage_minimal_node_modules(tmp.path());

    let island_src = tmp.path().join("island.tsx");
    std::fs::write(
        &island_src,
        "export default function Island() { return null; }\n",
    )
    .expect("write island");

    let bundler = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(tmp.path()),
    );
    let cfg = BundleConfig::production()
        .with_outdir(tmp.path().join("dist"))
        .with_minify(false);

    let out = bundler
        .bundle(&[Island::new("Island", island_src)], &cfg)
        .expect("real esbuild bundle");

    // Entry JS arrives in memory — the bundler does not write to disk.
    assert!(!out.asset_path.exists(), "bundler must not write to disk");
    assert!(!out.bytes.is_empty(), "entry bytes must be non-empty");
    assert!(
        out.chunks.is_empty(),
        "a zero-dynamic-import project must emit exactly the entry, no chunks: {:?}",
        out.chunks.iter().map(|c| &c.filename).collect::<Vec<_>>()
    );
}

// -----------------------------------------------------------------------------
// Client-script entries (#976) — real-esbuild integration
// -----------------------------------------------------------------------------

/// End-to-end for the client-script path against the real esbuild binary:
/// discover a staged `pages/search-widget.client.ts`, bundle it via
/// `build_production_client_scripts`, and assert the production asset
/// carries the stable URL/relative-path shape with every dynamic import
/// inlined (`--splitting=false` — no chunk shipping in v1).
#[test]
#[ignore = "env-gate: esbuild binary — cargo test -p zfb-islands -- --ignored \
            (ZFB_ESBUILD_BIN, absolute path, or the staged \
            crates/zfb/binaries/esbuild/esbuild slot; wired into health.yml)"]
fn client_script_real_esbuild_bundles_discovered_entry() {
    use std::collections::BTreeMap;
    use zfb_islands::{
        build_production_client_scripts_with_workers, discover_client_scripts,
        module_worker_filename, ClientScriptWorkerEntry,
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // A lazily-imported sibling module proves splitting=false: its body
    // must be INLINED into the single output (no chunk emitted).
    let pages = root.join("pages");
    std::fs::create_dir_all(&pages).expect("mkdir pages");
    std::fs::write(
        pages.join("lazy-part.ts"),
        "export const LAZY_MARKER = \"zfb_client_inline_marker\";\n",
    )
    .expect("write lazy module");
    std::fs::write(
        pages.join("search-widget.client.ts"),
        "const q: string = \"zfb_client_entry_marker\";\n\
         new Worker(new URL('./search.worker.ts', import.meta.url), { type: 'module' });\n\
         import(\"./lazy-part.ts\").then((m) => console.log(q, m.LAZY_MARKER));\n",
    )
    .expect("write client entry");
    let worker_path = pages.join("search.worker.ts");
    std::fs::write(
        &worker_path,
        "self.postMessage('zfb_client_worker_marker');\n",
    )
    .expect("write module worker");

    let (entries, collisions) = discover_client_scripts(root).expect("discovery");
    assert!(collisions.is_empty(), "no collisions: {collisions:?}");
    assert_eq!(entries.len(), 1, "exactly one entry: {entries:?}");
    assert_eq!(entries[0].entry_name, "search-widget");

    // The command-layer preprocessing pass applies this exact locked rewrite
    // before handing the staged sources to the islands crate.
    let entry_source = std::fs::read_to_string(&entries[0].source_path).unwrap();
    let rewrite =
        zfb_build::rewrite_module_worker_urls(&entry_source, &entries[0].source_path, root)
            .expect("rewrite worker URL");
    assert!(rewrite.expanded_source.contains(".js?v="));
    std::fs::write(&entries[0].source_path, rewrite.expanded_source).unwrap();
    let worker_filename = module_worker_filename(root, &worker_path).unwrap();
    let workers = BTreeMap::from([(
        "search-widget".to_string(),
        vec![ClientScriptWorkerEntry {
            filename: worker_filename.clone(),
            source_path: worker_path,
        }],
    )]);

    let bundler =
        EsbuildSubprocessBundler::new(EsbuildSubprocessConfig::default().with_working_dir(root));
    let cfg = BundleConfig::production()
        .with_outdir(root.join("dist"))
        .with_minify(false);

    let assets = build_production_client_scripts_with_workers(&bundler, &entries, &workers, &cfg)
        .expect("real esbuild bundle");
    assert_eq!(assets.len(), 1);
    let asset = &assets[0];

    assert_eq!(asset.entry_name, "search-widget");
    assert_eq!(asset.stable_url, "/assets/client/search-widget.js");
    assert_eq!(
        asset.relative_path,
        PathBuf::from("assets/client/search-widget.js")
    );

    let js = String::from_utf8(asset.bytes.clone()).expect("bundle is valid UTF-8");
    assert!(
        js.contains("zfb_client_entry_marker"),
        "entry body missing from bundle:\n{js}"
    );
    assert!(
        js.contains("zfb_client_inline_marker"),
        "dynamic import must be INLINED (splitting=false) — marker missing:\n{js}"
    );
    // TypeScript annotations must be stripped by esbuild's ts loader.
    assert!(
        !js.contains(": string"),
        "TS annotation survived bundling:\n{js}"
    );
    assert!(js.contains(&worker_filename), "worker URL missing:\n{js}");
    assert!(js.contains("?v="), "worker cache query missing:\n{js}");
    assert_eq!(asset.companions.len(), 1);
    assert_eq!(asset.companions[0].filename, worker_filename);
    assert!(
        String::from_utf8_lossy(&asset.companions[0].bytes).contains("zfb_client_worker_marker"),
        "worker bundle marker missing"
    );
}

// -----------------------------------------------------------------------------
// Terminal `?raw` preprocessing (#1499) — real-esbuild integration
// -----------------------------------------------------------------------------

#[test]
#[ignore = "env-gate: esbuild binary — ZFB_ESBUILD_BIN=<absolute path to pinned esbuild> \
            cargo test -p zfb-islands --test integration \
            islands_shadow_raw_import_bundles_text -- --ignored"]
fn islands_shadow_raw_import_bundles_text() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    stage_minimal_node_modules(root);
    std::fs::create_dir_all(root.join("components")).unwrap();
    let importer = root.join("components/shader.tsx");
    std::fs::write(
        &importer,
        "\"use client\";\nimport shader from './demo.frag?raw';\n\
         export function Shader() { return shader; }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("components/demo.frag"),
        "ZFB_RAW_ISLAND_MARKER\nline-two\n",
    )
    .unwrap();

    let expansion = zfb_build::raw_import_expand::expand_raw_imports(
        &std::fs::read_to_string(&importer).unwrap(),
        &importer,
        root,
        &|_| false,
    )
    .unwrap();
    let shadow = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(shadow.path().join("components")).unwrap();
    shadow_link(
        &root.join("node_modules"),
        &shadow.path().join("node_modules"),
    );
    let shadow_importer = shadow.path().join("components/shader.tsx");
    std::fs::write(&shadow_importer, expansion.expanded_source).unwrap();
    for module in expansion.generated_modules {
        std::fs::write(
            shadow.path().join("components").join(module.filename),
            module.source,
        )
        .unwrap();
    }

    let bundler = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(root.to_path_buf()),
    );
    let config = BundleConfig::production()
        .with_outdir(root.join("dist"))
        .with_minify(false)
        .with_preserve_symlinks(true);
    let output = bundler
        .bundle(&[Island::new("Shader", shadow_importer)], &config)
        .expect("raw island bundle");
    let js = String::from_utf8(output.bytes).unwrap();
    assert!(!js.contains("?raw"), "{js}");
    assert!(js.contains("ZFB_RAW_ISLAND_MARKER"), "{js}");
    assert!(js.contains("line-two"), "{js}");
}

#[test]
#[ignore = "env-gate: esbuild binary — ZFB_ESBUILD_BIN=<absolute path to pinned esbuild> \
            cargo test -p zfb-islands --test integration \
            client_script_raw_import_bundles_text -- --ignored"]
fn client_script_raw_import_bundles_text() {
    use zfb_islands::{build_production_client_scripts, ClientScriptEntry};

    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    std::fs::create_dir_all(root.join("pages")).unwrap();
    let importer = root.join("pages/raw.client.ts");
    std::fs::write(
        &importer,
        "import text from './message.txt?raw';\nconsole.log(text);\n",
    )
    .unwrap();
    std::fs::write(
        root.join("pages/message.txt"),
        "ZFB_RAW_CLIENT_MARKER\nsecond-line\n",
    )
    .unwrap();
    let expansion = zfb_build::raw_import_expand::expand_raw_imports(
        &std::fs::read_to_string(&importer).unwrap(),
        &importer,
        root,
        &|_| false,
    )
    .unwrap();
    let stage = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(stage.path().join("pages")).unwrap();
    let staged_entry = stage.path().join("pages/raw.client.ts");
    std::fs::write(&staged_entry, expansion.expanded_source).unwrap();
    for module in expansion.generated_modules {
        std::fs::write(
            stage.path().join("pages").join(module.filename),
            module.source,
        )
        .unwrap();
    }

    let bundler = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(stage.path().to_path_buf()),
    );
    let config = BundleConfig::production()
        .with_outdir(stage.path().join("dist"))
        .with_minify(false);
    let assets = build_production_client_scripts(
        &bundler,
        &[ClientScriptEntry {
            entry_name: "raw".into(),
            source_path: staged_entry,
        }],
        &config,
    )
    .expect("raw client script bundle");
    let js = String::from_utf8(assets[0].bytes.clone()).unwrap();
    assert!(!js.contains("?raw"), "{js}");
    assert!(js.contains("ZFB_RAW_CLIENT_MARKER"), "{js}");
    assert!(js.contains("second-line"), "{js}");
}

// -----------------------------------------------------------------------------
// CSS-import policy (#1395) — real-esbuild integration
//
// Pre-fix, an island importing a `.css` file failed the build: the islands
// esbuild arg set had no `--loader:.css=empty` policy (unlike the SSR
// bundler), so esbuild wrote a sibling CSS output file that
// `read_back_outdir`/`validate_chunk_filename` rejected as "esbuild emitted
// an unexpected output file". These tests pin the fix — plain `.css` and
// `.module.css` imports must both bundle successfully.
// -----------------------------------------------------------------------------

/// Acceptance (#1395): an island importing a plain `.css` file must bundle
/// without error under the `--loader:.css=empty` policy in
/// `build_esbuild_args_with_entry_name`.
#[test]
#[ignore = "env-gate: esbuild binary — cargo test -p zfb-islands -- --ignored \
            (ZFB_ESBUILD_BIN, absolute path, or the staged \
            crates/zfb/binaries/esbuild/esbuild slot; wired into health.yml)"]
fn island_css_import_bundles_without_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    stage_minimal_node_modules(tmp.path());

    std::fs::write(
        tmp.path().join("styles.css"),
        ".zfb_css_import_marker { color: red; }\n",
    )
    .expect("write styles.css");

    let island_src = tmp.path().join("island.tsx");
    std::fs::write(
        &island_src,
        "import \"./styles.css\";\n\
         export default function Island() { return null; }\n",
    )
    .expect("write island");

    let bundler = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(tmp.path()),
    );
    let cfg = BundleConfig::production()
        .with_outdir(tmp.path().join("dist"))
        .with_minify(false);

    let out = bundler
        .bundle(&[Island::new("Island", island_src)], &cfg)
        .expect(
            "island importing a plain .css file must bundle successfully under \
             the --loader:.css=empty policy (issue #1395)",
        );

    // Bundler carries bytes in memory — no disk write.
    assert!(!out.asset_path.exists(), "bundler must not write to disk");
    assert!(!out.bytes.is_empty(), "entry bytes must be non-empty");
    assert!(
        out.chunks.is_empty(),
        "no dynamic import in this fixture — zero chunks expected: {:?}",
        out.chunks.iter().map(|c| &c.filename).collect::<Vec<_>>()
    );

    // `--loader:.css=empty` must have neutralised the CSS bytes — no raw
    // CSS text leaks into the JS entry.
    let entry = String::from_utf8(out.bytes).expect("entry bytes are valid UTF-8");
    assert!(
        !entry.contains("zfb_css_import_marker"),
        "raw CSS bytes leaked into the islands JS bundle:\n{entry}"
    );
}

/// Acceptance (#1395): an island importing a `.module.css` file must also
/// bundle without error. Client-side CSS-modules class maps are OUT of
/// scope for this bundle (see the `esbuild.rs` module doc's "CSS-import
/// policy" section, and #1404/#1406) — `--loader:.css=empty` also matches
/// `.module.css` (no more specific rule is registered for the islands
/// bundle), so the imported default resolves to an empty object rather
/// than a scoped class-name map. This test only pins "does not fail the
/// build"; it deliberately does not assert a class-name map.
#[test]
#[ignore = "env-gate: esbuild binary — cargo test -p zfb-islands -- --ignored \
            (ZFB_ESBUILD_BIN, absolute path, or the staged \
            crates/zfb/binaries/esbuild/esbuild slot; wired into health.yml)"]
fn island_module_css_import_bundles_without_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    stage_minimal_node_modules(tmp.path());

    std::fs::write(
        tmp.path().join("x.module.css"),
        ".zfb_module_css_marker { color: blue; }\n",
    )
    .expect("write x.module.css");

    let island_src = tmp.path().join("island.tsx");
    std::fs::write(
        &island_src,
        "import styles from \"./x.module.css\";\n\
         export default function Island() { return styles; }\n",
    )
    .expect("write island");

    let bundler = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(tmp.path()),
    );
    let cfg = BundleConfig::production()
        .with_outdir(tmp.path().join("dist"))
        .with_minify(false);

    let out = bundler
        .bundle(&[Island::new("Island", island_src)], &cfg)
        .expect(
            "island importing a .module.css file must bundle successfully under \
             the --loader:.css=empty policy (issue #1395); a scoped class-name \
             map is out of scope, see #1404/#1406",
        );

    // Bundler carries bytes in memory — no disk write.
    assert!(!out.asset_path.exists(), "bundler must not write to disk");
    assert!(!out.bytes.is_empty(), "entry bytes must be non-empty");
    assert!(
        out.chunks.is_empty(),
        "no dynamic import in this fixture — zero chunks expected: {:?}",
        out.chunks.iter().map(|c| &c.filename).collect::<Vec<_>>()
    );

    let entry = String::from_utf8(out.bytes).expect("entry bytes are valid UTF-8");
    assert!(
        !entry.contains("zfb_module_css_marker"),
        "raw CSS bytes leaked into the islands JS bundle:\n{entry}"
    );
}

// -----------------------------------------------------------------------------
// Sub-task 3 — manifest emission (acceptance)
//
// The 2-island fixture under `fixtures/two-islands/` is the smallest realistic
// project we can scan: one page imports two distinct `"use client"` modules,
// each exporting a single component. The manifest must surface both with the
// resolved tsx paths.
// -----------------------------------------------------------------------------

fn fixture_root(name: &str) -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.join("fixtures").join(name)
}

/// Regression for issue #122 / #117 — pnpm-workspace consumer shape.
///
/// Reproduces the bug where production builds of a pnpm-workspace
/// consumer ship `data-zfb-island` markers but no client runtime: the
/// scanner used to short-circuit on bare specifiers and `FsResolver`
/// returned `None` for them, so workspace packages whose source `.tsx`
/// carried `"use client"` never made it into the islands set.
///
/// The fixture lays out a consumer with:
///
/// - `pages/home.tsx` importing `@takazudo/zfb-blog-islands` by its
///   scoped bare name.
/// - `workspace/zfb-blog-islands/package.json` with a
///   `"source": "src/index.tsx"` field (the convention pnpm-workspace
///   TypeScript packages use for un-built sources).
/// - `workspace/zfb-blog-islands/src/index.tsx` carrying
///   `"use client"` and exporting two components.
/// - `node_modules/@takazudo/zfb-blog-islands` is set up by the test
///   as a symlink to the workspace package — the codex-review fix on
///   PR #125 narrowed the bare-specifier probe to symlink-shaped
///   `node_modules/<pkg>` entries (i.e. workspace packages), so this
///   regression test mirrors what pnpm produces on disk.
///
/// `scan_islands` against this fixture must yield two islands —
/// `Counter` and `ThemeToggle` — each pointing at the workspace
/// package's source file.
#[cfg(unix)]
#[test]
fn pnpm_workspace_consumer_fixture_yields_workspace_package_islands() {
    let root = fixture_root("pnpm-workspace-consumer");
    // Set up the workspace symlink pnpm would maintain at install time.
    // We do this in the test (not as a checked-in symlink) because
    // checked-in symlinks travel poorly across OSes / git settings.
    let pkg = root.join("workspace/zfb-blog-islands");
    let scope_dir = root.join("node_modules/@takazudo");
    let pkg_link = scope_dir.join("zfb-blog-islands");
    std::fs::create_dir_all(&scope_dir).expect("create node_modules scope dir");
    // Leftover from a prior run? Remove and re-create.
    let _ = std::fs::remove_file(&pkg_link);
    let _ = std::fs::remove_dir_all(&pkg_link);
    std::os::unix::fs::symlink(&pkg, &pkg_link).expect("symlink workspace pkg into node_modules");

    let pages = vec![root.join("pages/home.tsx")];
    let resolver = FsResolver::new();
    let islands = scan_islands(&pages, &resolver).expect("scan");

    let names: Vec<String> = islands.iter().map(|i| i.component_name.clone()).collect();
    assert_eq!(
        names,
        vec!["Counter".to_string(), "ThemeToggle".to_string()],
        "expected exactly Counter + ThemeToggle from workspace package; got {islands:?}",
    );

    // Both islands must point at the same workspace-package source file
    // — proves the scanner walked node_modules and read package.json's
    // `source` field rather than picking up a stray copy somewhere else.
    let expected = pkg
        .join("src/index.tsx")
        .canonicalize()
        .expect("canonicalize workspace island source");
    for island in &islands {
        assert_eq!(island.source_path, expected, "got: {island:?}");
    }
}

#[test]
fn two_islands_fixture_yields_two_entry_manifest() {
    let root = fixture_root("two-islands");
    let pages = vec![root.join("pages/home.tsx")];
    let resolver = FsResolver::new();
    let islands = scan_islands(&pages, &resolver).expect("scan");
    assert_eq!(islands.len(), 2, "got: {islands:?}");

    // FsResolver canonicalises (e.g. `/var` → `/private/var` on macOS).
    // Use the canonical form of the fixture root so `relative_to` can
    // strip the prefix cleanly on every host OS.
    let root = root.canonicalize().expect("canonicalize fixture root");

    // Build the manifest, rebased to the fixture root so paths are
    // portable across machines.
    let manifest = Manifest::from_islands(&islands).relative_to(&root);
    let counter = manifest.get("Counter").expect("Counter present");
    let theme = manifest.get("ThemeToggle").expect("ThemeToggle present");
    assert_eq!(
        counter,
        Path::new("components/counter.tsx"),
        "counter path must be relative to fixture root"
    );
    assert_eq!(
        theme,
        Path::new("components/theme-toggle.tsx"),
        "theme-toggle path must be relative to fixture root"
    );
    // No collisions for distinct component names.
    assert!(manifest.collisions().is_empty());

    // Spot-check the JSON wire format. (Use the canonical root as well.)
    let json = manifest_json(&islands, Some(root.as_path()));
    assert!(json.contains("\"Counter\""));
    assert!(json.contains("\"ThemeToggle\""));
    assert!(json.contains("\"components/counter.tsx\""));
    assert!(json.contains("\"components/theme-toggle.tsx\""));
    // Counter sorts before ThemeToggle alphabetically.
    let cpos = json.find("\"Counter\"").unwrap();
    let tpos = json.find("\"ThemeToggle\"").unwrap();
    assert!(cpos < tpos, "json keys must be sorted: {json}");
}

// ---------------------------------------------------------------------------
// Islands shadow: `import.meta.glob` in an island bundles + executes (#1404)
// ---------------------------------------------------------------------------
//
// These prove the ESBUILD half of the #1385 pt.1 fix (the shadow-tree
// materialisation half lives in `crates/zfb/src/commands/build.rs` and is
// unit-tested there without a binary). The test constructs the shadow tree
// by hand — exactly the shape `materialise_islands_shadow` produces: plain
// island files symlinked, the `import.meta.glob` data module written as a
// REAL expanded copy, `node_modules` symlinked whole — and drives the real
// bundler against it with `--preserve-symlinks`. This is the L3
// "execute-the-artifact" doctrine: string-equality proves the macro was
// removed, and running the bundle under node proves it evaluates (the #1385
// crash was `import.meta.glob` being `undefined` at module-init).

/// Symlink `from` -> `to` for the shadow (unix) / copy-fallback elsewhere.
#[cfg(unix)]
fn shadow_link(from: &Path, to: &Path) {
    std::os::unix::fs::symlink(from, to).expect("symlink");
}
#[cfg(not(unix))]
fn shadow_link(from: &Path, to: &Path) {
    // Windows islands shadows are out of scope (#1404); tests run on unix CI.
    std::fs::copy(from, to).map(|_| ()).expect("copy fallback");
}

/// Build a real project tree + a hand-materialised shadow of it under a fresh
/// tempdir, returning `(project_tempdir, shadow_tempdir, shadow_island_path)`.
/// The glob data module is written EXPANDED in the shadow (mirroring
/// `zfb_build::glob_expand::expand_import_meta_glob`'s output); the raw macro
/// stays only in the real project tree.
fn stage_glob_island_shadow() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    let proj = tempfile::tempdir().expect("proj tempdir");
    let proj_root = proj.path();
    stage_minimal_node_modules(proj_root);
    std::fs::create_dir_all(proj_root.join("components/widgets")).unwrap();
    // Island file (plain — no glob) importing the glob data module + a widget.
    std::fs::write(
        proj_root.join("components/gallery.tsx"),
        "\"use client\";\n\
         import { widgets } from \"./gallery-data\";\n\
         export function Gallery() { return Object.keys(widgets).length; }\n",
    )
    .unwrap();
    // REAL glob data module (raw macro — what the user wrote).
    std::fs::write(
        proj_root.join("components/gallery-data.tsx"),
        "export const widgets = import.meta.glob('./widgets/*.tsx', { eager: true });\n",
    )
    .unwrap();
    std::fs::write(
        proj_root.join("components/widgets/a.tsx"),
        "export const a = 1;\n",
    )
    .unwrap();
    std::fs::write(
        proj_root.join("components/widgets/b.tsx"),
        "export const b = 2;\n",
    )
    .unwrap();

    // Hand-materialised shadow.
    let shadow = tempfile::tempdir().expect("shadow tempdir");
    let shadow_root = shadow.path();
    std::fs::create_dir_all(shadow_root.join("components/widgets")).unwrap();
    shadow_link(
        &proj_root.join("node_modules"),
        &shadow_root.join("node_modules"),
    );
    shadow_link(
        &proj_root.join("components/gallery.tsx"),
        &shadow_root.join("components/gallery.tsx"),
    );
    shadow_link(
        &proj_root.join("components/widgets/a.tsx"),
        &shadow_root.join("components/widgets/a.tsx"),
    );
    shadow_link(
        &proj_root.join("components/widgets/b.tsx"),
        &shadow_root.join("components/widgets/b.tsx"),
    );
    // EXPANDED glob module as a REAL file (mirrors expand_import_meta_glob).
    std::fs::write(
        shadow_root.join("components/gallery-data.tsx"),
        "import * as __glob_0 from \"./widgets/a.tsx\";\n\
         import * as __glob_1 from \"./widgets/b.tsx\";\n\
         export const widgets = {\n\
         \x20 \"./widgets/a.tsx\": __glob_0,\n\
         \x20 \"./widgets/b.tsx\": __glob_1\n\
         };\n",
    )
    .unwrap();

    let shadow_island = shadow_root.join("components/gallery.tsx");
    (proj, shadow, shadow_island)
}

#[test]
#[ignore = "env-gate: esbuild binary — cargo test -p zfb-islands -- --ignored \
            (ZFB_ESBUILD_BIN, absolute path, or the staged \
            crates/zfb/binaries/esbuild/esbuild slot; wired into health.yml)"]
fn islands_shadow_expands_glob_and_executes() {
    let (proj, _shadow, shadow_island) = stage_glob_island_shadow();
    let proj_root = proj.path();
    let out_dir = tempfile::tempdir().expect("outdir");

    let bundler = EsbuildSubprocessBundler::new(
        // working_dir = the REAL project root (entry temp file + entry's own
        // bare imports resolve against the real node_modules); the island's
        // source_path points into the shadow, so its transitive imports
        // resolve through the shadow under --preserve-symlinks.
        EsbuildSubprocessConfig::default().with_working_dir(proj_root.to_path_buf()),
    );
    let bundle_cfg = BundleConfig::production()
        .with_outdir(out_dir.path())
        .with_minify(false)
        .with_preserve_symlinks(true);
    let out = bundler
        .bundle(
            &[Island::new("Gallery", shadow_island.clone())],
            &bundle_cfg,
        )
        .expect("glob island must bundle via the shadow");
    let js = String::from_utf8(out.bytes).expect("bundle is utf-8");

    // The raw Vite macro must be GONE — the shadow's expanded copy won.
    assert!(
        !js.contains("import.meta.glob("),
        "bundle still contains the raw import.meta.glob macro:\n{js}"
    );
    assert!(
        js.contains("Gallery"),
        "island component must survive tree-shaking into the bundle:\n{js}"
    );

    // Execute-the-artifact: the bundle must EVALUATE without throwing. If the
    // macro had leaked (see the load-bearing test below), module-init would
    // throw `TypeError: import.meta.glob is not a function`.
    if node_available() {
        let script_dir = tempfile::tempdir().expect("script dir");
        let bundle_path = script_dir.path().join("bundle.mjs");
        std::fs::write(&bundle_path, js.as_bytes()).unwrap();
        let runner = script_dir.path().join("run.mjs");
        std::fs::write(
            &runner,
            "globalThis.document = { querySelectorAll: () => [], addEventListener: () => {} };\n\
             globalThis.window = globalThis;\n\
             await import(\"./bundle.mjs\");\n\
             console.log(\"ZFB_OK\");\n",
        )
        .unwrap();
        let output = std::process::Command::new("node")
            .arg(&runner)
            .output()
            .expect("spawn node");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("ZFB_OK"),
            "bundle failed to execute under node — glob likely unexpanded.\n\
             status={:?}\nstdout={stdout}\nstderr={stderr}",
            output.status
        );
    } else {
        eprintln!("[islands_shadow] node not on PATH; skipped the execute-the-artifact step.");
    }
}

#[test]
#[ignore = "env-gate: esbuild binary — cargo test -p zfb-islands -- --ignored \
            (ZFB_ESBUILD_BIN, absolute path, or the staged \
            crates/zfb/binaries/esbuild/esbuild slot; wired into health.yml)"]
fn islands_shadow_preserve_symlinks_is_load_bearing() {
    // Same shadow, but bundled WITHOUT --preserve-symlinks: esbuild
    // canonicalises the symlinked island file back to the REAL project tree
    // and resolves `./gallery-data` to the RAW (un-expanded) module — so the
    // macro leaks back into the bundle. This pins WHY the flag is required.
    let (proj, _shadow, shadow_island) = stage_glob_island_shadow();
    let out_dir = tempfile::tempdir().expect("outdir");
    let bundler = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(proj.path().to_path_buf()),
    );
    let bundle_cfg = BundleConfig::production()
        .with_outdir(out_dir.path())
        .with_minify(false)
        .with_preserve_symlinks(false);
    let out = bundler
        .bundle(&[Island::new("Gallery", shadow_island)], &bundle_cfg)
        .expect("bundle (no preserve-symlinks)");
    let js = String::from_utf8(out.bytes).expect("utf-8");
    assert!(
        js.contains("import.meta.glob("),
        "without --preserve-symlinks the raw macro MUST leak (proving the flag is \
         load-bearing); got a bundle without it:\n{js}"
    );
}

/// `true` when a `node` binary is on PATH (for the execute-the-artifact step).
fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
