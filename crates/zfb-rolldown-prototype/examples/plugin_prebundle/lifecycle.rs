//! Probe-only publication barrier. This is not active-build cancellation.
use anyhow::{anyhow, bail, Context, Result};
use rolldown::plugin::{
    __inner::SharedPluginable, HookCloseBundleArgs, HookUsage, Plugin, PluginContext,
};
use rolldown::{Bundler, BundlerOptions};
use rolldown_common::Output;
use std::{
    borrow::Cow,
    fs::OpenOptions,
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
};

#[derive(Debug, Default)]
struct CloseObservation {
    started: AtomicBool,
    scan_errors: OnceLock<String>,
}

/// At this pin, scan failure calls close hooks without marking BundleHandle
/// closed. Observe that call rather than invoking resource hooks twice. This
/// is private to this one-shot probe, not a general plugin shutdown scheduler.
#[derive(Debug, Clone, Default)]
struct CloseTracker(Arc<CloseObservation>);
impl Plugin for CloseTracker {
    fn name(&self) -> Cow<'static, str> {
        "zfb-plugin-probe-close-tracker".into()
    }
    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::CloseBundle
    }
    async fn close_bundle(
        &self,
        _ctx: &PluginContext,
        args: Option<&HookCloseBundleArgs<'_>>,
    ) -> Result<()> {
        if let Some(args) = args {
            // Upstream can replace scan diagnostics with a failing close hook.
            // Retain the original before subsequent probe hooks can fail.
            let _ = self.0.scan_errors.set(
                args.errors
                    .iter()
                    .map(|error| error.to_diagnostic().convert_to_string(false))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        self.0.started.store(true, Ordering::Release);
        Ok(())
    }
}

/// Own one generation through observed close and driver cleanup. The bundler
/// cannot escape for reuse. The caller must finish shutting down its runtime
/// before publishing bytes; no native worker writes the output path.
pub(super) async fn generate_and_close(
    options: BundlerOptions,
    mut plugins: Vec<SharedPluginable>,
) -> Result<String> {
    let tracker = CloseTracker::default();
    plugins.insert(0, CloseTracker::new_shared(tracker.clone()));
    let mut bundler = Bundler::with_plugins(options, plugins)
        .map_err(|error| anyhow!("Rolldown options: {error:?}"))?;
    let generated = async {
        let result = bundler.generate().await.map_err(|errors| {
            anyhow!(
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
                bail!("plugin probe refused unresolved-import warning: {message}");
            }
            eprintln!("{message}");
        }
        let mut outputs = result.assets.into_iter();
        let Some(Output::Chunk(chunk)) = outputs.next() else {
            bail!("plugin probe requires exactly one JavaScript chunk");
        };
        if outputs.next().is_some() {
            bail!("plugin probe refuses additional chunks/assets");
        }
        Ok(chunk.code.to_string())
    }
    .await;
    // Do not put `?` between generation and cleanup. Scan failure may already
    // have called close hooks; successful generation, render failure and output
    // rejection still need explicit close. Never retry a failed resource hook.
    let closed = if tracker.0.started.load(Ordering::Acquire) {
        Ok(())
    } else {
        bundler
            .close()
            .await
            .map_err(|error| anyhow!("close plugin probe bundle: {error:?}"))
    };
    // Explicit close skips clear when a hook fails; automatic scan-error close
    // skips it entirely. Clear retained driver state on both normal-return paths.
    // This does not establish active cancellation or bounded quiescence.
    if let Some(handle) = &bundler.last_bundle_handle {
        handle.plugin_driver().clear();
    }
    let generated = generated.map_err(|error| match tracker.0.scan_errors.get() {
        Some(original) if original != &error.to_string() => {
            anyhow!("{original}\nRolldown error after scan cleanup: {error:#}")
        }
        _ => error,
    });
    finish(generated, closed)
}

fn finish<T>(generated: Result<T>, closed: Result<()>) -> Result<T> {
    match (generated, closed) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(generation), Err(close)) => Err(anyhow!(
            "{generation:#}\nAdditionally, cleanup failed: {close:#}"
        )),
    }
}

/// Reserve only a new path; never truncate or remove pre-existing output.
/// This is the standalone probe's output, not production's locked stage guard.
pub(super) fn publish_new(path: &Path, code: &str) -> Result<()> {
    publish_with(path, |file| file.write_all(code.as_bytes()))
}

fn publish_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create new plugin probe output {}", path.display()))?;
    let written = write(&mut file);
    // Windows must release this handle before removing an incomplete output.
    drop(file);
    if let Err(error) = written {
        let cleanup = std::fs::remove_file(path);
        return match cleanup {
            Ok(()) => Err(error).context("write plugin probe output; incomplete file removed"),
            Err(cleanup) => Err(anyhow!(
                "write plugin probe output: {error}; failed to remove {}: {cleanup}",
                path.display()
            )),
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolldown::plugin::{HookGenerateBundleArgs, HookRenderStartArgs};
    use rolldown_common::{CodeSplittingMode, EmittedAsset, OutputFormat, Platform, SourceMapType};
    use std::{
        borrow::Cow,
        path::PathBuf,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "zfb-plugin-publication-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn options(&self, source: &str) -> BundlerOptions {
            let entry = self.0.join("plugin.ts");
            std::fs::write(&entry, source).unwrap();
            BundlerOptions {
                cwd: Some(self.0.clone()),
                input: Some(vec![entry.to_string_lossy().into_owned().into()]),
                file: Some(self.output().to_string_lossy().into_owned()),
                format: Some(OutputFormat::Esm),
                platform: Some(Platform::Node),
                code_splitting: Some(CodeSplittingMode::Bool(false)),
                sourcemap: Some(SourceMapType::Inline),
                ..Default::default()
            }
        }
        fn output(&self) -> PathBuf {
            self.0.join("output.mjs")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Debug)]
    struct Observer {
        closes: Arc<AtomicUsize>,
        fail_render: bool,
        fail_close: bool,
    }
    impl Plugin for Observer {
        fn name(&self) -> Cow<'static, str> {
            "zfb-plugin-publication-test".into()
        }
        fn register_hook_usage(&self) -> HookUsage {
            HookUsage::RenderStart | HookUsage::CloseBundle
        }
        async fn render_start(
            &self,
            _ctx: &PluginContext,
            _args: &HookRenderStartArgs<'_>,
        ) -> Result<()> {
            if self.fail_render {
                bail!("deliberate-render-failure");
            }
            Ok(())
        }
        async fn close_bundle(
            &self,
            _ctx: &PluginContext,
            _args: Option<&HookCloseBundleArgs<'_>>,
        ) -> Result<()> {
            self.closes.fetch_add(1, Ordering::SeqCst);
            if self.fail_close {
                bail!("deliberate-close-failure");
            }
            Ok(())
        }
    }

    #[derive(Debug)]
    struct EmitAsset;
    impl Plugin for EmitAsset {
        fn name(&self) -> Cow<'static, str> {
            "zfb-plugin-extra-asset-test".into()
        }
        fn register_hook_usage(&self) -> HookUsage {
            HookUsage::GenerateBundle
        }
        async fn generate_bundle(
            &self,
            ctx: &PluginContext,
            args: &mut HookGenerateBundleArgs<'_>,
        ) -> Result<()> {
            assert!(!args.is_write, "probe must never call native write");
            ctx.emit_file_async(EmittedAsset {
                name: Some("unexpected.txt".into()),
                source: "unexpected asset".to_string().into(),
                ..Default::default()
            })
            .await?;
            Ok(())
        }
    }

    fn run(
        options: BundlerOptions,
        fail_render: bool,
        fail_close: bool,
    ) -> (Result<String>, usize) {
        run_with_plugins(options, fail_render, fail_close, Vec::new())
    }

    fn run_with_plugins(
        options: BundlerOptions,
        fail_render: bool,
        fail_close: bool,
        mut plugins: Vec<SharedPluginable>,
    ) -> (Result<String>, usize) {
        let closes = Arc::new(AtomicUsize::new(0));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        plugins.push(Observer::new_shared(Observer {
            closes: closes.clone(),
            fail_render,
            fail_close,
        }));
        let result = runtime.block_on(generate_and_close(options, plugins));
        drop(runtime);
        (result, closes.load(Ordering::SeqCst))
    }

    #[test]
    fn successful_generation_is_closed_before_explicit_publication() {
        let fixture = Fixture::new();
        let (result, closes) = run(fixture.options("export default () => 17;"), false, false);
        let code = result.unwrap();
        assert_eq!(closes, 1);
        assert!(
            !fixture.output().exists(),
            "generate must never write output"
        );
        assert!(code.contains("sourceMappingURL=data:"));
        publish_new(&fixture.output(), &code).unwrap();
        assert_eq!(std::fs::read_to_string(fixture.output()).unwrap(), code);
    }

    #[test]
    fn render_error_is_closed_without_publishing() {
        let fixture = Fixture::new();
        let (result, closes) = run(fixture.options("export default 17;"), true, false);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("deliberate-render-failure"));
        assert_eq!(closes, 1);
        assert!(!fixture.output().exists());
    }

    #[test]
    fn close_error_rejects_generated_bytes() {
        let fixture = Fixture::new();
        let (result, closes) = run(fixture.options("export default 17;"), false, true);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("deliberate-close-failure"));
        assert_eq!(closes, 1);
        assert!(!fixture.output().exists());
    }

    #[test]
    fn unresolved_warning_is_closed_without_publishing() {
        let fixture = Fixture::new();
        // Deliberately omit PackageExternals: this is a genuine native warning,
        // not a changed expectation for the separately tested alias resolver.
        let (result, closes) = run(
            fixture.options("import { value } from 'absent-probe-package'; export default value;"),
            false,
            false,
        );
        let message = result.unwrap_err().to_string();
        assert!(message.starts_with("plugin probe refused unresolved-import warning:"));
        assert!(message.contains("absent-probe-package"));
        assert_eq!(closes, 1);
        assert!(!fixture.output().exists());
    }

    #[test]
    fn missing_relative_import_is_closed_without_publishing() {
        let fixture = Fixture::new();
        let (result, closes) = run(
            fixture.options("import './absent-source.ts';"),
            false,
            false,
        );
        assert!(result.unwrap_err().to_string().contains("absent-source.ts"));
        assert_eq!(closes, 1);
        assert!(!fixture.output().exists());
    }

    #[test]
    fn additional_native_asset_is_rejected_before_publication() {
        let fixture = Fixture::new();
        let (result, closes) = run_with_plugins(
            fixture.options("export default 17;"),
            false,
            false,
            vec![EmitAsset::new_shared(EmitAsset)],
        );
        let message = result.unwrap_err().to_string();
        assert_eq!(message, "plugin probe refuses additional chunks/assets");
        assert_eq!(closes, 1);
        assert!(!fixture.output().exists());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn scan_and_automatic_close_errors_keep_the_original_without_retry() {
        let fixture = Fixture::new();
        let (result, closes) = run(
            fixture.options("import './absent-source.ts';"),
            false,
            true,
        );
        let message = result.unwrap_err().to_string();
        assert!(message.contains("absent-source.ts"));
        assert!(message.contains("deliberate-close-failure"));
        assert_eq!(closes, 1, "failed resource close must not be retried");
        assert!(!fixture.output().exists());
    }

    #[test]
    fn render_and_explicit_close_errors_are_both_retained() {
        let fixture = Fixture::new();
        let (result, closes) = run(fixture.options("export default 17;"), true, true);
        let message = result.unwrap_err().to_string();
        assert!(message.contains("deliberate-render-failure"));
        assert!(message.contains("deliberate-close-failure"));
        assert_eq!(closes, 1);
        assert!(!fixture.output().exists());
    }

    #[test]
    fn existing_directory_and_its_contents_are_preserved() {
        let fixture = Fixture::new();
        std::fs::create_dir(fixture.output()).unwrap();
        let owned = fixture.output().join("other-owner.txt");
        std::fs::write(&owned, "other-owner").unwrap();
        assert!(publish_new(&fixture.output(), "replacement").is_err());
        assert!(fixture.output().is_dir());
        assert_eq!(std::fs::read_to_string(owned).unwrap(), "other-owner");
    }

    #[test]
    fn failed_write_removes_its_partial_output_but_not_a_sibling() {
        let fixture = Fixture::new();
        let sibling = fixture.0.join("other-owner.mjs");
        std::fs::write(&sibling, "other-owner").unwrap();
        let result = publish_with(&fixture.output(), |file| {
            file.write_all(b"partial-output")?;
            Err(std::io::Error::other("deliberate-write-failure"))
        });
        assert!(format!("{:#}", result.unwrap_err()).contains("deliberate-write-failure"));
        assert!(!fixture.output().exists());
        assert_eq!(std::fs::read_to_string(sibling).unwrap(), "other-owner");
    }

    #[test]
    fn generation_and_cleanup_errors_are_both_retained() {
        let result = finish::<String>(
            Err(anyhow!("original-generation-error")),
            Err(anyhow!("independent-close-error")),
        );
        let message = result.unwrap_err().to_string();
        assert!(message.starts_with("original-generation-error"));
        assert!(message.contains("independent-close-error"));
    }

    #[test]
    fn existing_output_is_neither_overwritten_nor_removed() {
        let fixture = Fixture::new();
        std::fs::write(fixture.output(), "other-owner").unwrap();
        assert!(publish_new(&fixture.output(), "replacement").is_err());
        assert_eq!(
            std::fs::read_to_string(fixture.output()).unwrap(),
            "other-owner"
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_symlink_is_not_followed_or_removed() {
        let fixture = Fixture::new();
        let target = fixture.0.join("other-owner.mjs");
        std::fs::write(&target, "other-owner").unwrap();
        std::os::unix::fs::symlink(&target, fixture.output()).unwrap();
        assert!(publish_new(&fixture.output(), "replacement").is_err());
        assert!(std::fs::symlink_metadata(fixture.output())
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "other-owner");
    }
}
