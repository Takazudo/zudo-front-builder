//! Typed requests (#4629) drive the same planner the legacy argv decoder
//! feeds, and `generate` publishes nothing until the caller asks.
use crate::request::*;
use std::{fs, path::PathBuf, process::Command};

fn temp_dir(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "zfb-typed-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

#[test]
fn typed_and_argv_requests_plan_identically() {
    let mut cmd = Command::new("unused");
    cmd.current_dir("/srv/site").args([
        "pages/index.tsx",
        "--bundle",
        "--format=esm",
        "--platform=neutral",
        "--target=es2022",
        "--outdir=out",
        "--entry-names=[name]",
        "--chunk-names=c-[hash]",
        "--asset-names=a-[name]-[hash]",
        "--splitting",
        "--minify",
        "--keep-names",
        "--sourcemap=linked",
        "--preserve-symlinks",
        "--jsx=automatic",
        "--jsx-import-source=preact",
        "--conditions=worker,browser",
        "--main-fields=module,main",
        "--alias:#a=./a.mjs",
        "--define:X=\"1\"",
        "--loader:.svg=file",
        "--loader:.txt=text",
        "--loader:.wasm=copy",
        "--external:node:*",
        "--external:pkg",
        "--metafile=out/meta.json",
    ]);
    cmd.env("NODE_PATH", "/srv/hoist");
    let decoded = super::prepared::decode_argv_for_tests(&cmd).unwrap();
    let typed = BundleRequest {
        cwd: "/srv/site".into(),
        entry: "pages/index.tsx".into(),
        output: OutputLayout::Dir {
            dir: "out".into(),
            entry_names: Some("[name]".into()),
            chunk_names: Some("c-[hash]".into()),
            asset_names: Some("a-[name]-[hash]".into()),
        },
        format: Format::Esm,
        platform: Platform::Neutral,
        target: Some("es2022".into()),
        jsx: Jsx {
            runtime: Some(JsxRuntime::Automatic),
            import_source: Some("preact".into()),
        },
        tsconfig: None,
        resolve: Resolve {
            aliases: vec![("#a".into(), "./a.mjs".into())],
            conditions: Some(vec!["worker".into(), "browser".into()]),
            main_fields: Some(vec!["module".into(), "main".into()]),
            preserve_symlinks: true,
            extra_module_dirs: vec!["/srv/hoist".into()],
        },
        defines: vec![("X".into(), "\"1\"".into())],
        loaders: vec![
            (".svg".into(), Loader::File),
            (".txt".into(), Loader::Text),
            (".wasm".into(), Loader::Copy),
        ],
        externals: vec![
            External::Prefix("node:".into()),
            External::Package("pkg".into()),
        ],
        splitting: true,
        minify: true,
        keep_names: true,
        source_map: SourceMap::Linked,
        graph: Some("out/meta.json".into()),
    };
    assert_eq!(decoded, typed);
}

#[test]
fn generate_publishes_nothing_until_asked_and_reports_the_graph() {
    let dir = temp_dir("generate");
    fs::write(dir.join("dep.mjs"), "export const v = 'DEP_VALUE';\n").unwrap();
    fs::write(
        dir.join("entry.mjs"),
        "import { v } from '#dep';\nimport fs from 'node:fs';\nconsole.log(v, fs);\n",
    )
    .unwrap();
    let mut request = BundleRequest::new(
        &dir,
        "entry.mjs",
        OutputLayout::File("out/bundle.mjs".into()),
        Platform::Neutral,
    );
    request
        .resolve
        .aliases
        .push(("#dep".into(), "./dep.mjs".into()));
    request.externals.push(External::Prefix("node:".into()));
    request.graph = Some("out/meta.json".into());
    let generated = crate::generate(&request).unwrap();
    assert!(!dir.join("out").exists(), "generate must not write");
    let bundle = String::from_utf8(
        generated
            .file(&dir.join("out/bundle.mjs"))
            .expect("bundle generated")
            .to_vec(),
    )
    .unwrap();
    assert!(bundle.contains("DEP_VALUE"), "{bundle}");
    assert!(bundle.contains("node:fs"), "{bundle}");
    assert_eq!(generated.graph["zfbBackend"], BACKEND_IDENTITY);
    let imports = generated.graph["inputs"]["entry.mjs"]["imports"]
        .as_array()
        .unwrap();
    assert_eq!(imports[0]["path"], "dep.mjs");
    assert_eq!(imports[1]["path"], "node:fs");
    assert_eq!(imports[1]["external"], true);
    generated.publish().unwrap();
    assert!(dir.join("out/bundle.mjs").is_file());
    assert!(dir.join("out/meta.json").is_file());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn raw_css_is_rejected_with_the_supported_path() {
    let dir = temp_dir("css");
    fs::write(dir.join("a.css"), "a{color:red}\n").unwrap();
    fs::write(dir.join("entry.mjs"), "import './a.css';\n").unwrap();
    let mut request = BundleRequest::new(
        &dir,
        "entry.mjs",
        OutputLayout::File("out/bundle.mjs".into()),
        Platform::Browser,
    );
    request.loaders.push((".css".into(), Loader::Css));
    let error = format!("{:#}", crate::generate(&request).unwrap_err());
    assert!(
        error.contains("ZFB_ROLLDOWN_RAW_CSS_UNSUPPORTED"),
        "{error}"
    );
    assert!(error.contains("*.module.css"), "{error}");
    assert!(error.contains("Wind"), "{error}");
    assert!(error.contains("Lightning CSS"), "{error}");
    assert!(!error.contains("esbuild"), "{error}");
    assert!(!error.contains("ZFB_BUNDLER"), "{error}");
    assert!(!dir.join("out").exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn splitting_requires_a_directory_output() {
    let mut request = BundleRequest::new(
        "/srv",
        "e.mjs",
        OutputLayout::File("o.mjs".into()),
        Platform::Browser,
    );
    request.splitting = true;
    let error = format!("{:#}", crate::generate(&request).unwrap_err());
    assert!(error.contains("requires a directory output"), "{error}");
}

#[test]
fn every_loader_name_round_trips() {
    for name in [
        "js", "jsx", "ts", "tsx", "json", "text", "base64", "dataurl", "binary", "empty", "file",
        "copy", "css",
    ] {
        assert_eq!(Loader::parse(name).map(Loader::name), Some(name));
    }
    assert_eq!(Loader::parse("asset"), None);
}
