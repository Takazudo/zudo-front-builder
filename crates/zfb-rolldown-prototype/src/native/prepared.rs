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
    /// The caller's spelling of the working directory. Every user-visible path
    /// (output files, metafile keys) keeps it.
    pub(super) cwd: PathBuf,
    /// The engine's working directory, `BundlerOptions.cwd`: physical outside
    /// Windows, because Rolldown's module ids are, and it spells ids relative
    /// to its cwd inside the hashed chunk bytes (`//#region <id>`, #4479).
    pub(super) physical_cwd: PathBuf,
    /// The output directory in the caller's spelling.
    pub(super) outdir: Option<PathBuf>,
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
        let physical_cwd = if windows {
            cwd.clone()
        } else {
            std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone())
        };
        let engine_spelling = |value: String| engine_spelling(value, windows);
        let logical_join =
            |value: &str| engine_spelling(cwd.join(value).to_string_lossy().into_owned());
        // Only the cwd prefix moves to its physical spelling: a symlinked
        // directory inside the project (a shadow-staged subtree) stays as
        // spelled, so relative imports still resolve beside the staged files.
        let engine_join = |value: &str| {
            let logical = cwd.join(value);
            let joined = if windows {
                physical_cwd.join(value)
            } else if let Ok(rest) = logical.strip_prefix(&cwd) {
                physical_cwd.join(rest)
            } else {
                physical_spelling(&logical)
            };
            engine_spelling(joined.to_string_lossy().into_owned())
        };
        let mut outdir = None;
        let mut options = BundlerOptions {
            cwd: Some(physical_cwd.clone()),
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
                let aliases = resolve.alias.get_or_insert_default();
                for (key, target) in alias_entries(k, engine_spelling(v.to_string())?, windows) {
                    aliases.push((key, vec![Some(target)]));
                }
            } else if let Some(value) = arg.strip_prefix("--external:") {
                if value.strip_suffix('*').unwrap_or(value).contains('*') {
                    bail!("Rolldown prototype only supports suffix external wildcards: {value}");
                }
                external.push(value.to_string());
            } else if let Some(value) = arg.strip_prefix("--outfile=") {
                options.file = Some(engine_join(value)?);
                outdir = PathBuf::from(logical_join(value)?)
                    .parent()
                    .map(Path::to_path_buf);
            } else if let Some(value) = arg.strip_prefix("--outdir=") {
                options.dir = Some(engine_join(value)?);
                outdir = Some(PathBuf::from(logical_join(value)?));
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
                        // only the entry after bundling (islands.js -> islands-<hash>.js,
                        // chunks verbatim). Merging common chunks folds code shared by
                        // the entry and a dynamic import into the entry chunk, so the
                        // lazy chunk imports "./islands.js" and 404s (#4444/#4448);
                        // redundant-load avoidance does the same for already-loaded code.
                        options
                            .experimental
                            .get_or_insert_default()
                            .chunk_optimization =
                            Some(rolldown_common::ChunkOptimizationOption::Options(
                                rolldown_common::ChunkOptimizationOptions {
                                    merge_common_chunks: false,
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
        if let Some(value) = subprocess_node_path(cmd, std::env::var_os("NODE_PATH")) {
            let modules = resolve
                .modules
                .get_or_insert_with(|| vec!["node_modules".into()]);
            for path in std::env::split_paths(&value) {
                modules.push(engine_spelling(path.to_string_lossy().into_owned())?);
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
            physical_cwd,
            outdir,
            options,
            metafile,
            file_extensions,
        })
    }
}

/// The `NODE_PATH` the esbuild subprocess would see: the command's own setting
/// or removal, otherwise the inherited value. The package manager's bin shim
/// exports one naming its hidden-hoist store (`node_modules/.pnpm/node_modules`),
/// which is how esbuild under `--preserve-symlinks` finds a dependency of a
/// package reached through a symlink outside the project (#4430).
fn subprocess_node_path(
    cmd: &Command,
    inherited: Option<std::ffi::OsString>,
) -> Option<std::ffi::OsString> {
    match cmd.get_envs().find(|(key, _)| *key == "NODE_PATH") {
        Some((_, value)) => value.map(ToOwned::to_owned),
        None => inherited,
    }
    .filter(|value| !value.is_empty())
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

/// `path` with its directory in physical spelling. The directory is walked
/// component by component: existing components are canonicalized, a missing
/// one starts a not-yet-existing tail (which cannot hold a symlink) where `..`
/// cancels lexically, and a `..` that climbs back out of that tail resumes
/// physical resolution from the existing prefix. The final component itself is
/// never resolved, so a symlinked entry or output keeps its own name, as
/// esbuild's `--preserve-symlinks` expects. Anything other than plain absence
/// (permission error, dangling symlink, non-directory component) cannot be
/// resolved soundly, so the logical spelling is returned unchanged.
fn physical_spelling(path: &Path) -> PathBuf {
    use std::io::ErrorKind;
    use std::path::Component;

    let (Some(name), Some(dir)) = (path.file_name(), path.parent()) else {
        return path.to_path_buf();
    };
    // A relative spelling starts from the physical cwd so a leading `..` is not dropped.
    let mut resolved = if dir.has_root() {
        PathBuf::new()
    } else if dir.as_os_str().is_empty() {
        return path.to_path_buf();
    } else {
        match std::fs::canonicalize(".") {
            Ok(cwd) => cwd,
            Err(_) => return path.to_path_buf(),
        }
    };
    let mut missing: Vec<&std::ffi::OsStr> = Vec::new();
    for component in dir.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                if missing.pop().is_none() {
                    // `resolved` is canonical, so its lexical parent is its physical parent.
                    resolved.pop();
                }
            }
            Component::Normal(part) => {
                if !missing.is_empty() {
                    missing.push(part);
                    continue;
                }
                let candidate = resolved.join(part);
                match std::fs::canonicalize(&candidate) {
                    Ok(real) if real.is_dir() => resolved = real,
                    Ok(_) => return path.to_path_buf(),
                    Err(error) if error.kind() == ErrorKind::NotFound => {
                        match std::fs::symlink_metadata(&candidate) {
                            Err(error) if error.kind() == ErrorKind::NotFound => missing.push(part),
                            _ => return path.to_path_buf(),
                        }
                    }
                    Err(_) => return path.to_path_buf(),
                }
            }
        }
    }
    let mut out = resolved;
    out.extend(missing);
    out.join(name)
}

/// Rolldown entries for one esbuild `--alias:<key>=<target>` (exact key plus
/// `<key>/` subpaths). Its resolver appends a prefix alias's subpath with
/// `Path::push`, so on Windows `zfb/zudo-react` under `zfb=@takazudo/zfb`
/// becomes the bare specifier `@takazudo\zfb\zudo-react`, which no package
/// resolves (`Matched alias not found`, run 40122653116). A bare-package
/// target is split into an exact entry and a `*` wildcard entry, whose
/// subpath is substituted as text. Path targets keep the prefix entry.
fn alias_entries(key: &str, target: String, windows: bool) -> Vec<(String, String)> {
    let bare_package = !target.is_empty()
        && !target.starts_with(['.', '/', '\\'])
        && !target.contains(['\\', ':', '*']);
    if !windows || !bare_package || key.is_empty() || key.ends_with('$') || key.contains('*') {
        return vec![(key.to_string(), target)];
    }
    vec![
        (format!("{key}$"), target.clone()),
        (format!("{key}/*"), format!("{target}/*")),
    ]
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

    fn aliases(cmd: &Command, windows: bool) -> Vec<(String, Vec<Option<String>>)> {
        let request = Request::decode_for_host(cmd, windows).unwrap();
        request.options.resolve.unwrap().alias.unwrap()
    }

    #[test]
    fn windows_host_splits_a_bare_package_alias_into_exact_and_wildcard_entries() {
        let mut cmd = prepared(r"\\?\C:\shadow");
        cmd.arg("--alias:zfb=@takazudo/zfb");
        let entry = |key: &str, target: &str| (key.to_string(), vec![Some(target.to_string())]);
        assert_eq!(
            aliases(&cmd, true),
            vec![
                entry("#shadow", r"C:\shadow\alias.mjs"),
                entry("zfb$", "@takazudo/zfb"),
                entry("zfb/*", "@takazudo/zfb/*"),
            ]
        );
        assert_eq!(
            aliases(&cmd, false),
            vec![
                entry("#shadow", r"\\?\C:\shadow\alias.mjs"),
                entry("zfb", "@takazudo/zfb"),
            ]
        );
    }

    #[test]
    fn alias_entries_keep_path_and_already_shaped_targets_whole() {
        for (key, target) in [
            ("#shadow", r"C:\shadow\alias.mjs"),
            ("#rel", "./alias.mjs"),
            ("#abs", "/srv/alias.mjs"),
            ("#unc", r"\\server\share\alias.mjs"),
            ("exact$", "@scope/pkg"),
            ("wild/*", "@scope/pkg/*"),
            ("#glob", "@scope/pkg/*"),
        ] {
            assert_eq!(
                super::alias_entries(key, target.to_string(), true),
                vec![(key.to_string(), target.to_string())]
            );
        }
    }

    /// The split shape must still resolve the exact key and its subpaths
    /// through a real installed package (exercised on every host).
    #[test]
    fn exact_and_wildcard_alias_entries_resolve_an_installed_package() {
        let dir = std::env::temp_dir().join(format!("zfb-native-alias-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pkg = dir.join("node_modules/@scope/pkg");
        std::fs::create_dir_all(pkg.join("sub")).unwrap();
        std::fs::write(
            pkg.join("package.json"),
            r#"{"name":"@scope/pkg","exports":{".":"./index.js","./sub":"./sub/index.js"}}"#,
        )
        .unwrap();
        std::fs::write(pkg.join("index.js"), "export const root = 'PKG_ROOT';\n").unwrap();
        std::fs::write(pkg.join("sub/index.js"), "export const sub = 'PKG_SUB';\n").unwrap();
        let cwd = std::fs::canonicalize(&dir).unwrap();
        std::fs::write(
            cwd.join("entry.mjs"),
            "import { root } from 'short';\nimport { sub } from 'short/sub';\nconsole.log(root, sub);\n",
        )
        .unwrap();
        let mut cmd = Command::new("unused-prepared-esbuild");
        cmd.current_dir(&cwd).args([
            "entry.mjs",
            "--bundle",
            "--format=esm",
            "--platform=neutral",
            "--outfile=out/bundle.mjs",
            "--alias:short$=@scope/pkg",
            "--alias:short/*=@scope/pkg/*",
        ]);
        let result = super::super::run_prepared(&cmd);
        let output = std::fs::read_to_string(cwd.join("out/bundle.mjs")).unwrap_or_default();
        std::fs::remove_dir_all(&cwd).unwrap();
        result.unwrap();
        assert!(
            output.contains("PKG_ROOT") && output.contains("PKG_SUB"),
            "{output}"
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
    fn node_path_follows_what_the_esbuild_subprocess_would_inherit() {
        use super::subprocess_node_path;
        let inherited = || Some(std::ffi::OsString::from("/store/.pnpm/node_modules"));
        let plain = Command::new("esbuild");
        assert_eq!(subprocess_node_path(&plain, inherited()), inherited());
        assert_eq!(subprocess_node_path(&plain, None), None);
        let mut set = Command::new("esbuild");
        set.env("NODE_PATH", "/lease/node_modules");
        assert_eq!(
            subprocess_node_path(&set, inherited()),
            Some("/lease/node_modules".into())
        );
        let mut removed = Command::new("esbuild");
        removed.env_remove("NODE_PATH");
        assert_eq!(subprocess_node_path(&removed, inherited()), None);
        let mut empty = Command::new("esbuild");
        empty.env("NODE_PATH", "");
        assert_eq!(subprocess_node_path(&empty, inherited()), None);
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

    #[cfg(unix)]
    mod physical_spelling_fixture {
        use super::super::physical_spelling;
        use std::fs;
        use std::os::unix::fs::symlink;
        use std::path::PathBuf;

        struct Tree(PathBuf);
        impl Tree {
            fn new(name: &str) -> Self {
                let root = std::env::temp_dir().join(format!(
                    "zfb-physical-spelling-{name}-{}",
                    std::process::id()
                ));
                let _ = fs::remove_dir_all(&root);
                fs::create_dir(&root).unwrap();
                // Only the fixture's own parent is canonicalized; the subject is `link`.
                let root = fs::canonicalize(root).unwrap();
                fs::create_dir(root.join("real")).unwrap();
                symlink(root.join("real"), root.join("link")).unwrap();
                Self(root)
            }
            fn at(&self, rel: &str) -> PathBuf {
                self.0.join(rel)
            }
        }
        impl Drop for Tree {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn dotdot_after_a_missing_component_cancels_lexically_under_a_symlinked_root() {
            let tree = Tree::new("missing-dotdot");
            let spelled = tree.at("link/build/../dist/out.js");
            assert_eq!(physical_spelling(&spelled), tree.at("real/dist/out.js"));
        }

        #[test]
        fn dotdot_climbing_back_into_existing_directories_resumes_physical_resolution() {
            let tree = Tree::new("resume");
            fs::create_dir_all(tree.at("elsewhere/sub")).unwrap();
            symlink(tree.at("elsewhere/sub"), tree.at("real/hop")).unwrap();
            // missing/.. cancels, `hop` is a symlink into elsewhere/sub, so
            // the following `..` is elsewhere, not real.
            let spelled = tree.at("link/missing/../hop/../out/f.js");
            assert_eq!(physical_spelling(&spelled), tree.at("elsewhere/out/f.js"));
        }

        #[test]
        fn existing_ancestor_and_final_symlink_component_keep_their_spelling() {
            let tree = Tree::new("final");
            fs::write(tree.at("real/target.js"), "").unwrap();
            symlink(tree.at("real/target.js"), tree.at("real/entry.js")).unwrap();
            assert_eq!(
                physical_spelling(&tree.at("link/newdir/f.js")),
                tree.at("real/newdir/f.js")
            );
            assert_eq!(
                physical_spelling(&tree.at("link/entry.js")),
                tree.at("real/entry.js")
            );
        }

        #[test]
        fn dangling_symlink_and_non_directory_components_stay_logical() {
            let tree = Tree::new("uncertain");
            symlink(tree.at("nowhere"), tree.at("real/dangling")).unwrap();
            fs::write(tree.at("real/file"), "").unwrap();
            for spelled in [
                tree.at("link/dangling/../x/f.js"),
                tree.at("link/file/../x/f.js"),
            ] {
                assert_eq!(physical_spelling(&spelled), spelled);
            }
        }

        #[test]
        fn decode_spells_an_outside_cwd_outdir_physically() {
            let tree = Tree::new("decode");
            fs::create_dir(tree.at("cwd")).unwrap();
            let outdir = tree.at("link/build/../dist");
            let mut cmd = super::Command::new("esbuild");
            cmd.current_dir(tree.at("cwd"))
                .args(["entry.mjs", "--bundle"])
                .arg(format!("--outdir={}", outdir.display()));
            let request = super::Request::decode_for_host(&cmd, false).unwrap();
            let expected = tree.at("real/dist");
            assert_eq!(
                request.options.dir.as_deref(),
                Some(&*expected.to_string_lossy())
            );
            // The user-visible outdir keeps the caller's spelling.
            assert_eq!(request.outdir.as_deref(), Some(outdir.as_path()));
        }
    }
}
