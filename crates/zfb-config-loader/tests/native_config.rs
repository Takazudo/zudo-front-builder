//! Real `zfb.config.ts` loads: native bundling through a real compiler worker
//! process, then the build's evaluator (V8 by default, `node` on slim builds).
//!
//! The worker host is this crate's `config-loader-test-worker` binary, so
//! these loads are also the non-CLI embedding case: the host executable is
//! not zfb.
use std::path::Path;

use tempfile::TempDir;
use zfb_config_loader::{load_from_ts_file, LoadOptions, LoadedTsConfig, WorkerLauncher};

fn launcher() -> WorkerLauncher {
    WorkerLauncher::new(env!("CARGO_BIN_EXE_config-loader-test-worker"))
        .arg(zfb_compiler_worker::PRIVATE_ARG)
}

fn options() -> LoadOptions {
    LoadOptions {
        compiler_worker: Some(launcher()),
        ..LoadOptions::default()
    }
}

fn project(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (path, source) in files {
        let path = tmp.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    tmp
}

async fn load(dir: &Path, opts: &LoadOptions) -> anyhow::Result<LoadedTsConfig> {
    load_from_ts_file(&dir.join("zfb.config.ts"), dir, opts).await
}

#[tokio::test]
async fn local_relative_imports_and_helper_closures_are_bundled() {
    let tmp = project(&[
        (
            "zfb.config.ts",
            "import { siteBase, collection } from \"./config/site\";\n\
             export default { base: siteBase, collections: [collection(\"blog\")] };\n",
        ),
        (
            "config/site.ts",
            "import { prefix } from \"../shared/prefix.js\";\n\
             export const siteBase = prefix(\"docs\");\n\
             export function collection(name: string) {\n\
               const dir = (n: string) => `content/${n}`;\n\
               return { name, path: dir(name) };\n\
             }\n",
        ),
        (
            "shared/prefix.js",
            "export const prefix = (s) => `/${s}/`;\n",
        ),
    ]);
    let loaded = load(tmp.path(), &options()).await.expect("config loads");
    assert_eq!(loaded.config["base"], "/docs/");
    assert_eq!(loaded.config["collections"][0]["name"], "blog");
    assert_eq!(loaded.config["collections"][0]["path"], "content/blog");
    assert!(loaded.resolved_plugins.is_empty());
}

#[tokio::test]
async fn both_helper_aliases_resolve_to_the_staged_stub() {
    let tmp = project(&[
        (
            "zfb.config.ts",
            "import { defineConfig } from \"zfb/config\";\n\
             import preset from \"./preset\";\n\
             export default defineConfig({ port: 4321, presets: [preset] });\n",
        ),
        (
            "preset.ts",
            "import { definePreset } from \"@takazudo/zfb/config\";\n\
             export default definePreset(\"@scope/preset\", { plugins: [{ name: \"./p.mjs\" }] });\n",
        ),
    ]);
    let opts = LoadOptions {
        resolve_plugins: false,
        ..options()
    };
    let loaded = load(tmp.path(), &opts).await.expect("config loads");
    assert_eq!(loaded.config["port"], 4321);
    let plugin = &loaded.config["presets"][0]["plugins"][0];
    assert_eq!(plugin["name"], "./p.mjs");
    assert_eq!(plugin["source_package"], "@scope/preset");
}

#[tokio::test]
async fn typescript_only_syntax_is_compiled() {
    let tmp = project(&[(
        "zfb.config.ts",
        "interface Site { title: string; port?: number }\n\
         type Mode = \"static\" | \"hybrid\";\n\
         enum Size { Small = 1, Large = 2 }\n\
         const mode: Mode = \"static\";\n\
         const site = { title: \"t\", port: 3000 } satisfies Site;\n\
         function pick<T extends object, K extends keyof T>(o: T, k: K): T[K] { return o[k]; }\n\
         export default { output: mode, port: pick(site, \"port\")!, size: Size.Large as number };\n",
    )]);
    let loaded = load(tmp.path(), &options()).await.expect("config loads");
    assert_eq!(loaded.config["output"], "static");
    assert_eq!(loaded.config["port"], 3000);
    assert_eq!(loaded.config["size"], 2);
}

#[tokio::test]
async fn syntax_error_surfaces_the_native_diagnostic_with_the_config_path() {
    let tmp = project(&[("zfb.config.ts", "export default { port: 3000,, };\n")]);
    let ts_path = tmp.path().join("zfb.config.ts");
    let message = format!("{:#}", load(tmp.path(), &options()).await.unwrap_err());
    assert!(message.contains("failed to bundle"), "{message}");
    assert!(
        message.contains(ts_path.to_str().unwrap()),
        "names the config path: {message}"
    );
    assert!(
        message.contains("zfb.config.ts:1"),
        "has a location: {message}"
    );
}

#[tokio::test]
async fn unresolved_local_import_surfaces_the_native_diagnostic() {
    let tmp = project(&[(
        "zfb.config.ts",
        "import missing from \"./missing\";\nexport default { port: missing };\n",
    )]);
    let message = format!("{:#}", load(tmp.path(), &options()).await.unwrap_err());
    assert!(message.contains("failed to bundle"), "{message}");
    assert!(
        message.contains("Could not resolve \"./missing\""),
        "{message}"
    );
}

#[cfg(feature = "direct-v8")]
#[tokio::test]
async fn node_builtin_imports_are_rejected_before_evaluation() {
    for (specifier, source) in [
        (
            "node:fs",
            "import fs from \"node:fs\";\nexport default { files: fs.readdirSync(\".\") };\n",
        ),
        (
            "path",
            "import { join } from \"path\";\nexport default { outDir: join(\"a\", \"b\") };\n",
        ),
    ] {
        let tmp = project(&[("zfb.config.ts", source)]);
        let message = format!("{:#}", load(tmp.path(), &options()).await.unwrap_err());
        assert!(
            message.contains("must be self-contained data config"),
            "{specifier}: {message}"
        );
        assert!(
            message.contains(&format!("imports {specifier}")),
            "{specifier}: {message}"
        );
        assert!(
            !message.contains("embedded V8 evaluator failed"),
            "{specifier}: rejected before evaluation: {message}"
        );
    }
}

#[cfg(not(feature = "direct-v8"))]
#[tokio::test]
async fn slim_node_evaluator_keeps_node_builtins_available() {
    let tmp = project(&[(
        "zfb.config.ts",
        "import { join } from \"node:path\";\nexport default { outDir: join(\"a\", \"b\") };\n",
    )]);
    let loaded = load(tmp.path(), &options()).await.expect("config loads");
    assert_eq!(loaded.config["outDir"], "a/b");
}

#[tokio::test]
async fn missing_launcher_is_an_actionable_error() {
    let tmp = project(&[("zfb.config.ts", "export default {};\n")]);
    let message = format!(
        "{:#}",
        load(tmp.path(), &LoadOptions::default()).await.unwrap_err()
    );
    assert!(message.contains("zfb.config.ts"), "{message}");
    assert!(message.contains("WorkerLauncher"), "{message}");
    assert!(message.contains("run_if_requested"), "{message}");
    assert!(
        message.contains("LoadOptions::compiler_worker"),
        "{message}"
    );
}

#[tokio::test]
async fn resolve_plugins_controls_plugin_resolution() {
    let tmp = project(&[
        (
            "zfb.config.ts",
            "export default { outDir: \"out\", plugins: [{ name: \"./plugins/local.mjs\" }, \
             { name: \"@absent/zfb-plugin-ghost\" }] };\n",
        ),
        ("plugins/local.mjs", "export default {};\n"),
    ]);

    let skipped = LoadOptions {
        resolve_plugins: false,
        ..options()
    };
    let loaded = load(tmp.path(), &skipped)
        .await
        .expect("resolution skipped");
    assert!(loaded.resolved_plugins.is_empty());
    assert_eq!(loaded.config["outDir"], "out");

    let message = format!("{:#}", load(tmp.path(), &options()).await.unwrap_err());
    assert!(message.contains("@absent/zfb-plugin-ghost"), "{message}");

    std::fs::write(
        tmp.path().join("zfb.config.ts"),
        "export default { plugins: [{ name: \"./plugins/local.mjs\" }] };\n",
    )
    .unwrap();
    let loaded = load(tmp.path(), &options())
        .await
        .expect("local plugin resolves");
    assert_eq!(loaded.resolved_plugins.len(), 1);
    assert!(
        loaded.resolved_plugins[0].starts_with("file://")
            && loaded.resolved_plugins[0].ends_with("/plugins/local.mjs"),
        "{:?}",
        loaded.resolved_plugins
    );
}
