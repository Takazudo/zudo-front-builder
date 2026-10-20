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
                options.tsconfig = Some(rolldown_common::TsConfig::Manual(
                    engine_join(value)?.into(),
                ));
            } else if let Some(value) = arg.strip_prefix("--target=") {
                transform.target = Some(rolldown_common::Either::Left(value.to_string()));
            } else if let Some(value) = arg.strip_prefix("--jsx-import-source=") {
                jsx.import_source = Some(value.to_string());
                has_jsx = true;
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
                            Some(rolldown_common::CodeSplittingMode::Bool(true))
                    }
                    "--splitting=false" => {
                        options.code_splitting =
                            Some(rolldown_common::CodeSplittingMode::Bool(false))
                    }
                    "--keep-names" => options.keep_names = Some(true),
                    "--minify" => {
                        options.minify = Some(rolldown_common::RawMinifyOptions::Bool(true))
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
