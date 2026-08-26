//! Standalone, site-build-free CSS compilation.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use zfb_css::{CssEmitterOutput, TailwindSubprocessConfig, TailwindSubprocessEngine};

use crate::cli::{CssArgs, CssCodeHighlightMode};
use crate::commands::css_support::{
    default_content_globs, resolve_framework_css_with_options, role_classes_inline_sources,
    run_css_emitter_without_modules, with_embedded_tailwind_binary,
};
use crate::config::CodeHighlightMode;

struct CompileRequest {
    tailwind: TailwindSubprocessConfig,
    project_root: PathBuf,
    output: PathBuf,
    framework_css: Option<String>,
}

trait Emitter {
    fn emit(&self, request: CompileRequest) -> Result<CssEmitterOutput>;
}

struct ProductionEmitter;

impl Emitter for ProductionEmitter {
    fn emit(&self, mut request: CompileRequest) -> Result<CssEmitterOutput> {
        request.tailwind = with_embedded_tailwind_binary(request.tailwind);
        let engine = TailwindSubprocessEngine::new(request.tailwind);
        run_css_emitter_without_modules(
            engine,
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

    if paths_resolve_same(&input, &output) {
        validation_errors.push(format!(
            "CSS input and output resolve to the same path: {}",
            input.display()
        ));
    }
    bail_collected(validation_errors)?;

    // Loading config is the command's only project interaction. In
    // particular, this does not discover pages/content and never starts the
    // plugin host. A config-less directory returns Config::default() without
    // evaluating TypeScript or booting V8.
    let config = crate::config::load_from_dir(&project_root)
        .await
        .context("failed to load project configuration for CSS compilation")?;

    let mut content_globs = if args.no_auto_source {
        Vec::new()
    } else {
        default_content_globs(&project_root)
    };
    content_globs.extend(
        explicit_sources
            .into_iter()
            .map(|(_, path)| path.to_string_lossy().into_owned()),
    );

    let mode_override = args.code_highlight_mode.map(|mode| match mode {
        CssCodeHighlightMode::Class => CodeHighlightMode::Class,
        CssCodeHighlightMode::Inline => CodeHighlightMode::Inline,
    });
    let framework_css = resolve_framework_css_with_options(
        &config,
        mode_override,
        args.no_default_highlight_styles,
    );

    // The standalone command intentionally ignores config.tailwind.enabled:
    // invoking `zfb css` is itself the explicit request to run Tailwind.
    let tailwind = TailwindSubprocessConfig::default()
        .with_working_dir(project_root.clone())
        .with_input_css(input)
        .with_content_globs(content_globs)
        .with_inline_sources(role_classes_inline_sources(&config))
        .with_explicit_sourcing(true);

    let emitted = emitter
        .emit(CompileRequest {
            tailwind,
            project_root,
            output: output.clone(),
            framework_css,
        })
        .context("Tailwind CSS compilation failed")?;

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
    let unresolved = unresolved_tailwind_directives(&emitted.bytes);
    if !unresolved.is_empty() {
        output_errors.push(format!(
            "CSS output still contains unresolved Tailwind directives: {}",
            unresolved.join(", ")
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
    // root's direct children, matching Tailwind/globset separator semantics.
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

/// Return active unresolved Tailwind constructs while ignoring comments and
/// string contents. CSS at-rule names and import matching are ASCII
/// case-insensitive.
fn unresolved_tailwind_directives(css: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(css);
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    let mut parentheses = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }
        if bytes[index] == b'(' {
            parentheses += 1;
            index += 1;
            continue;
        }
        if bytes[index] == b')' {
            parentheses = parentheses.saturating_sub(1);
            index += 1;
            continue;
        }
        if bytes[index] == b'@' && parentheses == 0 {
            let name_start = index + 1;
            let mut name_end = name_start;
            while name_end < bytes.len()
                && (bytes[name_end].is_ascii_alphanumeric() || bytes[name_end] == b'-')
            {
                name_end += 1;
            }
            let name = text[name_start..name_end].to_ascii_lowercase();
            if matches!(name.as_str(), "tailwind" | "apply" | "source") {
                found.push(format!("@{name}"));
            } else if name == "import" {
                let mut cursor = name_end;
                loop {
                    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                        cursor += 1;
                    }
                    if bytes.get(cursor) == Some(&b'/') && bytes.get(cursor + 1) == Some(&b'*') {
                        cursor += 2;
                        while cursor + 1 < bytes.len()
                            && !(bytes[cursor] == b'*' && bytes[cursor + 1] == b'/')
                        {
                            cursor += 1;
                        }
                        cursor = (cursor + 2).min(bytes.len());
                    } else {
                        break;
                    }
                }
                let mut import = None;
                if matches!(bytes.get(cursor), Some(b'\'' | b'\"')) {
                    let quote = bytes[cursor];
                    cursor += 1;
                    let start = cursor;
                    while cursor < bytes.len() && bytes[cursor] != quote {
                        if bytes[cursor] == b'\\' {
                            cursor += 1;
                        }
                        cursor += 1;
                    }
                    import = Some(text[start..cursor].to_ascii_lowercase());
                } else if bytes
                    .get(cursor..cursor.saturating_add(3))
                    .is_some_and(|word| word.eq_ignore_ascii_case(b"url"))
                {
                    cursor += 3;
                    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                        cursor += 1;
                    }
                    if bytes.get(cursor) == Some(&b'(') {
                        cursor += 1;
                        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                            cursor += 1;
                        }
                        let quote = bytes
                            .get(cursor)
                            .copied()
                            .filter(|byte| matches!(byte, b'\'' | b'\"'));
                        if quote.is_some() {
                            cursor += 1;
                        }
                        let start = cursor;
                        while cursor < bytes.len()
                            && quote.map_or(bytes[cursor] != b')', |quote| bytes[cursor] != quote)
                        {
                            cursor += 1;
                        }
                        import = Some(text[start..cursor].trim().to_ascii_lowercase());
                    }
                }
                if let Some(import) = import {
                    if import == "tailwindcss" || import.starts_with("tailwindcss/") {
                        found.push(format!("@import {import:?}"));
                    }
                }
            }
            index = name_end.max(index + 1);
            continue;
        }
        if matches!(bytes[index], b'\'' | b'\"') {
            let quote = bytes[index];
            index += 1;
            while index < bytes.len() && bytes[index] != quote {
                if bytes[index] == b'\\' {
                    index += 1;
                }
                index += 1;
            }
        }
        index += 1;
    }
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use zfb_css::url_attribution::PackageUrlAsset;

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

    fn mock_output(request: CompileRequest, output: &str) -> Result<CssEmitterOutput> {
        let engine = TailwindSubprocessEngine::new(request.tailwind.with_mock_output(output));
        run_css_emitter_without_modules(
            engine,
            &request.project_root,
            request.output.parent().unwrap(),
            request.framework_css,
        )
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("entry.css"), "@import \"tailwindcss\";\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn compiles_with_defaults_explicit_sourcing_and_css_modules_disabled() {
        let dir = project();
        let seen = RefCell::new(false);
        let emitter = MockEmitter(|request: CompileRequest| {
            *seen.borrow_mut() = true;
            assert_eq!(request.tailwind.working_dir, dir.path());
            assert!(request.tailwind.explicit_sourcing);
            assert_eq!(request.tailwind.content_globs.len(), 5);
            assert_eq!(
                request.tailwind.content_globs[0],
                dir.path().join("pages").to_string_lossy()
            );
            mock_output(request, "/*! tailwindcss v4.2.0 */\nbody {}\n")
        });
        run_from(&args(), dir.path(), &emitter).await.unwrap();
        assert!(*seen.borrow());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dist/out.css")).unwrap(),
            "/*! tailwindcss v4.2.0 */\nbody {}\n\n"
        );
        assert!(!dir.path().join("dist/css-modules").exists());
    }

    #[tokio::test]
    async fn paths_anchor_to_cwd_but_sources_anchor_to_project_root_and_exact_dedupe() {
        let outer = project();
        let root = outer.path().join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.tsx"), "className=\"p-4\"").unwrap();
        let mut command = args();
        command.project_root = Some(PathBuf::from("project"));
        command.source = vec!["src/**/*.tsx".into(), "src/**/*.tsx".into()];
        let emitter = MockEmitter(|request: CompileRequest| {
            assert_eq!(
                request.tailwind.input_css,
                Some(outer.path().join("entry.css"))
            );
            assert_eq!(request.tailwind.content_globs.len(), 6);
            assert_eq!(
                request.tailwind.content_globs[5],
                root.join("src/**/*.tsx").to_string_lossy()
            );
            mock_output(request, "body {}")
        });
        run_from(&command, outer.path(), &emitter).await.unwrap();
    }

    #[tokio::test]
    async fn no_auto_source_keeps_only_ordered_explicit_sources() {
        let dir = project();
        std::fs::write(dir.path().join("a.tsx"), "").unwrap();
        std::fs::write(dir.path().join("b.mdx"), "").unwrap();
        let mut command = args();
        command.no_auto_source = true;
        command.source = vec!["b.mdx".into(), "a.tsx".into()];
        let emitter = MockEmitter(|request: CompileRequest| {
            assert_eq!(
                request.tailwind.content_globs,
                [
                    dir.path().join("b.mdx").to_string_lossy().into_owned(),
                    dir.path().join("a.tsx").to_string_lossy().into_owned(),
                ]
            );
            mock_output(request, "body {}")
        });
        run_from(&command, dir.path(), &emitter).await.unwrap();
    }

    #[tokio::test]
    async fn explicit_source_validation_does_not_prune_dependency_directories() {
        let dir = project();
        std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        std::fs::write(dir.path().join("node_modules/pkg/index.js"), "").unwrap();
        let mut command = args();
        command.source = vec!["node_modules/pkg/**/*.js".into()];
        let emitter = MockEmitter(|request| mock_output(request, "body {}"));
        run_from(&command, dir.path(), &emitter).await.unwrap();
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
    async fn config_highlight_values_and_cli_overrides_are_applied_without_plugin_host() {
        let dir = project();
        std::fs::write(
            dir.path().join("zfb.config.json"),
            r#"{"tailwind":{"enabled":false},"plugins":[],"codeHighlight":{"mode":"class","classPrefix":"tok-","defaultStylesheet":true,"roleClasses":{"keyword":"text-red-500 text-bold"}}}"#,
        ).unwrap();
        let emitter = MockEmitter(|request: CompileRequest| {
            assert_eq!(
                request.tailwind.inline_sources,
                ["text-bold", "text-red-500"]
            );
            let framework = request.framework_css.as_deref().expect("class CSS");
            assert!(framework.contains(".tok-kw"));
            assert!(!framework.contains(".hi-kw"));
            mock_output(request, "body {}")
        });
        run_from(&args(), dir.path(), &emitter).await.unwrap();

        let mut command = args();
        command.code_highlight_mode = Some(CssCodeHighlightMode::Inline);
        let emitter = MockEmitter(|request: CompileRequest| {
            assert!(request.framework_css.is_none());
            mock_output(request, "body {}")
        });
        run_from(&command, dir.path(), &emitter).await.unwrap();
    }

    #[tokio::test]
    async fn class_override_uses_default_prefix_and_styles_can_be_disabled() {
        let dir = project();
        let mut command = args();
        command.code_highlight_mode = Some(CssCodeHighlightMode::Class);
        let emitter = MockEmitter(|request: CompileRequest| {
            assert!(request.framework_css.as_deref().unwrap().contains(".hi-kw"));
            mock_output(request, "body {}")
        });
        run_from(&command, dir.path(), &emitter).await.unwrap();

        command.no_default_highlight_styles = true;
        let emitter = MockEmitter(|request: CompileRequest| {
            assert!(request.framework_css.is_none());
            mock_output(request, "body {}")
        });
        run_from(&command, dir.path(), &emitter).await.unwrap();
    }

    #[tokio::test]
    async fn broken_pages_and_content_do_not_affect_css_compilation() {
        let dir = project();
        std::fs::create_dir_all(dir.path().join("pages")).unwrap();
        std::fs::create_dir_all(dir.path().join("content")).unwrap();
        std::fs::write(dir.path().join("pages/broken.tsx"), "<<< invalid TSX >>>").unwrap();
        std::fs::write(dir.path().join("content/broken.mdx"), "---\n: invalid\n---").unwrap();
        let emitter = MockEmitter(|request| mock_output(request, "body {}"));
        run_from(&args(), dir.path(), &emitter).await.unwrap();
    }

    #[tokio::test]
    async fn preflight_collects_missing_input_zero_glob_and_same_path() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = args();
        command.input = PathBuf::from("missing.css");
        command.output = PathBuf::from("missing.css");
        command.source = vec!["nothing/**/*.tsx".into()];
        let emitter = MockEmitter(|_| panic!("preflight failure must not emit"));
        let error = run_from(&command, dir.path(), &emitter)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot read CSS input"), "{error}");
        assert!(error.contains("matched zero files"), "{error}");
        assert!(error.contains("same path"), "{error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn canonical_input_output_alias_is_rejected_by_run() {
        let dir = project();
        std::os::unix::fs::symlink("entry.css", dir.path().join("alias.css")).unwrap();
        let mut command = args();
        command.output = PathBuf::from("alias.css");
        let emitter = MockEmitter(|_| panic!("same canonical path must not emit"));
        let error = run_from(&command, dir.path(), &emitter)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("same path"), "{error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn parent_components_after_symlinks_keep_filesystem_resolution_semantics() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("real/nested")).unwrap();
        std::fs::write(
            dir.path().join("real/entry.css"),
            "@import \"tailwindcss\";\n",
        )
        .unwrap();
        std::os::unix::fs::symlink("real/nested", dir.path().join("alias")).unwrap();

        let mut command = args();
        command.input = PathBuf::from("alias/../entry.css");
        let emitter = MockEmitter(|request: CompileRequest| {
            assert_eq!(
                request.tailwind.input_css,
                Some(dir.path().join("alias/../entry.css"))
            );
            mock_output(request, "body {}")
        });

        run_from(&command, dir.path(), &emitter).await.unwrap();
    }

    #[tokio::test]
    async fn unreadable_input_is_returned_from_run() {
        let dir = project();
        std::fs::remove_file(dir.path().join("entry.css")).unwrap();
        std::fs::create_dir(dir.path().join("entry.css")).unwrap();
        let emitter = MockEmitter(|_| panic!("unreadable input must not emit"));
        let error = run_from(&args(), dir.path(), &emitter)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot read CSS input"), "{error}");
    }

    #[tokio::test]
    async fn subprocess_failure_is_returned_and_preserves_existing_output() {
        let dir = project();
        std::fs::create_dir_all(dir.path().join("dist")).unwrap();
        std::fs::write(dir.path().join("dist/out.css"), "previous").unwrap();
        let emitter = MockEmitter(|_| Err(anyhow::anyhow!("mock tailwind subprocess failed")));
        let error = run_from(&args(), dir.path(), &emitter)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Tailwind CSS compilation failed"), "{error}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dist/out.css")).unwrap(),
            "previous"
        );
    }

    #[tokio::test]
    async fn companions_are_rejected_with_url_diagnostics_and_no_write() {
        let dir = project();
        let emitter = MockEmitter(|request| {
            let mut output = mock_output(request, "body { src: url(\"font-a1b2.woff2\") }")?;
            output.companions.push(PackageUrlAsset {
                filename: "font-a1b2.woff2".into(),
                bytes: vec![1, 2, 3],
            });
            Ok(output)
        });
        let error = run_from(&args(), dir.path(), &emitter)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("url(\"font-a1b2.woff2\")"), "{error}");
        assert!(!dir.path().join("dist/out.css").exists());
    }

    #[tokio::test]
    async fn unresolved_tailwind_tokens_are_rejected_but_external_imports_are_allowed() {
        let dir = project();
        let emitter = MockEmitter(|request| {
            mock_output(
                request,
                "/* @apply ignored; */\n.x::before { content: '@source ignored'; }\n@import \"https://example.com/a.css\";\n.x { @apply flex; }",
            )
        });
        let error = run_from(&args(), dir.path(), &emitter)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("@apply"), "{error}");
        assert!(!error.contains("example.com"), "{error}");
    }

    #[test]
    fn unresolved_scanner_is_token_aware_and_covers_all_tailwind_constructs() {
        assert!(unresolved_tailwind_directives(
            br#"/* @tailwind decoy */ .x{content:"@apply"} @TAILWIND base; @source "x"; @import 'tailwindcss/utilities';"#
        ).iter().any(|item| item == "@tailwind"));
        let clean = unresolved_tailwind_directives(
            br#"@import "https://example.com/tailwindcss";
                @import url("theme.css");
                .x { background: url(@apply); width: calc(1px + var(--@source)); }"#,
        );
        assert!(clean.is_empty(), "{clean:?}");
        let imports = unresolved_tailwind_directives(
            br#"@import /* active */ URL( 'tailwindcss/utilities' );"#,
        );
        assert_eq!(imports, ["@import \"tailwindcss/utilities\""]);
    }
}
