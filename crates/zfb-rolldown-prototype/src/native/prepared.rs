//! Private decoder for ZFB's existing prepared esbuild command contract.
//! Fail closed on unknown options; this remains explicit argv ownership debt.
use super::windows_path;
use anyhow::{anyhow, bail, Context, Result};
use rolldown::BundlerOptions;
use rolldown_common::ModuleType;
use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

/// Owned boundary: the already prepared job's policies, not a public backend API.
/// The argv decoder intentionally rejects options outside this prototype's scope.
pub(super) struct Request {
    pub(super) cwd: PathBuf,
    pub(super) options: BundlerOptions,
    pub(super) metafile: Option<PathBuf>,
    pub(super) file_extensions: Vec<String>,
}

impl Request {
    pub(super) fn decode(cmd: &Command) -> Result<Self> {
        Self::decode_for_host(cmd, cfg!(windows))
    }

    fn decode_for_host(cmd: &Command, windows: bool) -> Result<Self> {
        let cwd = cmd
            .get_current_dir()
            .map(Path::to_path_buf)
            .unwrap_or(std::env::current_dir()?);
        let cwd = if windows {
            let cwd = cwd.into_os_string().into_string().map_err(|cwd| {
                anyhow!(
                    "Rolldown prototype requires a UTF-8 working directory: {}",
                    cwd.to_string_lossy()
                )
            })?;
            PathBuf::from(engine_spelling(cwd, windows)?)
        } else {
            cwd
        };
        let engine_spelling = |value: String| engine_spelling(value, windows);
        let engine_join =
            |value: &str| engine_spelling(cwd.join(value).to_string_lossy().into_owned());
        let mut options = BundlerOptions {
            cwd: Some(cwd.clone()),
            code_splitting: Some(rolldown_common::CodeSplittingMode::Bool(false)),
            // Resource admission separates the source name from its hash at
            // the final '-'. Base64 hashes can themselves end in '-' or
            // contain that delimiter; use the pinned alphanumeric alphabet
            // without changing the existing resource admission contract.
            hash_characters: Some(rolldown_common::HashCharacters::Base36),
            // Esbuild runs no dead-code pass without --minify. Rolldown's default
            // DCE-only pass also drops bindings after an unconditional top-level
            // throw while keeping their export entries (#4336 ZFB-RD-003).
            minify: Some(rolldown_common::RawMinifyOptions::Bool(false)),
            // Esbuild drops module-level directives such as an island's
            // "use client" marker without a warning (#4404).
            checks: Some(rolldown_common::ChecksOptions {
                module_level_directive: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut resolve = rolldown::ResolveOptions::default();
        let mut definitions = Vec::new();
        let mut loaders = BTreeMap::new();
        let mut external = Vec::<String>::new();
        let mut metafile = None;
        let mut entry = None;
        let mut transform = rolldown_common::BundlerTransformOptions::default();
        let mut jsx = rolldown_common::JsxOptions::default();
        let mut has_jsx = false;
        let mut tsconfig = None;
        for arg in cmd.get_args() {
            let arg = arg
                .to_str()
                .context("prototype requires UTF-8 prepared arguments")?;
            if let Some(value) = arg.strip_prefix("--define:") {
                let (k, v) = value.split_once('=').context("define")?;
                definitions.push((k.to_string(), v.to_string()));
            } else if let Some(value) = arg.strip_prefix("--loader:") {
                let (k, v) = value.split_once('=').context("loader")?;
                loaders.insert(k.to_string(), v.to_string());
            } else if let Some(value) = arg.strip_prefix("--alias:") {
                let (k, v) = value.split_once('=').context("alias")?;
                resolve
                    .alias
                    .get_or_insert_default()
                    .push((k.to_string(), vec![Some(engine_spelling(v.to_string())?)]));
            } else if let Some(value) = arg.strip_prefix("--external:") {
                if value.strip_suffix('*').unwrap_or(value).contains('*') {
                    bail!("Rolldown prototype only supports suffix external wildcards: {value}");
                }
                external.push(value.to_string());
            } else if let Some(value) = arg.strip_prefix("--outfile=") {
                options.file = Some(engine_join(value)?);
            } else if let Some(value) = arg.strip_prefix("--outdir=") {
                options.dir = Some(engine_join(value)?);
            } else if let Some(value) = arg.strip_prefix("--entry-names=") {
                options.entry_filenames = Some(format!("{value}.js").into());
            } else if let Some(value) = arg.strip_prefix("--chunk-names=") {
                options.chunk_filenames = Some(format!("{value}.js").into());
            } else if let Some(value) = arg.strip_prefix("--asset-names=") {
                options.asset_filenames = Some(format!("{value}[extname]").into());
            } else if let Some(value) = arg.strip_prefix("--metafile=") {
                metafile = Some(cwd.join(value));
            } else if let Some(value) = arg.strip_prefix("--tsconfig=") {
                let path = PathBuf::from(engine_join(value)?);
                options.tsconfig = Some(rolldown_common::TsConfig::Manual(path.clone()));
                tsconfig = Some(path);
            } else if let Some(value) = arg.strip_prefix("--target=") {
                transform.target = Some(rolldown_common::Either::Left(value.to_string()));
            } else if let Some(value) = arg.strip_prefix("--jsx-import-source=") {
                jsx.import_source = Some(value.to_string());
                has_jsx = true;
            } else if let Some(value) = arg.strip_prefix("--conditions=") {
                // Esbuild adds custom conditions to its defaults; Rolldown's
                // resolver extends its own defaults the same way.
                resolve.condition_names = Some(value.split(',').map(str::to_string).collect());
            } else if let Some(value) = arg.strip_prefix("--main-fields=") {
                resolve.main_fields = Some(value.split(',').map(str::to_string).collect());
            } else {
                match arg {
                    "--bundle"
                    | "--tree-shaking=true"
                    | "--log-level=warning"
                    | "--log-limit=0" => {}
                    "--format=esm" => options.format = Some(rolldown_common::OutputFormat::Esm),
                    "--platform=browser" => {
                        options.platform = Some(rolldown_common::Platform::Browser)
                    }
                    "--platform=neutral" => {
                        options.platform = Some(rolldown_common::Platform::Neutral);
                        resolve.main_fields.get_or_insert_default();
                    }
                    "--splitting" | "--splitting=true" => {
                        options.code_splitting =
                            Some(rolldown_common::CodeSplittingMode::Bool(true));
                        // Esbuild never makes a chunk import an entry, and zfb renames
                        // entries after bundling. Rolldown keeps code shared with an
                        // already-loaded entry inside that entry unless this is off.
                        options
                            .experimental
                            .get_or_insert_default()
                            .chunk_optimization =
                            Some(rolldown_common::ChunkOptimizationOption::Options(
                                rolldown_common::ChunkOptimizationOptions {
                                    merge_common_chunks: true,
                                    avoid_redundant_chunk_loads: false,
                                },
                            ));
                    }
                    "--splitting=false" => {
                        options.code_splitting =
                            Some(rolldown_common::CodeSplittingMode::Bool(false))
                    }
                    "--keep-names" => options.keep_names = Some(true),
                    "--minify" => {
                        options.minify = Some(rolldown_common::RawMinifyOptions::Bool(true));
                        // Rolldown's compressor drops statements after an unconditional
                        // top-level throw, re-hoisting only var bindings while keeping the
                        // export entries, so an exported const loses its declaration
                        // (#4336 ZFB-RD-015). Esbuild bundles top-level let/const/class as
                        // var in every mode; only this mode runs the compressor.
                        options.top_level_var = Some(true);
                    }
                    "--sourcemap=linked" => {
                        options.sourcemap = Some(rolldown_common::SourceMapType::File)
                    }
                    "--preserve-symlinks" => resolve.symlinks = Some(false),
                    "--jsx=automatic" => {
                        jsx.runtime = Some("automatic".into());
                        has_jsx = true;
                    }
                    _ if !arg.starts_with('-') && entry.is_none() => {
                        entry = Some(engine_join(arg)?)
                    }
                    _ => bail!("Rolldown prototype does not implement prepared option {arg:?}"),
                }
            }
        }
        // Esbuild's NODE_PATH fallback is an ordered additional module search path.
        for (k, v) in cmd.get_envs() {
            if k == "NODE_PATH" {
                if let Some(v) = v {
                    let modules = resolve
                        .modules
                        .get_or_insert_with(|| vec!["node_modules".into()]);
                    for path in std::env::split_paths(v) {
                        modules.push(engine_spelling(path.to_string_lossy().into_owned())?);
                    }
                }
            }
        }
        options.input = Some(vec![entry.context("prepared entry")?.into()]);
        options.resolve = Some(resolve);
        options.define = Some(definitions.into_iter().collect());
        options.external = Some(rolldown_common::IsExternal::Fn(Some(Arc::new(
            move |specifier, _, _| {
                let found = external.iter().any(|p| {
                    if let Some(prefix) = p.strip_suffix('*') {
                        specifier.starts_with(prefix)
                    } else {
                        specifier == p
                            || specifier
                                .strip_prefix(p)
                                .is_some_and(|rest| rest.starts_with('/'))
                    }
                });
                Box::pin(async move { Ok(found) })
            },
        ))));
        let mut file_extensions = Vec::new();
        let mut types = Vec::new();
        for (ext, loader) in loaders {
            if loader == "file" {
                file_extensions.push(ext);
            } else {
                types.push((ext, ModuleType::from_known_str(&loader)?));
            }
        }
        options.module_types = Some(types.into_iter().collect());
        if let Some(path) = &tsconfig {
            if jsx.runtime.is_none() || jsx.import_source.is_none() {
                has_jsx |= promote_tsconfig_jsx(path, &mut jsx)?;
            }
        }
        if has_jsx {
            transform.jsx = Some(rolldown_common::Either::Right(jsx));
        }
        options.transform = Some(transform);
        Ok(Self {
            cwd,
            options,
            metafile,
            file_extensions,
        })
    }
}

/// Esbuild applies a `--tsconfig` override's JSX settings to every module,
/// while Rolldown's per-file tsconfig lookup skips paths inside `node_modules`
/// (#4407). Lift the JSX fields argv left unset into the explicit transform
/// options, which govern every module; per-file pragmas still win.
fn promote_tsconfig_jsx(path: &Path, jsx: &mut rolldown_common::JsxOptions) -> Result<bool> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // Rolldown reports a missing manual tsconfig itself.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("read prepared tsconfig {}", path.display()))
        }
    };
    let config: serde_json::Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "Rolldown prototype requires a strict-JSON prepared tsconfig: {}",
            path.display()
        )
    })?;
    let options = config.get("compilerOptions");
    let field = |name: &str| options.and_then(|options| options.get(name));
    let runtime = match field("jsx") {
        None => None,
        Some(value) if value.as_str() == Some("react-jsx") => Some("automatic"),
        Some(value) => bail!(
            "Rolldown prototype only supports compilerOptions.jsx \"react-jsx\" in a prepared tsconfig, found {value} in {}",
            path.display()
        ),
    };
    let import_source = match field("jsxImportSource") {
        None => None,
        Some(serde_json::Value::String(value)) => Some(value.clone()),
        Some(value) => bail!(
            "compilerOptions.jsxImportSource must be a string, found {value} in {}",
            path.display()
        ),
    };
    if runtime.is_none() && import_source.is_none() {
        if config.get("extends").is_some() {
            bail!(
                "Rolldown prototype does not follow extends for JSX options in a prepared tsconfig: {}",
                path.display()
            );
        }
        return Ok(false);
    }
    let mut promoted = false;
    if jsx.runtime.is_none() {
        if let Some(runtime) = runtime {
            jsx.runtime = Some(runtime.into());
            promoted = true;
        }
    }
    if jsx.import_source.is_none() {
        if let Some(import_source) = import_source {
            jsx.import_source = Some(import_source);
            promoted = true;
        }
    }
    Ok(promoted)
}

/// Every path Rolldown resolves against must be in the ordinary spelling on
/// Windows; elsewhere `\\?\` is an ordinary file name and stays untouched.
fn engine_spelling(value: String, windows: bool) -> Result<String> {
    if !windows {
        return Ok(value);
    }
    Ok(match windows_path::for_rolldown(&value)? {
        Cow::Borrowed(_) => value,
        Cow::Owned(ordinary) => ordinary,
    })
}

#[cfg(test)]
mod tests {
    use super::Request;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn prepared(cwd: &str) -> Command {
        let mut cmd = Command::new("esbuild");
        cmd.current_dir(cwd)
            .args(["entry.mjs", "--bundle", "--format=esm", "--outdir=out"])
            .args(["--outfile=out/route.js", "--tsconfig=tsconfig.json"])
            .arg(r"--alias:#shadow=\\?\C:\shadow\alias.mjs")
            // A verbatim UNC entry has no ':' or ';', so it splits the same way on every host.
            .env("NODE_PATH", r"\\?\UNC\host\share\node_modules");
        cmd
    }

    #[test]
    fn windows_host_hands_rolldown_the_ordinary_shadow_spelling() {
        let request = Request::decode_for_host(&prepared(r"\\?\C:\shadow"), true).unwrap();
        let options = &request.options;
        assert_eq!(request.cwd, PathBuf::from(r"C:\shadow"));
        assert_eq!(options.cwd.as_deref(), Some(Path::new(r"C:\shadow")));
        let input = &options.input.as_ref().unwrap()[0].import;
        assert_eq!(
            input,
            &Path::new(r"C:\shadow").join("entry.mjs").to_string_lossy()
        );
        let joined = |rel: &str| Path::new(r"C:\shadow").join(rel);
        assert_eq!(
            options.dir.as_deref(),
            Some(&*joined("out").to_string_lossy())
        );
        assert_eq!(
            options.file.as_deref(),
            Some(&*joined("out/route.js").to_string_lossy())
        );
        assert!(matches!(
            &options.tsconfig,
            Some(rolldown_common::TsConfig::Manual(path)) if *path == joined("tsconfig.json")
        ));
        let modules = options.resolve.as_ref().unwrap().modules.as_ref().unwrap();
        assert!(modules.contains(&r"\\host\share\node_modules".to_string()));
        assert_eq!(
            options.resolve.as_ref().unwrap().alias.as_ref().unwrap()[0].1,
            vec![Some(r"C:\shadow\alias.mjs".to_string())]
        );
    }

    struct TsconfigDir(PathBuf);
    impl TsconfigDir {
        fn new(name: &str, tsconfig: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("zfb-prepared-jsx-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("tsconfig.json"), tsconfig).unwrap();
            Self(dir)
        }
        fn decode(&self, flags: &[&str]) -> anyhow::Result<Request> {
            let mut cmd = Command::new("esbuild");
            cmd.current_dir(&self.0)
                .args(["entry.mjs", "--bundle", "--tsconfig=tsconfig.json"])
                .args(flags);
            Request::decode_for_host(&cmd, false)
        }
    }
    impl Drop for TsconfigDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn jsx(request: &Request) -> &rolldown_common::JsxOptions {
        match request.options.transform.as_ref().unwrap().jsx.as_ref() {
            Some(rolldown_common::Either::Right(jsx)) => jsx,
            other => panic!("expected explicit jsx options, got {other:?}"),
        }
    }

    const SYNTHETIC: &str =
        r#"{"compilerOptions":{"jsx":"react-jsx","jsxImportSource":"@takazudo/zfb/zudo-react"}}"#;

    #[test]
    fn tsconfig_jsx_fields_argv_leaves_unset_become_explicit() {
        let dir = TsconfigDir::new("promote", SYNTHETIC);
        let request = dir.decode(&[]).unwrap();
        assert_eq!(jsx(&request).runtime.as_deref(), Some("automatic"));
        assert_eq!(
            jsx(&request).import_source.as_deref(),
            Some("@takazudo/zfb/zudo-react")
        );
        let request = dir.decode(&["--jsx=automatic"]).unwrap();
        assert_eq!(
            jsx(&request).import_source.as_deref(),
            Some("@takazudo/zfb/zudo-react")
        );
    }

    #[test]
    fn explicit_jsx_flags_win_without_reading_the_tsconfig() {
        let dir = TsconfigDir::new("explicit", "{ // JSONC the decoder must not parse\n}");
        let request = dir
            .decode(&["--jsx=automatic", "--jsx-import-source=preact"])
            .unwrap();
        assert_eq!(jsx(&request).import_source.as_deref(), Some("preact"));
    }

    #[test]
    fn unsupported_tsconfig_jsx_settings_fail_closed_naming_the_file() {
        for (name, tsconfig) in [
            ("preserve", r#"{"compilerOptions":{"jsx":"preserve"}}"#),
            ("classic", r#"{"compilerOptions":{"jsx":"react"}}"#),
            ("extends", r#"{"extends":"./base.json"}"#),
            ("jsonc", "{ // comment\n}"),
        ] {
            let dir = TsconfigDir::new(name, tsconfig);
            let error = format!("{:#}", dir.decode(&[]).err().expect(name));
            assert!(error.contains("tsconfig.json"), "{name}: {error}");
        }
    }

    #[test]
    fn a_tsconfig_without_jsx_fields_adds_no_jsx_options() {
        let dir = TsconfigDir::new("plain", r#"{"compilerOptions":{"baseUrl":"."}}"#);
        let request = dir.decode(&[]).unwrap();
        assert!(request.options.transform.as_ref().unwrap().jsx.is_none());
    }

    #[test]
    fn windows_host_refuses_a_verbatim_shadow_it_cannot_rewrite() {
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        let error = Request::decode_for_host(&prepared(&long), true)
            .err()
            .expect("an extended-length shadow must not reach Rolldown");
        assert!(format!("{error:#}").contains("longer than MAX_PATH"));
    }

    #[test]
    fn other_hosts_keep_backslash_question_names_verbatim() {
        let request = Request::decode_for_host(&prepared(r"\\?\C:\shadow"), false).unwrap();
        assert_eq!(request.cwd, PathBuf::from(r"\\?\C:\shadow"));
        assert_eq!(
            request
                .options
                .resolve
                .as_ref()
                .unwrap()
                .alias
                .as_ref()
                .unwrap()[0]
                .1,
            vec![Some(r"\\?\C:\shadow\alias.mjs".to_string())]
        );
    }
}
