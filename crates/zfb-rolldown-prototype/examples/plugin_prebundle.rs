//! Bounded plugin-option feasibility probe, not a production caller.
//! Real Node execution in plugin-prebundle.mjs owns the semantic oracles.
#[cfg(feature = "native")]
#[path = "plugin_prebundle/job.rs"]
mod job;
#[cfg(feature = "native")]
#[path = "plugin_prebundle/lifecycle.rs"]
mod lifecycle;
#[cfg(feature = "native")]
#[path = "plugin_prebundle/packages.rs"]
mod packages;

#[cfg(feature = "native")]
fn main() -> anyhow::Result<()> {
    use anyhow::bail;
    use packages::PackageExternals;
    use rolldown::plugin::Plugin;
    use rolldown::{BundlerOptions, ResolveOptions};
    use rolldown_common::{
        AddonOutputOption, BundlerTransformOptions, CodeSplittingMode, Either, OutputFormat,
        Platform, SourceMapType,
    };
    use std::{path::Path, time::Duration};

    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        bail!("usage: plugin_prebundle ABSOLUTE_ENTRY ABSOLUTE_NEW_OUTFILE");
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
        // Resolve local mappings before externalizing packages. This disables
        // package-directory lookup, NOT tsconfig paths/baseUrl or file lookup.
        resolve: Some(ResolveOptions {
            modules: Some(vec![]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let job = job::Job::spawn(options, vec![PackageExternals::new_shared(PackageExternals)])?;
    let completed = match job.wait(Duration::from_secs(300)) {
        job::Wait::Joined(completed) => completed,
        job::Wait::Pending(job) => {
            job.cancellation().cancel();
            eprintln!(
                "plugin probe deadline expired; publication revoked; waiting for native shutdown (not bounded)"
            );
            // Cancellation is cooperative, not a way to kill a native thread.
            // Keep ownership and join instead of detaching work or claiming it
            // has stopped. The completed object must still refuse publication.
            job.join()
        }
    };
    completed.publish(Path::new(&args[1]))?;
    eprintln!("zfb-plugin-probe: native rolldown 24bc2d0");
    Ok(())
}

#[cfg(not(feature = "native"))]
fn main() {
    panic!("plugin_prebundle requires --features native; no backend fallback");
}
