//! Standalone, site-build-free CSS compilation.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use zfb_css::{AuthoredCssEngine, CssEmitterOutput, CssEngine, CssEngineOutput, WindEngine};

use crate::cli::{CssArgs, CssCodeHighlightMode};
use crate::commands::css_support::{
    build_standalone_wind_source_plan, configured_wind, index_standalone_wind_sources,
    resolve_framework_css_with_options, run_css_emitter_without_modules,
};
use crate::config::CodeHighlightMode;

struct CompileRequest {
    engine: ErasedEngine,
    project_root: PathBuf,
    output: PathBuf,
    framework_css: Option<String>,
    source_plan: Option<zfb_css::SourcePlan>,
}

struct ErasedEngine(Box<dyn CssEngine>);

impl CssEngine for ErasedEngine {
    fn produce_utility_css(&self, sources: &[PathBuf]) -> Result<CssEngineOutput> {
        self.0.produce_utility_css(sources)
    }
}

trait Emitter {
    fn emit(&self, request: CompileRequest) -> Result<CssEmitterOutput>;
}

struct ProductionEmitter;

impl Emitter for ProductionEmitter {
    fn emit(&self, request: CompileRequest) -> Result<CssEmitterOutput> {
        let _source_plan = request.source_plan;
        run_css_emitter_without_modules(
            request.engine,
            &request.project_root,
            request.output.parent().unwrap_or(&request.project_root),
            request.framework_css,
        )
    }
}

/// Compile one stylesheet without discovering routes, rendering pages, or
/// starting the plugin host.
pub async fn run(args: &CssArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    run_from(args, &cwd, &ProductionEmitter).await
}

async fn run_from(args: &CssArgs, cwd: &Path, emitter: &dyn Emitter) -> Result<()> {
    let cwd = absolute_path(cwd, cwd);
    let input = absolute_path(&cwd, &args.input);
    let output = absolute_path(&cwd, &args.output);
    let project_root = absolute_path(&cwd, args.project_root.as_deref().unwrap_or(Path::new(".")));

    let mut validation_errors = Vec::new();
    if let Err(error) = std::fs::read(&input) {
        validation_errors.push(format!(
            "cannot read CSS input {}: {error}",
            input.display()
        ));
    }
    if !project_root.is_dir() {
        validation_errors.push(format!(
            "project root is not a readable directory: {}",
            project_root.display()
        ));
    }

    if paths_resolve_same(&input, &output) {
        validation_errors.push(format!(
            "CSS input and output resolve to the same path: {}",
            input.display()
        ));
    }
    bail_collected(std::mem::take(&mut validation_errors))?;

    // Loading config is the command's only project interaction. In
    // particular, this does not discover pages/content and never starts the
    // plugin host. A config-less directory returns Config::default() without
    // evaluating TypeScript or booting V8.
    let config = crate::config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration for CSS compilation")?;
    let (generation_enabled, wind_config) = configured_wind(&config);

    let (engine, source_plan) = if generation_enabled {
        let explicit_sources = resolve_explicit_sources(&project_root, &args.source);
        for (authored, absolute) in &explicit_sources {
            match source_glob_matches_file(absolute) {
                Ok(true) => {}
                Ok(false) => validation_errors.push(format!(
                    "--source glob {authored:?} matched zero files (resolved as {})",
                    absolute.display()
                )),
                Err(error) => validation_errors.push(format!(
                    "invalid --source glob {authored:?} (resolved as {}): {error:#}",
                    absolute.display()
                )),
            }
        }
        bail_collected(validation_errors)?;
        let authored = authored_css_bundle(&input, &project_root)?;
        let plan = build_standalone_wind_source_plan(
            &project_root,
            &output,
            &config,
            !args.no_auto_source,
            &explicit_sources,
        )?;
        let indexed = index_standalone_wind_sources(&plan)?;
        let engine = WindEngine::new(wind_config, indexed.candidates, authored)
            .with_diagnostics(indexed.diagnostics)
            .with_origins(indexed.origins);
        (ErasedEngine(Box::new(engine)), Some(plan))
    } else {
        let authored = authored_css_bundle(&input, &project_root)?;
        (
            ErasedEngine(Box::new(AuthoredCssEngine::with_bundle(authored))),
            None,
        )
    };

    let mode_override = args.code_highlight_mode.map(|mode| match mode {
        CssCodeHighlightMode::Class => CodeHighlightMode::Class,
        CssCodeHighlightMode::Inline => CodeHighlightMode::Inline,
    });
    let framework_css = resolve_framework_css_with_options(
        &config,
        mode_override,
        args.no_default_highlight_styles,
    );

    let emitted = emitter
        .emit(CompileRequest {
            engine,
            project_root,
            output: output.clone(),
            framework_css,
            source_plan,
        })
        .context("wind CSS compilation failed")?;

    let mut output_errors = Vec::new();
    if !emitted.companions.is_empty() {
        let text = String::from_utf8_lossy(&emitted.bytes);
        let companion_names = emitted
            .companions
            .iter()
            .map(|asset| asset.filename.as_str())
            .collect::<HashSet<_>>();
        let references = zfb_css::url_scanner::scan_css_urls(&text)
            .into_iter()
            .filter(|occurrence| companion_names.contains(occurrence.decoded.as_str()))
            .map(|occurrence| format!("url({})", &text[occurrence.value_span]))
            .collect::<Vec<_>>();
        let references = if references.is_empty() {
            emitted
                .companions
                .iter()
                .map(|asset| format!("url(\"{}\")", asset.filename))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            references.join(", ")
        };
        output_errors.push(format!(
            "CSS output references companion assets that `zfb css` v1 cannot emit: {references}"
        ));
    }
    let emitted_text = String::from_utf8_lossy(&emitted.bytes);
    let unresolved = zfb_css::scan_leftover_directives(
        &emitted_text,
        &[
            "import",
            "tailwind",
            "theme",
            "source",
            "custom-variant",
            "apply",
            "utility",
            "variant",
            "plugin",
            "config",
            "reference",
            "--spacing",
            "--alpha",
            "--value",
        ],
    );
    if !unresolved.is_empty() {
        output_errors.push(format!(
            "CSS output still contains unresolved zudo-wind directives: {}",
            unresolved
                .iter()
                .map(|directive| directive.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    bail_collected(output_errors)?;

    zfb_build::atomic::atomic_write(&output, &emitted.bytes)
        .with_context(|| format!("failed to write CSS output {}", output.display()))
}

fn bail_collected(errors: Vec<String>) -> Result<()> {
    if errors.is_empty() {
        Ok(())
    } else {
        bail!(
            "CSS compilation validation failed:\n- {}",
            errors.join("\n- ")
        )
    }
}

/// Anchor a path without collapsing `..`: a parent component after a symlink
/// must be resolved by the filesystem, not against the symlink's lexical path.
fn absolute_path(base: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        if component != Component::CurDir {
            normalized.push(component.as_os_str());
        }
    }
    normalized
}

fn resolve_explicit_sources(project_root: &Path, sources: &[String]) -> Vec<(String, PathBuf)> {
    let mut seen = HashSet::new();
    sources
        .iter()
        .filter(|source| seen.insert((*source).clone()))
        .map(|source| {
            (
                source.clone(),
                absolute_path(project_root, Path::new(source)),
            )
        })
        .collect()
}

fn contains_glob_meta(component: &std::ffi::OsStr) -> bool {
    component
        .to_string_lossy()
        .bytes()
        .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b'{'))
}

fn source_glob_matches_file(pattern: &Path) -> Result<bool> {
    let components = pattern.components().collect::<Vec<_>>();
    let wildcard_at = components
        .iter()
        .position(|component| contains_glob_meta(component.as_os_str()));
    let Some(wildcard_at) = wildcard_at else {
        if pattern.is_file() {
            return Ok(true);
        }
        if pattern.is_dir() {
            return Ok(walkdir::WalkDir::new(pattern)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
                .any(|entry| entry.file_type().is_file()));
        }
        return Ok(false);
    };

    let mut root = PathBuf::new();
    for component in &components[..wildcard_at] {
        root.push(component.as_os_str());
    }
    if root.as_os_str().is_empty() {
        root.push(".");
    }
    if !root.is_dir() {
        return Ok(false);
    }
    let suffix = components[wildcard_at..]
        .iter()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    // Override globs without a leading slash match a basename at any depth
    // (gitignore semantics). Anchor here so `*.tsx` means only the static
    // root's direct children, matching the established glob semantics.
    let anchored_suffix = format!("/{suffix}");
    let mut overrides = ignore::overrides::OverrideBuilder::new(&root);
    overrides
        .add(&anchored_suffix)
        .with_context(|| format!("invalid source glob {suffix:?}"))?;
    let overrides = overrides
        .build()
        .with_context(|| format!("invalid source glob {suffix:?}"))?;
    let mut walker = ignore::WalkBuilder::new(&root);
    walker
        .follow_links(false)
        .standard_filters(false)
        .overrides(overrides);
    Ok(walker
        .build()
        .filter_map(Result::ok)
        .any(|entry| entry.file_type().is_some_and(|kind| kind.is_file())))
}

fn paths_resolve_same(input: &Path, output: &Path) -> bool {
    if input == output {
        return true;
    }
    let input = std::fs::canonicalize(input).ok();
    let output = if output.exists() {
        std::fs::canonicalize(output).ok()
    } else {
        output.parent().and_then(|parent| {
            std::fs::canonicalize(parent)
                .ok()
                .and_then(|parent| output.file_name().map(|name| parent.join(name)))
        })
    };
    input.is_some() && input == output
}

const FORBIDDEN_WIND_DIRECTIVES: &[&str] = &[
    "import",
    "tailwind",
    "theme",
    "source",
    "custom-variant",
    "apply",
    "utility",
    "variant",
    "plugin",
    "config",
    "reference",
    "--spacing",
    "--alpha",
    "--value",
];

fn authored_css_bundle(entry: &Path, project_root: &Path) -> Result<zfb_css::AuthoredCssBundle> {
    let raw = std::fs::read_to_string(entry)
        .with_context(|| format!("failed to read CSS input {}", entry.display()))?;
    check_leftover_directives(&raw, entry)?;
    // Inspect the entry before resolving any imports. The shared resolver
    // skips Tailwind's virtual import by design; the token-aware scan must
    // reject it first, including on wind:false.
    for stylesheet in zfb_css::resolve_css_imports(entry, project_root) {
        let css = std::fs::read_to_string(&stylesheet).with_context(|| {
            format!(
                "failed to read imported stylesheet {}",
                stylesheet.display()
            )
        })?;
        check_leftover_directives(&css, &stylesheet)?;
    }
    zfb_css::bundle_authored_css_with_assets(entry, project_root, &raw)
}

fn check_leftover_directives(css: &str, path: &Path) -> Result<()> {
    let leftover = zfb_css::scan_leftover_directives(css, FORBIDDEN_WIND_DIRECTIVES);
    if let Some(directive) = leftover.first() {
        bail!(
            "ZW009: forbidden {} at {}:{}:{}; migrate this stylesheet to zudo-wind: {}",
            directive.name,
            path.display(),
            directive.line,
            directive.column,
            "/docs/zudo-wind/coming-from-tailwind/"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockEmitter<F>(F);

    impl<F> Emitter for MockEmitter<F>
    where
        F: Fn(CompileRequest) -> Result<CssEmitterOutput>,
    {
        fn emit(&self, request: CompileRequest) -> Result<CssEmitterOutput> {
            (self.0)(request)
        }
    }

    fn args() -> CssArgs {
        CssArgs {
            input: PathBuf::from("entry.css"),
            output: PathBuf::from("dist/out.css"),
            project_root: None,
            source: Vec::new(),
            no_auto_source: false,
            code_highlight_mode: None,
            no_default_highlight_styles: false,
        }
    }

    fn stub_output(request: CompileRequest, css: &str) -> Result<CssEmitterOutput> {
        let _source_plan = request.source_plan;
        run_css_emitter_without_modules(
            zfb_css::StubCssEngine::new(css),
            &request.project_root,
            request.output.parent().unwrap(),
            request.framework_css,
        )
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("entry.css"), ".authored { color: red; }\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn builds_default_source_plan_and_emits_stub_output_without_modules() {
        let dir = project();
        std::fs::create_dir_all(dir.path().join("pages")).unwrap();
        std::fs::write(
            dir.path().join("pages/index.tsx"),
            "<div className=\"block\" />",
        )
        .unwrap();
        let emitter = MockEmitter(|request: CompileRequest| {
            let plan = request.source_plan.as_ref().expect("wind source plan");
            let labels: Vec<_> = plan.roots.iter().map(|root| root.label.as_str()).collect();
            assert_eq!(
                labels,
                [
                    "default/components",
                    "default/content",
                    "default/layouts",
                    "default/pages",
                    "default/src",
                ]
            );
            assert!(plan.extensions.contains("html"));
            assert!(plan.extensions.contains("mjs"));
            stub_output(request, ".block { display: block; }")
        });

        run_from(&args(), dir.path(), &emitter).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dist/out.css")).unwrap(),
            ".block { display: block; }\n"
        );
        assert!(!dir.path().join("dist/css-modules").exists());
    }

    #[tokio::test]
    async fn no_auto_source_keeps_only_deduplicated_explicit_html_globs() {
        let outer = project();
        let root = outer.path().join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.html"), "<div class=\"block\"></div>").unwrap();
        let mut command = args();
        command.project_root = Some(PathBuf::from("project"));
        command.no_auto_source = true;
        command.source = vec!["src/**/*.html".into(), "src/**/*.html".into()];
        let emitter = MockEmitter(|request: CompileRequest| {
            let plan = request.source_plan.as_ref().expect("wind source plan");
            assert_eq!(plan.roots.len(), 1, "duplicate CLI globs are deduplicated");
            assert_eq!(plan.roots[0].label, "cli/source-0000/match-0000");
            assert_eq!(plan.roots[0].resolved_path(), root.join("src/a.html"));
            assert!(!plan
                .roots
                .iter()
                .any(|root| root.label.starts_with("default/")));
            stub_output(request, ".block { display: block; }")
        });
        run_from(&command, outer.path(), &emitter).await.unwrap();
    }

    #[tokio::test]
    async fn wind_false_skips_source_validation_and_only_emits_authored_css() {
        let dir = project();
        std::fs::write(dir.path().join("zfb.config.json"), r#"{"wind":false}"#).unwrap();
        let mut command = args();
        command.source = vec!["missing/**/*.tsx".into()];
        let emitter = MockEmitter(|request: CompileRequest| {
            assert!(request.source_plan.is_none());
            let engine_css = request.engine.produce_utility_css(&[]).unwrap().css;
            assert_eq!(engine_css, ".authored {\n  color: red;\n}\n");
            run_css_emitter_without_modules(
                request.engine,
                &request.project_root,
                request.output.parent().unwrap(),
                request.framework_css,
            )
        });
        run_from(&command, dir.path(), &emitter).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dist/out.css")).unwrap(),
            ".authored {\n  color: red;\n}\n\n"
        );
    }

    #[tokio::test]
    async fn configured_class_highlight_css_honors_cli_mode_and_opt_out() {
        let dir = project();
        std::fs::write(
            dir.path().join("zfb.config.json"),
            r#"{"codeHighlight":{"mode":"class","classPrefix":"tok-","defaultStylesheet":true}}"#,
        )
        .unwrap();
        let emitter = MockEmitter(|request: CompileRequest| {
            let framework = request
                .framework_css
                .as_deref()
                .expect("class highlight CSS");
            assert!(framework.contains(".tok-kw"));
            assert!(!framework.contains(".hi-kw"));
            stub_output(request, "")
        });
        run_from(&args(), dir.path(), &emitter).await.unwrap();

        let mut command = args();
        command.code_highlight_mode = Some(CssCodeHighlightMode::Class);
        command.no_default_highlight_styles = true;
        let emitter = MockEmitter(|request: CompileRequest| {
            assert!(request.framework_css.is_none());
            stub_output(request, "")
        });
        run_from(&command, dir.path(), &emitter).await.unwrap();
    }

    #[tokio::test]
    async fn forbidden_directive_in_import_is_rejected_before_output() {
        let dir = project();
        std::fs::create_dir_all(dir.path().join("styles")).unwrap();
        std::fs::write(
            dir.path().join("entry.css"),
            "@import \"./styles/nested.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("styles/nested.css"),
            ".broken { @apply unknown; }\n",
        )
        .unwrap();
        let emitter = MockEmitter(|_| panic!("leftover directives fail before the emitter"));
        let error = run_from(&args(), dir.path(), &emitter)
            .await
            .expect_err("nested leftover directive must fail");
        let message = format!("{error:#}");
        assert!(message.contains("ZW009"), "{message}");
        assert!(message.contains("styles/nested.css"), "{message}");
        assert!(!dir.path().join("dist/out.css").exists());
    }

    #[test]
    fn source_glob_validation_preserves_path_separator_semantics() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("nested/only.tsx"), "").unwrap();
        assert!(!source_glob_matches_file(&dir.path().join("*.tsx")).unwrap());
        assert!(source_glob_matches_file(&dir.path().join("**/*.tsx")).unwrap());
    }

    #[tokio::test]
    async fn preflight_collects_missing_input_zero_glob_and_same_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("entry.css"), "body {}\n").unwrap();
        let mut command = args();
        command.input = PathBuf::from("missing.css");
        command.output = PathBuf::from("missing.css");
        command.source = vec!["nothing/**/*.tsx".into()];
        let emitter = MockEmitter(|_| panic!("preflight failure must not emit"));
        let error = run_from(&command, dir.path(), &emitter)
            .await
            .expect_err("preflight validation must fail");
        let message = format!("{error:#}");
        assert!(message.contains("cannot read CSS input"), "{message}");
        assert!(message.contains("same path"), "{message}");
        // Wind-enabled source validation happens after loading configuration;
        // this test pins the early file/path validation boundary.
        assert!(!message.contains("nothing/**/*.tsx"), "{message}");
    }
}
