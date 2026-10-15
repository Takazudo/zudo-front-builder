//! #318 L3 runtime probe. Run with embed_v8 and rolldown-prototype features.
#[cfg(all(feature = "embed_v8", feature = "rolldown-prototype"))]
fn main() -> anyhow::Result<()> {
    probe::run()
}
#[cfg(not(all(feature = "embed_v8", feature = "rolldown-prototype")))]
fn main() {
    panic!("requires embed_v8,rolldown-prototype");
}
#[cfg(all(feature = "embed_v8", feature = "rolldown-prototype"))]
mod probe {
    use anyhow::{ensure, Result};
    use std::{
        path::{Path, PathBuf},
        time::Instant,
    };
    use zfb_build::{bundle, BundleMode, BundlerInput};
    use zfb_render::{BundleModuleLoader, EmbeddedV8RenderHost, HttpRequestLike, RenderHost};
    fn input(root: &Path, backend: &str) -> BundlerInput {
        let mut input = BundlerInput::for_project(
            root.to_path_buf(),
            BundleMode::Development,
            root.parent().unwrap().join(format!("ssr-{backend}")),
            None,
        );
        input.zudo_react_island_names = Some(vec!["Counter".into()]);
        input.node_modules_dir = Some(root.join("node_modules"));
        input.esbuild_binary = Some(
            std::env::var_os("ZFB_ESBUILD_BIN")
                .expect("ZFB_ESBUILD_BIN")
                .into(),
        );
        input.tsconfig_paths.insert(
            "@fixture/message".into(),
            vec![root
                .join("components/message.ts")
                .to_string_lossy()
                .into_owned()],
        );
        input.css_module_class_maps = zfb_css::CssModulesProcessor::new(
            zfb_css::modules::CssModulesConfig::for_project_root(root),
        )
        .process(&[root.join("pages/page.module.css")])
        .expect("CSS preparation")
        .class_maps;
        input
    }
    pub fn run() -> Result<()> {
        let root = PathBuf::from(std::env::args().nth(1).expect("project root")).canonicalize()?;
        let backend = if std::env::var("ZFB_ROLLDOWN_PROTOTYPE").as_deref() == Ok("1") {
            "rolldown"
        } else {
            "esbuild"
        };
        let mut prior = None;
        let mut timings = Vec::new();
        let mut last = None;
        for _ in 0..3 {
            let start = Instant::now();
            let out = bundle(input(&root, backend))?;
            timings.push(start.elapsed().as_micros());
            let bytes = std::fs::read(&out.bundle_path)?;
            if let Some(ref prior) = prior {
                ensure!(*prior == bytes, "SSR bytes nondeterministic");
            }
            prior = Some(bytes);
            let deps = format!("{:?}", out.route_module_deps);
            ensure!(
                deps.contains("message-dep.ts"),
                "transitive graph missing: {deps}"
            );
            ensure!(
                deps.contains("Hero.mdx"),
                "original MDX graph mapping missing: {deps}"
            );
            ensure!(
                deps.contains("answer.wasm"),
                "transitive Wasm dependency missing: {deps}"
            );
            ensure!(
                deps.contains("glue.zfb-resource.mjs"),
                "transitive glue dependency missing: {deps}"
            );
            last = Some(out);
        }
        let out = last.unwrap();
        let source = std::fs::read_to_string(&out.bundle_path)?;
        ensure!(
            source.contains("//# sourceMappingURL=bundle.mjs.map"),
            "linked sourcemap missing"
        );
        ensure!(
            out.emitted_wasm_assets.len() == 1,
            "SSR Wasm provenance missing"
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let error_body = runtime.block_on(async {
            let mut host = EmbeddedV8RenderHost::with_loader(
                BundleModuleLoader::new().with_bundle_asset_root(out.bundle_path.parent().unwrap()),
            )?;
            host.execute_module("bundle.mjs", &source).await?;
            let home = host
                .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
                .await?;
            ensure!(
                home.status == 200,
                "SSR status {}: {:?}",
                home.status,
                home.body_utf8()
            );
            let html = home.body_utf8().unwrap();
            let classes = input(&root, backend).css_module_class_maps;
            ensure!(
                html.contains(&classes[&root.join("pages/page.module.css")]["hero"]),
                "prepared CSS class missing"
            );
            for expected in [
                "Native prototype",
                "<strong>MDX</strong>",
                "workspace alias",
                "data-zfb-island=\"Counter\"",
            ] {
                ensure!(html.contains(expected), "SSR missing {expected}: {html}");
            }
            let dist = root.parent().unwrap().join(format!("dist-{backend}"));
            std::fs::create_dir_all(&dist)?;
            std::fs::write(dist.join("index.html"), html)?;
            let failure = host
                .dispatch_fetch(HttpRequestLike::get("http://zfb.local/throw"))
                .await?;
            ensure!(failure.status == 500, "throw status {}", failure.status);
            Ok::<_, anyhow::Error>(failure.body_utf8().unwrap().to_string())
        })?;
        ensure!(
            error_body.contains("deliberate-rolldown-probe"),
            "missing deliberate throw: {error_body}"
        );
        let projected =
            zfb_build::renderer::prototype_project_error(&error_body, &out.sourcemap_path, &root);
        ensure!(
            projected
                .as_deref()
                .is_some_and(|s| s.starts_with("pages/throw.tsx:2:")),
            "no verified authored throw mapping: {projected:?}: {error_body}"
        );
        let mut excluded = input(&root, backend);
        excluded.bundle_exclude = vec!["components/message-dep.ts".into()];
        let error = bundle(excluded).expect_err("excluded transitive source must fail");
        ensure!(
            format!("{error:#}").contains("message-dep"),
            "unactionable excluded diagnostic: {error:#}"
        );
        let mut unresolved = input(&root, backend);
        unresolved.plugin_virtual_modules =
            vec![("virtual:unused".into(), "export default 1".into())];
        std::fs::write(
            root.join("pages/unresolved.ts"),
            "import missing from './absent-probe'; export default () => missing;",
        )?;
        let result = bundle(unresolved);
        std::fs::remove_file(root.join("pages/unresolved.ts"))?;
        let error = result.expect_err("unresolved import must fail");
        ensure!(
            format!("{error:#}").contains("absent-probe"),
            "unactionable unresolved diagnostic: {error:#}"
        );
        println!("SSR {backend}: host/MDX/CSS/alias/transitive deps/exclusion/diagnostic passed, mapping={projected:?}, selected_pipeline_us={timings:?}");
        Ok(())
    }
}
