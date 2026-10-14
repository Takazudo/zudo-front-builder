//! Bounded plugin-option feasibility probe, not a production caller.
//! It deliberately exposes unresolved external/tsconfig semantics for comparison.
#[cfg(feature = "native")]
fn main() -> anyhow::Result<()> {
    use anyhow::bail;
    use rolldown::{Bundler, BundlerOptions};
    use rolldown_common::{
        AddonOutputOption, BundlerTransformOptions, CodeSplittingMode, Either, IsExternal,
        OutputFormat, Platform, SourceMapType,
    };
    use std::{path::Path, sync::Arc};

    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        bail!("usage: plugin_prebundle ABSOLUTE_ENTRY ABSOLUTE_OUTFILE");
    }
    if !args.iter().all(|arg| Path::new(arg).is_absolute()) {
        bail!("probe paths must be absolute");
    }
    let options = BundlerOptions {
        cwd: Some(std::env::current_dir()?),
        input: Some(vec![args[0].clone().into()]),
        file: Some(args[1].clone()),
        format: Some(OutputFormat::Esm),
        platform: Some(Platform::Node),
        code_splitting: Some(CodeSplittingMode::Bool(false)),
        sourcemap: Some(SourceMapType::Inline),
        banner: Some(AddonOutputOption::String(Some(
            "import { createRequire as __zfbCreateRequire } from \"node:module\"; const require = __zfbCreateRequire(import.meta.url);".into(),
        ))),
        transform: Some(BundlerTransformOptions {
            target: Some(Either::Left("node22".into())),
            ..Default::default()
        }),
        // Candidate mapping only. In particular, the differential fixture checks
        // whether this incorrectly externalizes a tsconfig paths alias before
        // resolution. Do not copy this predicate into the production adapter.
        external: Some(IsExternal::Fn(Some(Arc::new(|specifier, _, _| {
            let bare = !specifier.starts_with('.') && !Path::new(specifier).is_absolute();
            Box::pin(async move { Ok(bare) })
        })))),
        ..Default::default()
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut bundler = Bundler::with_plugins(options, vec![])
            .map_err(|error| anyhow::anyhow!("Rolldown options: {error:?}"))?;
        let result = bundler.write().await.map_err(|errors| {
            anyhow::anyhow!(
                "{}",
                errors
                    .into_vec()
                    .iter()
                    .map(|error| error.to_diagnostic().convert_to_string(false))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })?;
        for warning in &result.warnings {
            let message = warning.to_diagnostic().convert_to_string(false);
            if warning.kind().to_string() == "UNRESOLVED_IMPORT" {
                bail!("{message}");
            }
            eprintln!("{message}");
        }
        bundler
            .close()
            .await
            .map_err(|error| anyhow::anyhow!("close plugin probe bundle: {error:?}"))?;
        Ok::<_, anyhow::Error>(())
    })?;
    eprintln!("zfb-plugin-probe: native rolldown 24bc2d0");
    Ok(())
}

#[cfg(not(feature = "native"))]
fn main() {
    panic!("plugin_prebundle requires --features native; no backend fallback");
}
