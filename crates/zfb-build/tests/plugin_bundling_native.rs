//! `.ts`/`.tsx`/`.mts`/`.cts` plugin entries compiled by the native compiler
//! worker and executed by the real Node plugin host (#4633).
//!
//! Every test goes through the production path: `PluginHost` spawns the real
//! `plugin-host.mjs` under `node`, and each entry is compiled by a real
//! worker process (this crate's test-only host executable) before the host
//! imports it. Node is required; nothing here skips.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use zfb_build::{
    bundle_plugin_entry, extract_plugin_error, BuildHookContext, PluginHost, PluginSpec,
    PLUGIN_BUNDLE_TEMP_PREFIX, PLUGIN_BUNDLE_TEMP_SUFFIX,
};
use zfb_compiler_worker::{CompilerWorker, WorkerLauncher};

fn launcher() -> WorkerLauncher {
    WorkerLauncher::new(env!("CARGO_BIN_EXE_zfb-build-test-compiler-worker"))
        .arg(zfb_compiler_worker::PRIVATE_ARG)
}

fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().into()
}

fn spec(name: &str, path: &Path) -> PluginSpec {
    PluginSpec {
        name: name.into(),
        module: file_url(path),
        options: serde_json::json!({}),
    }
}

async fn spawn(specs: Vec<PluginSpec>) -> anyhow::Result<PluginHost> {
    PluginHost::spawn_with_timeout(specs, None, None, Some(launcher())).await
}

fn must<T>(result: anyhow::Result<T>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("{what}: {err:#}"),
    }
}

fn must_fail<T>(result: anyhow::Result<T>, what: &str) -> anyhow::Error {
    match result {
        Ok(_) => panic!("{what}"),
        Err(err) => err,
    }
}

fn ctx(root: &Path) -> BuildHookContext {
    BuildHookContext {
        project_root: root.to_path_buf(),
        scratch_dir: root.join(".zfb-build").join("plugins"),
        out_dir: root.join("dist"),
        config: serde_json::json!({}),
        routes: None,
    }
}

fn js_str(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).unwrap()
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn install_package(root: &Path, name: &str, body: &str) {
    let dir = root.join("node_modules").join(name);
    write(
        &dir.join("package.json"),
        &format!(
            r#"{{"name":"{name}","version":"1.0.0","type":"module","main":"index.mjs","exports":{{".":"./index.mjs","./sub":"./sub.mjs"}}}}"#
        ),
    );
    write(&dir.join("index.mjs"), body);
}

fn staged_bundles(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with(PLUGIN_BUNDLE_TEMP_PREFIX) && name.ends_with(PLUGIN_BUNDLE_TEMP_SUFFIX)
        })
        .map(|e| e.path())
        .collect()
}

fn the_staged_bundle(dir: &Path) -> PathBuf {
    let mut staged = staged_bundles(dir);
    assert_eq!(
        staged.len(),
        1,
        "exactly one staged bundle expected: {staged:?}"
    );
    staged.remove(0)
}

async fn wait_removed(path: &Path) -> bool {
    tokio::time::timeout(Duration::from_secs(5), async {
        while path.exists() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .is_ok()
}

/// No compiler worker scratch file is left beside the entry.
fn assert_no_worker_leftovers(dir: &Path) {
    let leftovers: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".zfb-worker-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

// --- entry kinds ----------------------------------------------------------

#[tokio::test]
async fn ts_entry_with_enum_namespace_and_parameter_properties_runs_through_real_host() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("marker.txt");
    let plugin = tmp.path().join("plugin.ts");
    write(
        &plugin,
        &format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            enum Level {{ Low, High }}
            namespace Labels {{ export const high = "high-marker"; }}
            class Prop {{ constructor(public level: Level) {{}} }}
            export default {{
              name: "ts-constructs",
              preBuild() {{
                const p = new Prop(Level.High);
                writeFileSync({marker}, p.level === Level.High ? Labels.high : "low");
              }},
            }};
            "#,
            marker = js_str(&marker)
        ),
    );
    let host = must(spawn(vec![spec("ts-constructs", &plugin)]).await, "spawn");
    must(host.run_pre_build(&ctx(tmp.path())).await, "preBuild");
    host.shutdown().await.ok();
    assert_eq!(read(&marker), "high-marker");
}

#[tokio::test]
async fn tsx_entry_with_paths_alias_runs_through_real_host() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("tsconfig.json"),
        r#"{"compilerOptions":{"jsx":"react","jsxFactory":"h","baseUrl":".","paths":{"@lib/*":["./lib/*"]}}}"#,
    );
    write(
        &root.join("lib/marker.ts"),
        "export const marker = \"paths-alias-marker\";\n",
    );
    let marker = root.join("marker.txt");
    let plugin = root.join("plugin.tsx");
    write(
        &plugin,
        &format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            import {{ marker }} from "@lib/marker";
            function h(type: string, props: unknown, ...children: unknown[]) {{ return {{ type, children }}; }}
            const Widget = () => <b>{{marker}}</b>;
            export default {{
              name: "tsx-paths",
              preBuild() {{ writeFileSync({out}, JSON.stringify(Widget())); }},
            }};
            "#,
            out = js_str(&marker)
        ),
    );
    let host = must(spawn(vec![spec("tsx-paths", &plugin)]).await, "spawn");
    must(host.run_pre_build(&ctx(root)).await, "preBuild");
    host.shutdown().await.ok();
    let vnode: serde_json::Value = serde_json::from_str(&read(&marker)).unwrap();
    assert_eq!(vnode["type"], "b");
    assert_eq!(vnode["children"][0], "paths-alias-marker");
}

#[tokio::test]
async fn mts_entry_with_an_external_package_subpath_runs_through_real_host() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    install_package(root, "mts-dep", "export const v = 1;\n");
    write(
        &root.join("node_modules/mts-dep/sub.mjs"),
        "export const sub = 31;\n",
    );
    let marker = root.join("marker.txt");
    let plugin = root.join("plugin.mts");
    write(
        &plugin,
        &format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            import {{ sub }} from "mts-dep/sub";
            const n: number = sub;
            export default {{ name: "mts", preBuild() {{ writeFileSync({out}, String(n)); }} }};
            "#,
            out = js_str(&marker)
        ),
    );
    let host = must(spawn(vec![spec("mts", &plugin)]).await, "spawn");
    let bundle = read(&the_staged_bundle(root));
    assert!(
        bundle.contains("mts-dep/sub"),
        "subpath stays external: {bundle}"
    );
    must(host.run_pre_build(&ctx(root)).await, "preBuild");
    host.shutdown().await.ok();
    assert_eq!(read(&marker), "31");
}

#[tokio::test]
async fn cts_entry_using_require_and_export_equals_runs_through_real_host() {
    // A CommonJS-shaped entry bundled to ESM needs a real `require` in the
    // output (the banner); both a builtin and an external package go through
    // it, resolved from the staged bundle's location.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dep = root.join("node_modules/cjs-dep");
    write(
        &dep.join("package.json"),
        r#"{"name":"cjs-dep","version":"1.0.0","main":"index.js"}"#,
    );
    write(
        &dep.join("index.js"),
        "module.exports = { value: 'cjs-dep-marker' };\n",
    );
    let marker = root.join("marker.txt");
    let plugin = root.join("plugin.cts");
    write(
        &plugin,
        &format!(
            r#"
            const {{ writeFileSync }} = require("node:fs");
            const dep = require("cjs-dep");
            const plugin = {{
              name: "cts-require",
              preBuild() {{ writeFileSync({out}, dep.value); }},
            }};
            export = plugin;
            "#,
            out = js_str(&marker)
        ),
    );
    let host = must(
        spawn(vec![spec("cts-require", &plugin)]).await,
        "a .cts entry using top-level require()/export= must load; a \"Dynamic require\" \
         failure means the require banner is missing",
    );
    let bundle = read(&the_staged_bundle(root));
    assert!(
        !bundle.contains("cjs-dep-marker"),
        "the package must stay external: {bundle}"
    );
    must(host.run_pre_build(&ctx(root)).await, "preBuild");
    host.shutdown().await.ok();
    assert_eq!(read(&marker), "cjs-dep-marker");
}

// --- resolution -----------------------------------------------------------

#[tokio::test]
async fn dot_js_import_resolves_to_the_sibling_ts_source() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("helper.ts"),
        "export const helperMarker = \"dot-js-import-marker\";\n",
    );
    let marker = root.join("marker.txt");
    let plugin = root.join("index.ts");
    write(
        &plugin,
        &format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            import {{ helperMarker }} from "./helper.js";
            export default {{ name: "dot-js", preBuild() {{ writeFileSync({out}, helperMarker); }} }};
            "#,
            out = js_str(&marker)
        ),
    );
    let host = must(spawn(vec![spec("dot-js", &plugin)]).await, "spawn");
    must(host.run_pre_build(&ctx(root)).await, "preBuild");
    host.shutdown().await.ok();
    assert_eq!(read(&marker), "dot-js-import-marker");
}

#[tokio::test]
async fn nested_relative_dependencies_are_bundled_not_read_at_runtime() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("a/b.ts"),
        "import { d } from \"../c/d.ts\";\nexport const b = `b+${d}`;\n",
    );
    write(&root.join("c/d.ts"), "export const d = \"d-original\";\n");
    let marker = root.join("marker.txt");
    let plugin = root.join("plugin.ts");
    write(
        &plugin,
        &format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            import {{ b }} from "./a/b";
            export default {{ name: "nested", preBuild() {{ writeFileSync({out}, b); }} }};
            "#,
            out = js_str(&marker)
        ),
    );
    let host = must(spawn(vec![spec("nested", &plugin)]).await, "spawn");
    // A bundled source edited after compilation is not re-read.
    write(&root.join("c/d.ts"), "export const d = \"d-edited\";\n");
    must(host.run_pre_build(&ctx(root)).await, "preBuild");
    host.shutdown().await.ok();
    assert_eq!(read(&marker), "b+d-original");
}

#[tokio::test]
async fn aliases_resolve_before_package_externalization_and_packages_stay_live() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    install_package(root, "fake-dep", "export const dep = \"dep-v1\";\n");
    install_package(
        root,
        "shadowed",
        "export const value = \"installed-shadowed\";\n",
    );
    write(
        &root.join("tsconfig.json"),
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@lib/*":["./lib/*"],"shadowed":["./lib/shadow.ts"]}}}"#,
    );
    write(
        &root.join("lib/local.ts"),
        "export const local = \"local-alias\";\n",
    );
    write(
        &root.join("lib/shadow.ts"),
        "export const value = \"local-shadow\";\n",
    );
    let marker = root.join("marker.json");
    let plugin = root.join("plugin.ts");
    write(
        &plugin,
        &format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            import {{ local }} from "@lib/local";
            import {{ value as shadow }} from "shadowed";
            import {{ dep as atInit }} from "fake-dep";
            export default {{
              name: "aliases",
              async preBuild() {{
                const {{ dep: afterEdit }} = await import("fake-dep/sub");
                const {{ late }} = await import("late-dep");
                writeFileSync({out}, JSON.stringify({{ local, shadow, atInit, afterEdit, late }}));
              }},
            }};
            "#,
            out = js_str(&marker)
        ),
    );
    let host = must(spawn(vec![spec("aliases", &plugin)]).await, "spawn");

    let bundle = read(&the_staged_bundle(root));
    for marker in ["local-alias", "local-shadow"] {
        assert!(
            bundle.contains(marker),
            "{marker} must be bundled: {bundle}"
        );
    }
    for marker in ["dep-v1", "installed-shadowed"] {
        assert!(
            !bundle.contains(marker),
            "{marker} must not be inlined: {bundle}"
        );
    }

    // Edited and installed after bundling: Node must see both, which only
    // happens when the packages stayed external.
    write(
        &root.join("node_modules/fake-dep/sub.mjs"),
        "export const dep = \"dep-v2-after-bundling\";\n",
    );
    install_package(
        root,
        "late-dep",
        "export const late = \"installed-after-bundling\";\n",
    );

    must(host.run_pre_build(&ctx(root)).await, "preBuild");
    host.shutdown().await.ok();
    let seen: serde_json::Value = serde_json::from_str(&read(&marker)).unwrap();
    assert_eq!(seen["local"], "local-alias");
    assert_eq!(
        seen["shadow"], "local-shadow",
        "the alias must win over the installed package"
    );
    assert_eq!(seen["atInit"], "dep-v1");
    assert_eq!(seen["afterEdit"], "dep-v2-after-bundling");
    assert_eq!(seen["late"], "installed-after-bundling");
}

#[tokio::test]
async fn an_edit_to_an_installed_package_after_bundling_reaches_the_plugin() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    install_package(
        root,
        "edited-dep",
        "export const value = \"before-edit\";\n",
    );
    let marker = root.join("marker.txt");
    let plugin = root.join("plugin.ts");
    write(
        &plugin,
        &format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            import {{ value }} from "edited-dep";
            export default {{ name: "edited", preBuild() {{ writeFileSync({out}, value); }} }};
            "#,
            out = js_str(&marker)
        ),
    );
    // Compile first, then edit the package, then start Node on the staged
    // bundle: the import is evaluated only after the edit.
    let worker = CompilerWorker::new(launcher());
    let staged = must(
        bundle_plugin_entry(&plugin, &worker, "edited").await,
        "bundle",
    );
    assert!(!read(staged.path()).contains("before-edit"));
    write(
        &root.join("node_modules/edited-dep/index.mjs"),
        "export const value = \"after-edit\";\n",
    );
    let host = must(
        PluginHost::spawn(
            vec![PluginSpec {
                name: "edited".into(),
                module: staged.file_url().to_string(),
                options: serde_json::json!({}),
            }],
            None,
        )
        .await,
        "spawn on the staged bundle",
    );
    must(host.run_pre_build(&ctx(root)).await, "preBuild");
    host.shutdown().await.ok();
    assert_eq!(read(&marker), "after-edit");
}

#[tokio::test]
async fn the_tsconfig_nearest_to_the_entry_governs_paths_and_jsx() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("tsconfig.json"),
        r#"{"compilerOptions":{"jsx":"react","jsxFactory":"wrongFactory","baseUrl":".","paths":{"@v":["./wrong.ts"]}}}"#,
    );
    write(
        &root.join("wrong.ts"),
        "export const v = \"project-tsconfig\";\n",
    );
    let plugin_dir = root.join("plugins/own");
    write(
        &plugin_dir.join("tsconfig.json"),
        r#"{"compilerOptions":{"jsx":"react","jsxFactory":"h","baseUrl":".","paths":{"@v":["./right.ts"]}}}"#,
    );
    write(
        &plugin_dir.join("right.ts"),
        "export const v = \"entry-tsconfig\";\n",
    );
    let marker = root.join("marker.json");
    let plugin = plugin_dir.join("plugin.tsx");
    write(
        &plugin,
        &format!(
            r#"
            import {{ writeFileSync }} from "node:fs";
            import {{ v }} from "@v";
            function h(type: string) {{ return type; }}
            export default {{
              name: "own-tsconfig",
              preBuild() {{ writeFileSync({out}, JSON.stringify({{ v, jsx: <i /> }})); }},
            }};
            "#,
            out = js_str(&marker)
        ),
    );
    let host = must(spawn(vec![spec("own-tsconfig", &plugin)]).await, "spawn");
    must(host.run_pre_build(&ctx(root)).await, "preBuild");
    host.shutdown().await.ok();
    let seen: serde_json::Value = serde_json::from_str(&read(&marker)).unwrap();
    assert_eq!(seen["v"], "entry-tsconfig");
    assert_eq!(seen["jsx"], "i");
}

// --- staging, source maps ---------------------------------------------------

#[tokio::test]
async fn staged_bundle_sits_next_to_the_entry_so_import_meta_url_reads_siblings() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("sidecar.txt"), "ts-sidecar-marker");
    let marker = root.join("marker.json");
    let plugin = root.join("plugin.ts");
    write(
        &plugin,
        &format!(
            r#"
            import {{ writeFileSync, readFileSync }} from "node:fs";
            import {{ fileURLToPath }} from "node:url";
            import {{ dirname, join }} from "node:path";
            export default {{
              name: "dirname",
              preBuild() {{
                const dir = dirname(fileURLToPath(import.meta.url));
                writeFileSync({out}, JSON.stringify({{ dir, sidecar: readFileSync(join(dir, "sidecar.txt"), "utf8") }}));
              }},
            }};
            "#,
            out = js_str(&marker)
        ),
    );
    let host = must(spawn(vec![spec("dirname", &plugin)]).await, "spawn");
    must(host.run_pre_build(&ctx(root)).await, "preBuild");
    host.shutdown().await.ok();
    let seen: serde_json::Value = serde_json::from_str(&read(&marker)).unwrap();
    assert_eq!(seen["sidecar"], "ts-sidecar-marker");
    assert_eq!(
        std::fs::canonicalize(seen["dir"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(root).unwrap()
    );
    assert_no_worker_leftovers(root);
}

#[tokio::test]
async fn inline_source_map_maps_a_runtime_stack_back_to_the_ts_source() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let marker = root.join("stack.txt");
    let plugin = root.join("throws.ts");
    write(
        &plugin,
        &format!(
            "import {{ writeFileSync }} from \"node:fs\";\n\
             const label: string = \"x\";\n\
             function fail(): never {{\n\
             \x20 throw new Error(\"mapped-\" + label);\n\
             }}\n\
             export default {{\n\
             \x20 name: \"source-map\",\n\
             \x20 preBuild() {{\n\
             \x20   try {{ fail(); }} catch (err) {{ writeFileSync({out}, String((err as Error).stack)); }}\n\
             \x20 }},\n\
             }};\n",
            out = js_str(&marker)
        ),
    );
    let host = must(spawn(vec![spec("source-map", &plugin)]).await, "spawn");
    let bundle = read(&the_staged_bundle(root));
    assert!(
        bundle.contains("//# sourceMappingURL=data:application/json;")
            && bundle.contains(";base64,"),
        "the bundle must carry an inline source map: {bundle}"
    );
    assert!(
        bundle.starts_with("import { createRequire as __zfbCreateRequire }"),
        "the require banner must lead the bundle: {bundle}"
    );
    must(host.run_pre_build(&ctx(root)).await, "preBuild");
    host.shutdown().await.ok();
    let stack = read(&marker);
    assert!(stack.contains("mapped-x"), "{stack}");
    assert!(
        stack.contains("throws.ts:4"),
        "the stack must name the authored line: {stack}"
    );
}

// --- duplicate identity -----------------------------------------------------

#[tokio::test]
async fn duplicate_ts_plugin_specs_share_one_staged_bundle_and_evaluate_once_through_real_host() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let counter = root.join("counter.txt");
    let plugin = root.join("dup.ts");
    write(
        &plugin,
        &format!(
            r#"
            import {{ readFileSync, writeFileSync, existsSync }} from "node:fs";
            const counterPath: string = {counter};
            const prev: number = existsSync(counterPath) ? Number(readFileSync(counterPath, "utf8")) : 0;
            writeFileSync(counterPath, String(prev + 1));
            export default {{ name: "dup-ts", preBuild() {{}} }};
            "#,
            counter = js_str(&counter)
        ),
    );
    let host = must(
        spawn(vec![spec("dup-a", &plugin), spec("dup-b", &plugin)]).await,
        "spawn",
    );
    assert_eq!(
        staged_bundles(root).len(),
        1,
        "two specs naming the identical .ts module must reuse ONE staged bundle"
    );
    must(host.run_pre_build(&ctx(root)).await, "preBuild");
    host.shutdown().await.ok();
    assert_eq!(
        read(&counter),
        "1",
        "the shared staged URL must hit Node's module cache and evaluate once"
    );
}

// --- failures and lifetime ----------------------------------------------------

#[tokio::test]
async fn broken_ts_entry_fails_with_the_compile_error_not_a_plugin_error() {
    let tmp = tempfile::tempdir().unwrap();
    let plugin = tmp.path().join("broken.ts");
    write(
        &plugin,
        "import { nope } from \"./does-not-exist.ts\";\nexport default { name: \"broken\", nope };\n",
    );
    let err = must_fail(
        spawn(vec![spec("broken", &plugin)]).await,
        "a broken .ts entry must fail to compile, never boot the host",
    );
    let msg = err.to_string();
    let prefix = format!(
        "plugin bundling: failed to compile plugin `broken` ({}):",
        plugin.display()
    );
    assert!(
        msg.starts_with(&prefix),
        "expected prefix: {prefix}\ngot: {msg}"
    );
    assert!(
        msg.contains("does-not-exist.ts"),
        "the diagnostic must name the import: {msg}"
    );
    assert!(
        extract_plugin_error(&err).is_none(),
        "a compile failure is never a PluginError: {err:?}"
    );
    assert!(
        staged_bundles(tmp.path()).is_empty(),
        "nothing may stay staged"
    );
    assert_no_worker_leftovers(tmp.path());
}

#[tokio::test]
async fn init_failure_tears_down_the_host_and_removes_the_staged_bundle() {
    let tmp = tempfile::tempdir().unwrap();
    let plugin = tmp.path().join("throws-at-import.ts");
    write(
        &plugin,
        "throw new Error(\"boom-at-import\");\nexport default { name: \"throws-at-import\" };\n",
    );
    let err = must_fail(
        spawn(vec![spec("throws-at-import", &plugin)]).await,
        "a plugin that throws at import time must fail init",
    );
    let plugin_err = extract_plugin_error(&err).expect("an import-time throw is an init failure");
    assert_eq!(plugin_err.hook, "init");
    assert!(
        plugin_err.message.contains("boom-at-import"),
        "{plugin_err:?}"
    );
    for staged in staged_bundles(tmp.path()) {
        assert!(
            wait_removed(&staged).await,
            "the staged bundle must be removed once the host tears down after an init failure"
        );
    }
}

#[tokio::test]
async fn hook_failure_keeps_the_host_and_its_bundle_until_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let plugin = root.join("hook-throws.ts");
    write(
        &plugin,
        "export default { name: \"hook-throws\", preBuild(): void { throw new Error(\"boom-in-hook\"); } };\n",
    );
    let host = must(spawn(vec![spec("hook-throws", &plugin)]).await, "spawn");
    let staged = the_staged_bundle(root);
    let err = must_fail(
        host.run_pre_build(&ctx(root)).await,
        "a throwing preBuild must fail",
    );
    let plugin_err = extract_plugin_error(&err).expect("a hook throw is a PluginError");
    assert_eq!(plugin_err.hook, "preBuild");
    assert!(
        plugin_err.message.contains("boom-in-hook"),
        "{plugin_err:?}"
    );
    assert!(
        staged.exists(),
        "a hook failure must not tear down the live host's bundle"
    );
    host.shutdown().await.ok();
    drop(host);
    assert!(!staged.exists(), "shutdown must remove the staged bundle");
}

#[tokio::test]
async fn staged_bundle_is_locked_and_survives_until_host_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let plugin = root.join("plugin.ts");
    write(&plugin, "export default { name: \"lifetime\" };\n");
    let host = must(spawn(vec![spec("lifetime", &plugin)]).await, "spawn");
    let staged = the_staged_bundle(root);

    // A concurrent process's sweep proves abandonment with an exclusive lock;
    // the live owner's shared lock must refuse it.
    let probe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&staged)
        .unwrap();
    assert!(
        probe.try_lock().is_err(),
        "a live staged bundle must hold its owner's shared lock"
    );
    drop(probe);

    host.shutdown().await.ok();
    assert!(
        staged.exists(),
        "clones alive: the bundle stays until the host drops"
    );
    drop(host);
    assert!(
        !staged.exists(),
        "the staged bundle must be removed once the host is dropped"
    );
    assert_no_worker_leftovers(root);
}

#[tokio::test]
async fn bundling_sweeps_stale_strays_but_spares_live_and_concurrently_owned_bundles() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let stale_at = SystemTime::now() - Duration::from_secs(2 * 3600);
    let named = |tag: &str| {
        root.join(format!(
            "{PLUGIN_BUNDLE_TEMP_PREFIX}{tag}{PLUGIN_BUNDLE_TEMP_SUFFIX}"
        ))
    };

    let stranded = named("stranded");
    write(&stranded, "// leaked by a killed run\n");
    std::fs::File::options()
        .write(true)
        .open(&stranded)
        .unwrap()
        .set_modified(stale_at)
        .unwrap();

    let owned = named("owned");
    write(&owned, "// a suspended concurrent owner's bundle\n");
    let owner = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&owned)
        .unwrap();
    owner.set_modified(stale_at).unwrap();
    owner.lock_shared().unwrap();

    let fresh = named("fresh");
    write(&fresh, "// a concurrent process's fresh bundle\n");

    let plugin = root.join("plugin.ts");
    write(&plugin, "export default { name: \"sweeper\" };\n");
    let host = must(spawn(vec![spec("sweeper", &plugin)]).await, "spawn");

    assert!(!stranded.exists(), "an unlocked stale stray must be swept");
    assert!(
        owned.exists(),
        "a locked bundle must be spared however old it is"
    );
    assert!(fresh.exists(), "a fresh bundle must be spared");
    assert_eq!(
        staged_bundles(root).len(),
        3,
        "owned + fresh + this host's own bundle"
    );

    host.shutdown().await.ok();
    drop(host);
    drop(owner);
    let mut kept = staged_bundles(root);
    kept.sort();
    let mut expected = vec![fresh, owned];
    expected.sort();
    assert_eq!(
        kept, expected,
        "only this host's own bundle is removed at shutdown"
    );
}

#[tokio::test]
async fn a_second_bundle_failure_releases_the_first_specs_staged_bundle() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let good_dir = root.join("good");
    let good = good_dir.join("plugin.ts");
    write(&good, "export default { name: \"good\" };\n");
    let bad = root.join("bad/plugin.ts");
    write(
        &bad,
        "import { x } from \"./missing\";\nexport default { name: \"bad\", x };\n",
    );
    let err = must_fail(
        spawn(vec![spec("good", &good), spec("bad", &bad)]).await,
        "the second entry's compile failure must fail the spawn",
    );
    assert!(err.to_string().contains("plugin `bad`"), "{err:#}");
    assert!(
        staged_bundles(&good_dir).is_empty(),
        "the first spec's staged bundle must be released when spawn fails"
    );
}
