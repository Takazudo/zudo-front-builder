//! Compatibility for the absolute trailing-directory substitutions emitted by
//! ZFB's prepared tsconfig. Keep mapping selection in the pinned resolver; only
//! repair its load-as-file/directory handling of the selected trailing target.
use anyhow::{bail, Result};
use rolldown::plugin::{
    HookResolveIdArgs, HookResolveIdOutput, HookUsage, Plugin, PluginContext,
    PluginContextResolveOptions,
};
use rolldown_common::TsConfig;
use rolldown_resolver::ResolveError;
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(super) struct TrailingPaths {
    config_path: Option<PathBuf>,
}

impl TrailingPaths {
    pub(super) fn new(options: &rolldown::BundlerOptions) -> Self {
        Self {
            config_path: match &options.tsconfig {
                Some(TsConfig::Manual(path)) => Some(path.clone()),
                _ => None,
            },
        }
    }
}

impl Plugin for TrailingPaths {
    fn name(&self) -> Cow<'static, str> {
        "zfb-prepared-trailing-paths".into()
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId
    }

    async fn resolve_id(
        &self,
        ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> Result<Option<HookResolveIdOutput>> {
        let Some(config_path) = &self.config_path else {
            return Ok(None);
        };
        let Some(importer) = args.importer else {
            return Ok(None);
        };
        // Match the native/esbuild rule: project paths do not apply to imports
        // originating inside node_modules. Absolute/relative imports likewise
        // belong to the ordinary resolver (including our delegated probes).
        if in_node_modules(Path::new(importer))
            || Path::new(args.specifier).is_absolute()
            || args.specifier.starts_with('.')
        {
            return Ok(None);
        }
        let config = ctx.resolver().resolve_tsconfig(config_path)?;
        // Prepared configs have no project references. Leave native reference
        // discovery untouched instead of applying the root's map to a child.
        if !config.references.is_empty() {
            return Ok(None);
        }
        let targets = config.resolve_path_alias_or_base_url(args.specifier);
        if !targets.iter().any(|path| trailing_separator(path)) {
            return Ok(None);
        }
        // A relative directory probe would also enter native rootDirs, which
        // can resurrect a candidate outside the prepared shadow. This narrow
        // adapter does not implement rootDirs compatibility; refuse before any
        // candidate lookup or ordinary-resolver fallback can publish output.
        if config
            .compiler_options
            .root_dirs
            .as_ref()
            .is_some_and(|roots| !roots.is_empty())
        {
            bail!("ZFB_ROLLDOWN_TRAILING_ROOT_DIRS_UNSUPPORTED: trailing-directory tsconfig mappings combined with rootDirs are unsupported by the experimental native backend: {}", args.specifier);
        }
        // Preserve the selected exact/wildcard mapping's complete target order:
        // an ordinary target before a trailing target must still win.
        for target in targets {
            let target = super::verbatim_specifiers::ordinary_target(target)?;
            if trailing_separator(&target) {
                // esbuild's Dir/Base split probes dir/basename as a FILE before
                // package main/index. A normal resolve of dir/basename could
                // instead enter a nested directory, so select only real files.
                if let Some(name) = target.file_name() {
                    if let Some(file) = file_candidate(ctx, importer, &target.join(name))? {
                        if let Some(resolved) = resolve(ctx, args, &file).await? {
                            return Ok(Some(resolved));
                        }
                    }
                }
                // The delegated directory request skips the outside sibling
                // (dir.js), while preserving mainFields,
                // package imports/type/sideEffects, and symlink policy. OXC's
                // tsconfig substitution path fails to do this at the pin.
            }
            if let Some(resolved) = resolve(ctx, args, &target).await? {
                return Ok(Some(resolved));
            }
        }
        Ok(None)
    }
}

async fn resolve(
    ctx: &PluginContext,
    args: &HookResolveIdArgs<'_>,
    target: &Path,
) -> Result<Option<HookResolveIdOutput>> {
    // An absolute request re-enters OXC's baseUrl substitution before its
    // directory-only path. Resolve a relative directory request from within
    // the selected directory instead: no mapping is applied a second time,
    // and this synthetic importer is never loaded or added to provenance.
    let directory_importer = target.join("__zfb_directory_probe__.js");
    let directory_importer = directory_importer.to_string_lossy();
    let target_text = target.to_string_lossy();
    let (specifier, importer) = if trailing_separator(target) {
        ("./", Some(directory_importer.as_ref()))
    } else {
        (target_text.as_ref(), args.importer)
    };
    let result = ctx
        .resolve(
            specifier,
            importer,
            Some(PluginContextResolveOptions {
                import_kind: args.kind,
                is_entry: args.is_entry,
                custom: args.custom.clone(),
                ..Default::default()
            }),
        )
        .await?;
    match result {
        Ok(resolved) => Ok(Some(HookResolveIdOutput::from_resolved_id(resolved))),
        Err(ResolveError::NotFound(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn in_node_modules(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str() == "node_modules")
}

fn trailing_separator(path: &Path) -> bool {
    let value = path.as_os_str().as_encoded_bytes();
    value.ends_with(b"/") || cfg!(windows) && value.ends_with(b"\\")
}

fn file_candidate(
    ctx: &PluginContext,
    importer: &str,
    path: &Path,
) -> Result<Option<std::path::PathBuf>> {
    // Keep native extension/alias/symlink resolution, but forbid descending
    // into a basename child directory. This clone shares the native cache.
    let resolver = ctx.resolver().clone_default_resolver();
    let mut options = resolver.options().clone();
    options.main_fields.clear();
    options.main_files.clear();
    let resolver = resolver.clone_with_options(options);
    match resolver.resolve_file(importer, &path.to_string_lossy()) {
        Ok(resolved) => Ok(Some(resolved.full_path())),
        Err(ResolveError::NotFound(_) | ResolveError::ExtensionAlias(..)) => Ok(None),
        // Let the ordinary plugin resolver preserve browser:false semantics.
        Err(ResolveError::Ignored(_)) => Ok(Some(path.to_owned())),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::{
        fs,
        path::PathBuf,
        process::Command,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "zfb-trailing-paths-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(fs::canonicalize(root).unwrap())
        }

        fn write(&self, path: &str, value: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value).unwrap();
        }

        fn target(&self, path: &str) -> String {
            self.0.join(path).to_string_lossy().into_owned()
        }

        fn config(&self, paths: Value) {
            self.write(
                "tsconfig.json",
                &json!({"compilerOptions":{"baseUrl":".","paths":paths}}).to_string(),
            );
        }

        fn run(&self, entry: &str) -> Result<(String, Value)> {
            self.write("entry.js", entry);
            let mut cmd = Command::new("unused-esbuild");
            cmd.current_dir(&self.0).args([
                "--bundle",
                "--platform=neutral",
                "--format=esm",
                "--main-fields=main",
                "--tsconfig=tsconfig.json",
                "--outfile=out.js",
                "--metafile=meta.json",
                "entry.js",
            ]);
            crate::native::run_prepared(&cmd)?;
            Ok((
                fs::read_to_string(self.0.join("out.js"))?,
                serde_json::from_slice(&fs::read(self.0.join("meta.json"))?)?,
            ))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn trailing_paths_use_child_then_main_then_index_without_outside_fallback() {
        let f = Fixture::new();
        f.config(json!({"mapped":[f.target("choice/")]}));
        f.write("choice.js", "export default 'OUTSIDE_SIBLING';");
        f.write("choice/package.json", r#"{"main":"./dist/main.js"}"#);
        f.write("choice/choice.ts", "export default 'CHILD_TS';");
        f.write("choice/choice.js", "export default 'CHILD_JS';");
        f.write("choice/dist/main.js", "export default 'PACKAGE_MAIN';");
        f.write("choice/index.js", "export default 'DIRECTORY_INDEX';");
        let entry = "export {default} from 'mapped';";
        for (winner, remove) in [
            ("CHILD_TS", "choice/choice.ts"),
            ("CHILD_JS", "choice/choice.js"),
            ("PACKAGE_MAIN", "choice/dist/main.js"),
            ("DIRECTORY_INDEX", "choice/index.js"),
        ] {
            let (body, graph) = f.run(entry).unwrap();
            assert!(body.contains(winner), "{body}");
            assert!(!body.contains("OUTSIDE_SIBLING"), "{body}");
            assert!(graph["inputs"].get(remove).is_some(), "{graph}");
            // The actual producer expresses exclusions as absent shadow files.
            fs::remove_file(f.0.join(remove)).unwrap();
        }
        // A basename child DIRECTORY must never enter the file-only probe.
        f.write(
            "choice/choice/index.js",
            "export default 'NESTED_DIRECTORY';",
        );
        fs::remove_file(f.0.join("out.js")).unwrap();
        assert!(f.run(entry).is_err());
        assert!(!f.0.join("out.js").exists());
    }

    #[test]
    fn trailing_paths_preserve_exact_wildcard_and_target_array_order() {
        let f = Fixture::new();
        f.config(json!({
            "alias:exact":[f.target("first.js"),f.target("wild/exact/")],
            "alias:fallback":[f.target("missing.js"),f.target("wild/fallback/"),f.target("last.js")],
            "alias:*": [f.target("wild/*/")]
        }));
        f.write("first.js", "export default 'EXACT_FIRST';");
        f.write("last.js", "export default 'ARRAY_LAST';");
        for name in ["exact", "fallback", "other"] {
            f.write(
                &format!("wild/{name}/package.json"),
                r#"{"main":"./main.js"}"#,
            );
            f.write(
                &format!("wild/{name}/main.js"),
                &format!("export default 'WILDCARD_{name}';"),
            );
        }
        let (body, _) = f
            .run("export {default as a} from 'alias:exact'; export {default as b} from 'alias:fallback'; export {default as c} from 'alias:other';")
            .unwrap();
        assert!(body.contains("EXACT_FIRST"), "{body}");
        assert!(!body.contains("WILDCARD_exact"), "{body}");
        assert!(body.contains("WILDCARD_fallback"), "{body}");
        assert!(body.contains("WILDCARD_other"), "{body}");
        assert!(!body.contains("ARRAY_LAST"), "{body}");
        fs::remove_file(f.0.join("wild/fallback/main.js")).unwrap();
        let (body, _) = f.run("export {default} from 'alias:fallback';").unwrap();
        assert!(body.contains("ARRAY_LAST"), "{body}");

        f.config(json!({"alias:exact":[f.target("wild/exact/")]}));
        f.write(
            "wild/exact/extra.js",
            "export default 'EXACT_SUFFIX_ESCAPE';",
        );
        assert!(f.run("export {default} from 'alias:exact/extra';").is_err());
    }

    #[test]
    fn trailing_paths_keep_package_imports_and_side_effect_metadata() {
        let f = Fixture::new();
        f.config(json!({"mapped":[f.target("pkg/")]}));
        f.write(
            "pkg/package.json",
            r##"{"main":"./main.js","type":"module","sideEffects":false,"imports":{"#internal":"./internal.js"}}"##,
        );
        f.write("pkg/internal.js", "export default 'PACKAGE_INTERNAL';");
        f.write(
            "pkg/main.js",
            "import value from '#internal'; console.log('REMOVABLE_PACKAGE_EFFECT'); export default value;",
        );
        let (body, graph) = f.run("export {default} from 'mapped';").unwrap();
        assert!(body.contains("PACKAGE_INTERNAL"), "{body}");
        assert!(graph["inputs"].get("pkg/internal.js").is_some(), "{graph}");
        let (body, _) = f
            .run("import 'mapped'; export const kept = 'KEPT';")
            .unwrap();
        assert!(body.contains("KEPT"), "{body}");
        assert!(!body.contains("REMOVABLE_PACKAGE_EFFECT"), "{body}");
    }

    #[test]
    fn trailing_paths_do_not_apply_project_mappings_inside_node_modules() {
        let f = Fixture::new();
        f.config(json!({"mapped":[f.target("project/")]}));
        f.write("project/index.js", "export default 'PROJECT_MAPPING';");
        f.write(
            "node_modules/consumer/index.js",
            "export {default} from 'mapped';",
        );
        f.write(
            "node_modules/mapped/index.js",
            "export default 'PACKAGE_MAPPING';",
        );
        let (body, _) = f.run("export {default} from 'consumer';").unwrap();
        assert!(body.contains("PACKAGE_MAPPING"), "{body}");
        assert!(!body.contains("PROJECT_MAPPING"), "{body}");
    }

    #[test]
    fn trailing_paths_reject_root_dirs_before_fallback_or_publication() {
        let f = Fixture::new();
        fs::create_dir_all(f.0.join("virtual/choice")).unwrap();
        f.write("real/choice/index.js", "export default 'ROOT_DIR_ESCAPE';");
        f.write(
            "tsconfig.json",
            &json!({"compilerOptions": {
                "rootDirs": [f.target("virtual"), f.target("real")],
                "paths": {"mapped": [f.target("virtual/choice/")]}
            }})
            .to_string(),
        );
        let error = f
            .run("export {default} from 'mapped';")
            .expect_err("trailing mapped directory must not escape through rootDirs");
        assert!(
            error
                .to_string()
                .contains("ZFB_ROLLDOWN_TRAILING_ROOT_DIRS_UNSUPPORTED"),
            "{error:#}"
        );
        assert!(!f.0.join("out.js").exists());
        assert!(!f.0.join("meta.json").exists());
    }
}
