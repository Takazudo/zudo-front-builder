//! Opt-in #318 L3 producer; browser execution is in prototypes/rolldown/browser.mjs.
use anyhow::{ensure, Result};
use std::{path::PathBuf, sync::Arc, time::Instant};
use zfb_build::pipeline::prod::{
    CompanionFile, EmittedAsset, ProductionAssetPipeline, ProductionEmitters,
};
use zfb_build::pipeline::{AssetPipeline, BuildContext};
use zfb_islands::bundler::ModuleWorkerBundleEntry;
use zfb_islands::{
    BundleConfig, ClientBundler, EsbuildSubprocessBundler, EsbuildSubprocessConfig, Island,
};
fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).expect("project root")).canonicalize()?;
    let backend = if std::env::var("ZFB_ROLLDOWN_PROTOTYPE").as_deref() == Ok("1") {
        "rolldown"
    } else {
        "esbuild"
    };
    let build = zfb_build::bundler::zudo_react_build_token(&root)?;
    // Reuse the actual worker URL preparation against logical authored paths.
    let stage = tempfile::tempdir()?;
    std::fs::create_dir(stage.path().join("components"))?;
    for file in std::fs::read_dir(root.join("components"))? {
        let file = file?;
        std::fs::copy(
            file.path(),
            stage.path().join("components").join(file.file_name()),
        )?;
    }
    std::fs::copy(
        root.join("tsconfig.json"),
        stage.path().join("tsconfig.json"),
    )?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("node_modules"), stage.path().join("node_modules"))?;
    #[cfg(not(unix))]
    anyhow::bail!("prototype fixture staging currently requires Unix symlinks");
    let importer = root.join("components/Counter.tsx");
    let rewrite = zfb_build::module_worker::rewrite_module_worker_urls_with_context(
        &std::fs::read_to_string(&importer)?,
        &importer,
        &root,
        &zfb_build::module_worker::ModuleWorkerBuildContext::new(
            true,
            &Default::default(),
            &Default::default(),
        ),
    )?;
    std::fs::write(
        stage.path().join("components/Counter.tsx"),
        rewrite.expanded_source,
    )?;
    let workers = rewrite
        .worker_edges
        .into_iter()
        .map(|edge| {
            ModuleWorkerBundleEntry::new(
                &root,
                &edge.source_path,
                stage.path().join(edge.source_path.strip_prefix(&root)?),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let config = BundleConfig::production()
        .with_zudo_react_build(Some(build))
        .with_module_workers(workers)
        .with_module_labels(
            [(
                stage.path().join("components/Counter.tsx"),
                "components/Counter.tsx".into(),
            )]
            .into_iter()
            .collect(),
        );
    let bundler = EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig::default().with_working_dir(stage.path()),
    );
    let islands = [Island::new(
        "Counter",
        stage.path().join("components/Counter.tsx"),
    )];
    let mut timings = Vec::new();
    let mut previous = None;
    let mut publish_timings = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        let output = bundler.bundle(&islands, &config)?;
        timings.push(start.elapsed().as_micros());
        ensure!(!output.chunks.is_empty(), "dynamic import was not split");
        ensure!(output.workers.len() == 1, "worker companion missing");
        ensure!(output.resources.len() == 2, "resource provenance missing");
        let mut companions: Vec<_> = output
            .chunks
            .into_iter()
            .chain(output.workers)
            .map(|c| CompanionFile {
                filename: c.filename,
                bytes: c.bytes,
            })
            .collect();
        companions.extend(output.resources.into_iter().map(|c| CompanionFile {
            filename: c.filename,
            bytes: c.bytes,
        }));
        companions.sort_by(|a, b| a.filename.cmp(&b.filename));
        let snapshot = (output.bytes.clone(), companions.clone());
        if let Some(ref prior) = previous {
            ensure!(*prior == snapshot, "backend output was nondeterministic");
        }
        previous = Some(snapshot);
        let asset = EmittedAsset {
            bytes: output.bytes,
            relative_path: "assets/islands.js".into(),
            stable_url: Some("/assets/islands.js".into()),
            companions,
        };
        let pipeline = ProductionAssetPipeline::new(ProductionEmitters {
            islands: Some(Box::new(move || Ok(Some(asset.clone())))),
            ..Default::default()
        });
        let mut plan = zfb_build::RebuildPlan::empty();
        plan.rerun_islands = true;
        let publish_start = Instant::now();
        let outcome = pipeline.apply(
            &plan,
            &BuildContext {
                dist_root: root.parent().unwrap().join(format!("dist-{backend}")),
                render_pages: Arc::new(|_, _| Ok(vec![])),
                run_css: None,
                run_islands: None,
                run_client_scripts: None,
                reload_renderer: None,
            },
        )?;
        publish_timings.push(publish_start.elapsed().as_micros());
        let entry_url = &outcome
            .hashed_asset_urls
            .first()
            .expect("published islands URL")
            .1;
        std::fs::write(
            root.parent()
                .unwrap()
                .join(format!("browser-{backend}.json")),
            serde_json::to_vec_pretty(&serde_json::json!({"entry_url":entry_url}))?,
        )?;
    }
    println!("browser {backend}: deterministic split/worker/resources, selected_bundle_call_us={timings:?}, production_publish_us={publish_timings:?}");
    Ok(())
}
