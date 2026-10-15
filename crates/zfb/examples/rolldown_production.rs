//! Internal #318 probe: real production CLI, second rebundle, local adapter, existing V8.
#[cfg(all(feature = "embed_v8", feature = "rolldown-prototype", unix))]
fn main() -> anyhow::Result<()> {
    probe::run()
}
#[cfg(not(all(feature = "embed_v8", feature = "rolldown-prototype", unix)))]
fn main() {
    panic!("requires Unix, embed_v8 and rolldown-prototype");
}
#[cfg(all(feature = "embed_v8", feature = "rolldown-prototype", unix))]
mod probe {
    use anyhow::{ensure, Result};
    use serde_json::{json, Value};
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        time::Instant,
    };
    use zfb_render::{BundleModuleLoader, EmbeddedV8RenderHost, HttpRequestLike, RenderHost};
    fn copy(src: &Path, dst: &Path) -> Result<()> {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let to = dst.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy(&entry.path(), &to)?;
            } else {
                fs::copy(entry.path(), to)?;
            }
        }
        Ok(())
    }
    fn write(root: &Path, name: &str, bytes: impl AsRef<[u8]>) -> Result<()> {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, bytes)?;
        Ok(())
    }
    fn build(
        cli: &Path,
        project: &Path,
        tmp: &Path,
        evidence: &Path,
        label: &str,
        ok: bool,
    ) -> Result<String> {
        if tmp.exists() {
            fs::remove_dir_all(tmp)?;
        }
        fs::create_dir_all(tmp)?;
        let start = Instant::now();
        let out = Command::new(cli)
            .arg("build")
            .current_dir(project)
            .env("TMPDIR", tmp)
            .env("ZFB_KEEP_BUILD_SHADOW", "1")
            .env("ZFB_DEV_TIMING", "1")
            .output()?;
        let cli_elapsed_us = start.elapsed().as_micros();
        fs::write(
            evidence.join(format!("{label}-timing.json")),
            serde_json::to_vec(&json!({"cli_elapsed_us":cli_elapsed_us}))?,
        )?;
        let log = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        fs::write(evidence.join(format!("{label}.log")), &log)?;
        ensure!(
            out.status.success() == ok,
            "{label}: status {}\n{log}",
            out.status
        );
        Ok(log)
    }
    fn metadata(tmp: &Path, basename: &str) -> Result<Value> {
        let mut matches = Vec::new();
        for item in walkdir::WalkDir::new(tmp) {
            let item = item?;
            if item.file_name() != ".zfb-metafile.json" {
                continue;
            }
            let meta: Value = serde_json::from_slice(&fs::read(item.path())?)?;
            if meta["outputs"].as_object().is_some_and(|o| {
                o.keys()
                    .any(|p| Path::new(p).file_name().is_some_and(|n| n == basename))
            }) {
                matches.push(meta);
            }
        }
        ensure!(
            matches.len() == 1,
            "expected one actual {basename} graph, got {}",
            matches.len()
        );
        Ok(matches.remove(0))
    }
    fn graph_has(meta: &Value, needle: &str) -> bool {
        meta["inputs"]
            .as_object()
            .unwrap()
            .keys()
            .any(|p| p.contains(needle))
    }
    fn asset_provenance(meta: &Value, needle: &str) -> bool {
        meta["outputs"].as_object().unwrap().values().any(|v| {
            v["inputs"]
                .as_object()
                .is_some_and(|i| i.keys().any(|p| p.contains(needle)))
        })
    }
    async fn execute(bundle: &Path) -> Result<Value> {
        let source = fs::read_to_string(bundle)?;
        let mut host = EmbeddedV8RenderHost::with_loader(
            BundleModuleLoader::new().with_bundle_asset_root(bundle.parent().unwrap()),
        )?;
        host.execute_module(bundle.file_name().unwrap().to_str().unwrap(), &source)
            .await?;
        let mut responses = serde_json::Map::new();
        for (route, status, marker) in [
            ("/runtime", 200, "SSR_ONLY_318:29:CJS:31"),
            ("/api/admin/a/b", 200, "CATCH_318:a/b:43:31"),
            ("/api/admin/c", 200, "CATCH_318:c:43:31"),
            ("/", 404, ""),
            ("/throw", 500, "production-318-deliberate"),
        ] {
            let out = host
                .dispatch_fetch(HttpRequestLike::get(format!("http://zfb.local{route}")))
                .await?;
            let body = out.body_utf8().unwrap_or_default();
            ensure!(
                out.status == status && body.contains(marker),
                "{} {route}: {} {body}",
                bundle.display(),
                out.status
            );
            if route == "/runtime" {
                ensure!(
                    body.contains("<strong>Prepared runtime MDX 318</strong>"),
                    "MDX: {body}"
                );
                ensure!(body.contains("class=\""), "CSS class: {body}");
            }
            responses.insert(route.into(), json!({"status":out.status,"body":body}));
        }
        Ok(Value::Object(responses))
    }
    pub fn run() -> Result<()> {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let backend = if std::env::var("ZFB_ROLLDOWN_PROTOTYPE").as_deref() == Ok("1") {
            "rolldown"
        } else {
            "esbuild"
        };
        let base = repo.join("target/rolldown-production").join(backend);
        if base.exists() {
            fs::remove_dir_all(&base)?;
        }
        let project = base.join("project");
        let tmp = base.join("tmp");
        let evidence = base.join("evidence");
        fs::create_dir_all(&evidence)?;
        copy(
            &repo.join("prototypes/rolldown/production-fixture"),
            &project,
        )?;
        for dir in ["content", "layouts", "wasm"] {
            fs::create_dir_all(project.join(dir))?;
        }
        let wasm = [
            0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 127, 3, 2, 1, 0, 7, 10, 1, 6, 97, 110,
            115, 119, 101, 114, 0, 0, 10, 6, 1, 4, 0, 65, 17, 11,
        ];
        for (name, value) in [
            ("ssg-only", 17),
            ("runtime-only", 29),
            ("catchall-only", 43),
        ] {
            let mut bytes = wasm;
            bytes[37] = value;
            write(&project, &format!("wasm/{name}.wasm"), bytes)?;
        }
        let (_modules_handle, modules) = zfb::render_pipeline::embedded_node_modules()?;
        fs::create_dir_all(modules.join("@takazudo"))?;
        std::os::unix::fs::symlink(
            repo.join("packages/zfb-adapter-cloudflare"),
            modules.join("@takazudo/zfb-adapter-cloudflare"),
        )?;
        fs::create_dir_all(modules.join(".bin"))?;
        std::os::unix::fs::symlink(
            "../@takazudo/zfb-adapter-cloudflare/bin/cli.mjs",
            modules.join(".bin/zfb-adapter-cloudflare"),
        )?;
        write(
            &modules,
            "legacy-probe/package.json",
            r#"{"name":"legacy-probe","main":"index.cjs"}"#,
        )?;
        write(
            &modules,
            "legacy-probe/index.cjs",
            "module.exports = { value: 31 };\n",
        )?;
        std::os::unix::fs::symlink(&modules, project.join("node_modules"))?;
        let cli = std::env::var_os("ZFB_PROTOTYPE_CLI")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo.join("target/debug/zfb"));
        let bundle = project.join(".zfb-build/bundle-runtime.mjs");
        let inner = project.join("dist/_zfb_inner.mjs");
        let mut prior = None;
        for iteration in 0..2 {
            build(
                &cli,
                &project,
                &tmp,
                &evidence,
                &format!("positive-{iteration}"),
                true,
            )?;
            let bytes = (fs::read(&bundle)?, fs::read(&inner)?);
            if let Some(previous) = &prior {
                ensure!(
                    *previous == bytes,
                    "{backend} second-pass/adapter bytes nondeterministic"
                );
            }
            prior = Some(bytes);
        }
        let full = metadata(&tmp, "bundle.mjs")?;
        let runtime_meta = metadata(&tmp, "bundle-runtime.mjs")?;
        fs::write(
            evidence.join("full-metafile.json"),
            serde_json::to_vec_pretty(&full)?,
        )?;
        fs::write(
            evidence.join("runtime-metafile.json"),
            serde_json::to_vec_pretty(&runtime_meta)?,
        )?;
        for absent in ["pages/index.tsx", "ssg-only.wasm"] {
            ensure!(graph_has(&full, absent), "full graph missing {absent}");
            ensure!(
                !graph_has(&runtime_meta, absent),
                "runtime retained {absent}"
            );
        }
        for retained in [
            "runtime.tsx",
            "runtime-data.ts",
            "RuntimeCard.mdx",
            "[...adminPath].tsx",
            "legacy-probe/index.cjs",
            "runtime-only.wasm",
            "catchall-only.wasm",
            "runtime-glue.zfb-resource.mjs",
        ] {
            ensure!(
                graph_has(&runtime_meta, retained),
                "runtime graph missing {retained}"
            );
        }
        for asset in ["runtime-only.wasm", "catchall-only.wasm"] {
            ensure!(
                asset_provenance(&runtime_meta, asset),
                "output source provenance missing {asset}"
            );
        }
        for (importer, dependency) in [
            ("components/runtime-data.ts", "wasm/runtime-only.wasm"),
            (
                "pages/api/admin/[...adminPath].tsx",
                "wasm/catchall-only.wasm",
            ),
            ("components/runtime-data.ts", "legacy-probe/index.cjs"),
            (
                "pages/api/admin/[...adminPath].tsx",
                "legacy-probe/index.cjs",
            ),
        ] {
            let edges = runtime_meta["inputs"][importer]["imports"]
                .as_array()
                .expect("actual importer graph");
            ensure!(
                edges.iter().any(|edge| edge["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with(dependency))
                    && edge["external"] != true),
                "missing resolved edge {importer} -> {dependency}"
            );
        }
        ensure!(
            !serde_json::to_string(&runtime_meta)?.contains("__ROLLDOWN_COPY_MODULE__"),
            "private copy reference leaked into compatibility graph"
        );
        ensure!(
            runtime_meta.get("zfbBackend").is_some() == (backend == "rolldown"),
            "wrong backend graph"
        );
        let source = fs::read_to_string(&bundle)?;
        ensure!(!source.contains("SSG_ONLY_318"), "SSG code retained");
        ensure!(
            fs::read_to_string(project.join("dist/index.html"))?.contains("SSG_ONLY_318:17"),
            "real SSG render missing"
        );
        ensure!(
            !project.join("dist/runtime.html").exists()
                && !project.join("dist/runtime/index.html").exists(),
            "SSR route prerendered"
        );
        ensure!(
            source.contains("//# sourceMappingURL=bundle-runtime.mjs.map"),
            "runtime linked map missing"
        );
        ensure!(
            !fs::read_to_string(&inner)?.contains("sourceMappingURL="),
            "adapter map stripping contract changed"
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let scratch = rt.block_on(execute(&bundle))?;
        let adapter = rt.block_on(execute(&inner))?;
        let mapped = zfb_build::renderer::prototype_project_error(
            scratch["/throw"]["body"].as_str().unwrap(),
            &bundle.with_extension("mjs.map"),
            &project,
        );
        ensure!(
            mapped
                .as_deref()
                .is_some_and(|m| m.starts_with("pages/throw.tsx:3:")),
            "original second-pass error mapping: {mapped:?}"
        );
        let mut wasm_values = Vec::new();
        for item in walkdir::WalkDir::new(project.join("dist")) {
            let item = item?;
            if item.path().extension().is_some_and(|x| x == "wasm") {
                let bytes = fs::read(item.path())?;
                wasm_values.push(bytes[37]);
                ensure!(
                    fs::read_to_string(project.join("dist/.assetsignore"))?
                        .contains(item.file_name().to_str().unwrap()),
                    "missing assetsignore"
                );
            }
        }
        wasm_values.sort();
        ensure!(
            wasm_values == [29, 43],
            "runtime Wasm manifest: {wasm_values:?}"
        );
        let html = scratch["/runtime"]["body"].as_str().unwrap();
        ensure!(
            html.contains("GLUE_318:42"),
            "ordinary SSR glue execution missing"
        );
        let classes = zfb_css::CssModulesProcessor::new(
            zfb_css::modules::CssModulesConfig::for_project_root(&project),
        )
        .process(&[project.join("pages/runtime.module.css")])?
        .class_maps;
        let class = &classes[&project.join("pages/runtime.module.css")]["hero"];
        ensure!(
            html.contains(&format!("class=\"{class}\"")),
            "actual CSS module class missing: {html}"
        );
        let css = walkdir::WalkDir::new(project.join("dist"))
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "css"))
            .map(|e| fs::read_to_string(e.path()))
            .collect::<std::io::Result<Vec<_>>>()?
            .join("\n");
        ensure!(
            css.contains(class)
                && (css.contains("#0c2238")
                    || css.contains("12,34,56")
                    || css.contains("12, 34, 56")),
            "emitted CSS rule missing: {css}"
        );
        // A real CLI reproduction of the unsupported SSR file-URL contract.
        // Browser islands own this compound file loader; SSR does not install it.
        let page = fs::read_to_string(project.join("pages/runtime.tsx"))?;
        write(
            &project,
            "pages/runtime.tsx",
            page.replace("{ answer as glueAnswer }", "glueURL")
                .replace("glueAnswer()", "glueURL"),
        )?;
        let missing_glue = build(
            &cli,
            &project,
            &tmp,
            &evidence,
            "limitation-ssr-glue-url",
            false,
        )?;
        ensure!(
            missing_glue.contains("default")
                && missing_glue.contains("runtime-glue.zfb-resource.mjs"),
            "unexpected glue limitation: {missing_glue}"
        );
        write(&project, "pages/runtime.tsx", page)?;
        let config = fs::read(project.join("zfb.config.json"))?;
        write(
            &project,
            "zfb.config.json",
            r#"{"adapter":"@takazudo/zfb-adapter-cloudflare","bundle":{"mainFields":["main","module"],"exclude":["components/runtime-data.ts"]}}"#,
        )?;
        let excluded = build(&cli, &project, &tmp, &evidence, "negative-excluded", false)?;
        ensure!(
            excluded.contains("runtime-data") || excluded.contains("@fixture/data"),
            "unactionable exclusion"
        );
        write(&project, "zfb.config.json", config)?;
        write(&project,"pages/unresolved.ts","import absent from 'absent-production-318'; export const prerender=false; export default ()=>absent;\n")?;
        let unresolved = build(
            &cli,
            &project,
            &tmp,
            &evidence,
            "negative-unresolved",
            false,
        )?;
        ensure!(
            unresolved.contains("absent-production-318"),
            "unactionable unresolved import"
        );
        fs::remove_file(project.join("pages/unresolved.ts"))?;
        write(
            &project,
            "pnpm-workspace.yaml",
            "packages:\n  - '.'\n  - 'packages/*'\n",
        )?;
        write(
            &project,
            "packages/child/package.json",
            r#"{"name":"@scope/child","private":true}"#,
        )?;
        write(
            &project,
            "packages/child/index.ts",
            "export const marker='escaped-production-318';",
        )?;
        fs::create_dir_all(modules.join("@scope"))?;
        std::os::unix::fs::symlink(project.join("packages/child"), modules.join("@scope/child"))?;
        write(&project,"pages/escape.ts","import {marker} from '@scope/child'; export const prerender=false; export default ()=>marker;")?;
        let escape = build(
            &cli,
            &project,
            &tmp,
            &evidence,
            "negative-stage-escape",
            false,
        )?;
        ensure!(
            escape.contains("stage-escape audit") || escape.contains("escaped their stage"),
            "not the stage audit: {escape}"
        );
        let report = json!({"backend":backend,"scratch":scratch,"adapter":adapter,"mapped_throw":mapped,"wasm_values":wasm_values,"glue_ssr_module_executed":true,"glue_adapter_handoff":null,"deterministic_second_pass":true,"negatives":"rejected during initial full bundle; second pass not reached"});
        fs::write(
            evidence.join("report.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        println!("production {backend}: real full + runtime rebundle, V8 scratch + adapter, SSG filtering, MDX/CSS/alias/CJS, Wasm, original mapping, graph/provenance, negatives PASS; copied-glue SSR contract unsupported (separate reproduction); evidence={}",evidence.display());
        Ok(())
    }
}
