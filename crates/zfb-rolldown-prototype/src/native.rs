#[cfg(test)]
mod canonical_path_tests;
mod copy_reference;
#[cfg(test)]
mod css_tests;
mod external_imports;
#[cfg(test)]
mod external_imports_tests;
mod handled_imports;
#[cfg(test)]
mod jsx_tsconfig_tests;
#[cfg(test)]
mod metafile_order_tests;
pub(crate) mod module_specifiers;
mod prepared;
#[cfg(test)]
mod splitting_tests;
mod trailing_paths;
#[cfg(test)]
mod unresolved_tests;
mod verbatim_specifiers;
#[cfg(test)]
mod warning_tests;
mod windows_path;
use anyhow::{bail, Context, Result};
use prepared::Request;
use rolldown::plugin::{
    HookBuildEndArgs, HookLoadArgs, HookLoadOutput, HookResolveFileUrlArgs, HookTransformArgs,
    HookTransformOutput, HookUsage, Plugin, PluginContext, SharedLoadPluginContext,
    SharedTransformPluginContext,
};
use rolldown::Bundler;
use rolldown_common::{EmittedAsset, ModuleType, Output};
use serde_json::{json, Value};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::Instant,
};

/// Rolldown is the default bundler wherever this backend is compiled in.
/// `ZFB_BUNDLER=esbuild` selects the esbuild subprocess instead, and the
/// opt-in-era `ZFB_ROLLDOWN_PROTOTYPE=0` still does when `ZFB_BUNDLER` is unset.
/// The `zfb` CLI rejects any other `ZFB_BUNDLER` value before a build starts.
pub fn enabled() -> bool {
    match std::env::var_os("ZFB_BUNDLER") {
        Some(value) => value != "esbuild",
        None => std::env::var_os("ZFB_ROLLDOWN_PROTOTYPE").is_none_or(|value| value != "0"),
    }
}

#[derive(Debug)]
struct GraphPlugin {
    inputs: Arc<Mutex<BTreeMap<String, Value>>>,
    file_extensions: Vec<String>,
    copy_outputs: Arc<Mutex<BTreeMap<String, String>>>,
}
impl Plugin for GraphPlugin {
    fn name(&self) -> Cow<'static, str> {
        "zfb-prototype-provenance".into()
    }
    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::BuildEnd | HookUsage::Load | HookUsage::ResolveFileUrl | HookUsage::Transform
    }
    async fn transform(
        &self,
        _ctx: SharedTransformPluginContext,
        args: &HookTransformArgs<'_>,
    ) -> Result<Option<HookTransformOutput>> {
        // Inspect the effective native loader, not a filename guess: CSS
        // deliberately loaded as a file or text remains a supported resource.
        // Raw CSS is outside this experiment's contract at the pinned engine.
        if matches!(args.module_type, ModuleType::Css) {
            bail!(
                "ZFB_ROLLDOWN_RAW_CSS_UNSUPPORTED: the Rolldown bundler does not bundle raw CSS yet; set ZFB_BUNDLER=esbuild to build this project: {}",
                args.id
            );
        }
        Ok(None)
    }
    async fn load(
        &self,
        ctx: SharedLoadPluginContext,
        args: &HookLoadArgs<'_>,
    ) -> Result<Option<HookLoadOutput>> {
        // ZFB's resource contract permits exactly the terminal ?url suffix.
        // Keep the resolved ID (including the suffix) in native provenance,
        // but read the underlying file and derive its output extension from it.
        let file_id = args.id.strip_suffix("?url").unwrap_or(args.id);
        if !self
            .file_extensions
            .iter()
            .any(|ext| file_id.ends_with(ext))
        {
            return Ok(None);
        }
        let path = Path::new(file_id);
        let reference = ctx
            .emit_file_async(EmittedAsset {
                name: Some(
                    path.file_name()
                        .context("resource filename")?
                        .to_string_lossy()
                        .into_owned(),
                ),
                original_file_name: Some(args.id.to_string()),
                source: std::fs::read(path)?.into(),
                ..Default::default()
            })
            .await?;
        Ok(Some(HookLoadOutput {
            code: format!("export default import.meta.ROLLDOWN_FILE_URL_{reference};").into(),
            module_type: Some(ModuleType::Js),
            ..Default::default()
        }))
    }
    async fn resolve_file_url(
        &self,
        _ctx: &PluginContext,
        args: &HookResolveFileUrlArgs<'_>,
    ) -> Result<Option<String>> {
        // Match ZFB's file-loader contract: a relative URL string, resolved by the
        // consuming runtime against import.meta.url, not an eagerly absolute URL.
        Ok(Some(serde_json::to_string(&format!(
            "./{}",
            args.relative_path
        ))?))
    }
    async fn build_end(
        &self,
        ctx: &PluginContext,
        _args: Option<&HookBuildEndArgs<'_>>,
    ) -> Result<()> {
        let mut inputs = self.inputs.lock().unwrap();
        let mut copy_outputs = self.copy_outputs.lock().unwrap();
        for id in ctx.get_module_ids() {
            if let Some(reference) = copy_reference::reference(id.as_str())? {
                copy_outputs.insert(id.to_string(), ctx.get_file_name(reference)?.to_string());
                // Actual source nodes are installed from emitted asset provenance below.
                continue;
            }
            let Some(info) = ctx.get_module_info(&id) else {
                bail!("missing resolved module info: {id}")
            };
            // External modules carry module info without code. Esbuild's
            // metafile lists them only as external imports, never as inputs.
            if info.code.is_none() {
                continue;
            }
            let mut imports = Vec::new();
            for (ids, kind) in [
                (&info.imported_ids, "import-statement"),
                (&info.dynamically_imported_ids, "dynamic-import"),
            ] {
                for imported in ids {
                    if let Some(reference) = copy_reference::reference(imported.as_str())? {
                        let filename = ctx.get_file_name(reference)?.to_string();
                        copy_outputs.insert(imported.to_string(), filename.clone());
                        imports.push(json!({"path": imported.as_str(), "kind":kind, "external":false, "zfbCopiedOutput":filename}));
                    } else {
                        let external = ctx
                            .get_module_info(imported)
                            .is_none_or(|info| info.code.is_none());
                        imports.push(
                            json!({"path": imported.as_str(), "kind":kind, "external": external}),
                        );
                    }
                }
            }
            // Keep source order, as esbuild does: island graph identity walks
            // imports in this order, so sorting by resolved path would make the
            // build token depend on where packages are installed.
            inputs.insert(
                id.to_string(),
                json!({"bytes":info.code.as_ref().map_or(0, |s|s.len()), "imports": imports}),
            );
        }
        Ok(())
    }
}

/// Execute one selected prepared job in process. Runtime construction is isolated
/// on a scoped thread, so callers already running a Tokio/V8 host remain valid.
pub fn run_prepared(cmd: &Command) -> Result<()> {
    let request = Request::decode(cmd)?;
    let files = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()?
                    .block_on(run(request))
            })
            .join()
            .map_err(|_| anyhow::anyhow!("Rolldown prototype thread panicked"))?
    })?;
    // Native warnings and graph validation may reject an otherwise generated
    // bundle. Publish only validated bytes after the scoped worker and its
    // private runtime have actually joined. This synchronous adapter does not
    // provide the standalone probe's supervisor or cancellation deadlines.
    for (path, bytes) in files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)
            .with_context(|| format!("write native output {}", path.display()))?;
    }
    Ok(())
}
/// Re-render unresolved imports in esbuild's shape, with paths relative to the
/// job's working directory, so the callers' existing esbuild diagnostic mapping
/// (shadow path to project path, out-of-root hints) applies unchanged. Returns
/// `None` unless every diagnostic is an unresolved import it can parse.
fn esbuild_style_unresolved(rendered: &[String], cwd: &Path) -> Option<String> {
    let process_cwd = std::env::current_dir().ok()?;
    let mut report = String::new();
    for diagnostic in rendered {
        let (_, rest) = diagnostic.split_once("[UNRESOLVED_IMPORT] Could not resolve '")?;
        let (specifier, rest) = rest.split_once("' in ")?;
        let (path, row, col, frame) = match diagnostic.split_once("╭─[ ") {
            Some((_, location)) => {
                let (location, _) = location.split_once(" ]")?;
                let mut parts = location.rsplitn(3, ':');
                // Rolldown's column is 1-based; esbuild's is 0-based.
                let col = parts.next()?.parse::<usize>().ok()?.checked_sub(1)?;
                let row = parts.next()?.parse::<usize>().ok()?;
                let path = lexical_normal(&process_cwd.join(parts.next()?));
                let frame = esbuild_code_frame(&path, row, col, specifier.chars().count() + 2);
                (path, row, col, frame)
            }
            // An automatic-runtime import has no source span; name the pragma when present.
            None => {
                let importer = rest.lines().next()?.trim();
                let path = lexical_normal(&process_cwd.join(importer));
                let (row, col) = jsx_import_source_position(&path);
                (path, row, col, String::new())
            }
        };
        let shown = path.strip_prefix(cwd).unwrap_or(&path);
        report.push_str(&format!(
            "\u{2718} [ERROR] Could not resolve \"{specifier}\"\n\n    {}:{row}:{col}:\n{frame}\n",
            shown.display()
        ));
    }
    (!report.is_empty()).then(|| report.trim_end().to_string())
}

/// esbuild's source excerpt under a location: the 1-based `row` of `path`, then a
/// `~` underline of `width` characters from the 0-based `col`. Empty when the
/// file or row cannot be read.
fn esbuild_code_frame(path: &Path, row: usize, col: usize, width: usize) -> String {
    let source = std::fs::read_to_string(path).unwrap_or_default();
    let Some(line) = row
        .checked_sub(1)
        .and_then(|index| source.lines().nth(index))
    else {
        return String::new();
    };
    let gutter = row.to_string().len().max(3);
    let width = width.min(line.chars().count().saturating_sub(col)).max(1);
    format!(
        "    {row:>gutter$} \u{2502} {line}\n    {:gutter$} \u{2575} {:col$}{}\n",
        "",
        "",
        "~".repeat(width)
    )
}

fn jsx_import_source_position(path: &Path) -> (usize, usize) {
    let source = std::fs::read_to_string(path).unwrap_or_default();
    source
        .lines()
        .enumerate()
        .find_map(|(row, line)| line.find("@jsxImportSource").map(|col| (row + 1, col)))
        .unwrap_or((1, 0))
}

fn lexical_normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

async fn run(request: Request) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let start = Instant::now();
    let inputs = Arc::new(Mutex::new(BTreeMap::new()));
    let copy_outputs = Arc::new(Mutex::new(BTreeMap::new()));
    let plugin = GraphPlugin {
        inputs: inputs.clone(),
        file_extensions: request.file_extensions,
        copy_outputs: copy_outputs.clone(),
    };
    let outdir = request
        .options
        .file
        .as_ref()
        .map(|p| Path::new(p).parent().unwrap().to_path_buf())
        .or_else(|| request.options.dir.as_ref().map(PathBuf::from))
        .context("output location")?;
    let trailing_paths = trailing_paths::TrailingPaths::new(&request.options);
    let mut bundler = Bundler::with_plugins(
        request.options,
        vec![
            verbatim_specifiers::VerbatimSpecifiers::new_shared(
                verbatim_specifiers::VerbatimSpecifiers,
            ),
            trailing_paths::TrailingPaths::new_shared(trailing_paths),
            GraphPlugin::new_shared(plugin),
        ],
    )
    .map_err(|e| anyhow::anyhow!("Rolldown options: {e:?}"))?;
    let generated = bundler.generate().await;
    // This adapter's private plugins have no close hooks. Sequential
    // close also clears the pinned driver's retained state on returned errors;
    // a cleanup error must never replace the original generation diagnostic.
    let closed = bundler.close().await;
    let cwd = request.cwd.clone();
    let result = generated.map_err(|e| {
        let rendered: Vec<_> = e
            .into_vec()
            .iter()
            .map(|d| d.to_diagnostic().convert_to_string(false))
            .collect();
        match esbuild_style_unresolved(&rendered, &cwd) {
            Some(report) => anyhow::anyhow!("{report}"),
            None => anyhow::anyhow!("Rolldown native diagnostic: {}", rendered.join("\n")),
        }
    })?;
    closed.map_err(|e| anyhow::anyhow!("Rolldown close diagnostic: {e:?}"))?;
    let process_cwd = std::env::current_dir().unwrap_or_default();
    for warning in &result.warnings {
        if warning.kind().to_string() == "UNRESOLVED_IMPORT" {
            let rendered = [warning.to_diagnostic().convert_to_string(false)];
            // Esbuild leaves such an import() external and only warns (#4429);
            // the warning is still reported with the others below.
            if handled_imports::handled_unresolved(&rendered[0], &process_cwd) {
                continue;
            }
            match esbuild_style_unresolved(&rendered, &cwd) {
                Some(report) => bail!("{report}"),
                None => bail!("Rolldown refused unresolved import: {}", rendered[0]),
            }
        }
    }
    let bundle_us = start.elapsed().as_micros();
    let post = Instant::now();
    let mut outputs = BTreeMap::new();
    let mut files = Vec::new();
    let mut copied_sources = BTreeMap::new();
    let copy_outputs = copy_outputs.lock().unwrap().clone();
    let mut inputs = inputs.lock().unwrap().clone();
    let emitted_chunks: BTreeSet<String> = result
        .assets
        .iter()
        .filter_map(|output| match output {
            Output::Chunk(chunk) => Some(chunk.filename.to_string()),
            Output::Asset(_) => None,
        })
        .collect();
    for output in result.assets {
        let path = outdir.join(output.filename());
        files.push((path.clone(), output.content_as_bytes().to_vec()));
        match &output {
            Output::Chunk(chunk) => {
                let provenance: BTreeMap<_, _> = chunk
                    .module_ids
                    .iter()
                    .map(|id| (id.to_string(), json!({})))
                    .collect();
                let imports = chunk
                    .imports
                    .iter()
                    .map(|id| (id, "import-statement"))
                    .chain(
                        chunk
                            .dynamic_imports
                            .iter()
                            .map(|id| (id, "dynamic-import")),
                    )
                    .map(|(id, kind)| {
                        external_imports::import_entry(
                            id.as_str(),
                            kind,
                            &emitted_chunks,
                            &copy_outputs,
                            &outdir,
                        )
                    })
                    .collect::<Result<Vec<_>>>()?;
                outputs.insert(path.to_string_lossy().into_owned(),json!({"bytes":chunk.code.len(),"inputs":provenance,"imports":imports,"entryPoint":chunk.facade_module_id.as_ref().map(|id|id.as_str())}));
            }
            Output::Asset(asset) => {
                if !asset.filename.ends_with(".map") && asset.original_file_names.is_empty() {
                    bail!("asset {} has no source provenance", asset.filename);
                }
                copied_sources.insert(
                    asset.filename.to_string(),
                    asset
                        .original_file_names
                        .iter()
                        .map(|id| request.cwd.join(id).to_string_lossy().into_owned())
                        .collect::<Vec<_>>(),
                );
                let provenance: BTreeMap<_, _> = asset
                    .original_file_names
                    .iter()
                    .map(|id| {
                        let id = request.cwd.join(id).to_string_lossy().into_owned();
                        inputs
                            .entry(id.clone())
                            .or_insert_with(|| json!({"imports":[]}));
                        (id, json!({}))
                    })
                    .collect();
                outputs.insert(
                    path.to_string_lossy().into_owned(),
                    json!({"bytes":asset.source.as_bytes().len(),"inputs":provenance}),
                );
            }
        }
    }
    copy_reference::restore_sources(&mut inputs, &copied_sources)?;
    for filename in copy_outputs.values() {
        if copied_sources
            .get(filename)
            .is_none_or(|sources| sources.len() != 1)
        {
            bail!("copy output {filename} lacks unique emitted source provenance");
        }
    }
    // The existing audit/dependency schema keys staged routes relative to cwd.
    // Change only representation, preserving the engine's exact resolved edges.
    // Rolldown ids are canonical, so a symlinked cwd is matched physically too.
    let physical_cwd = std::fs::canonicalize(&request.cwd).unwrap_or_else(|_| request.cwd.clone());
    let key = |id: &str| {
        let id = Path::new(id);
        let base = if id.starts_with(&request.cwd) {
            &request.cwd
        } else {
            &physical_cwd
        };
        esbuild_relative(id, base)
    };
    for input in inputs.values_mut() {
        for import in input["imports"]
            .as_array_mut()
            .context("resolved imports")?
        {
            import["path"] = json!(key(import["path"]
                .as_str()
                .context("resolved import id")?));
        }
    }
    let inputs: BTreeMap<_, _> = inputs
        .into_iter()
        .map(|(id, info)| (key(&id), info))
        .collect();
    for output in outputs.values_mut() {
        let provenance = output["inputs"].as_object().context("output inputs")?;
        output["inputs"] = json!(provenance
            .iter()
            .map(|(id, info)| (key(id), info.clone()))
            .collect::<BTreeMap<_, _>>());
        if let Some(entry) = output.get("entryPoint").and_then(Value::as_str) {
            output["entryPoint"] = json!(key(entry));
        }
    }
    copy_reference::validate_metadata(&inputs, &outputs)?;
    if inputs.is_empty() {
        bail!("Rolldown returned no resolved input graph");
    }
    let warnings: Vec<_> = result
        .warnings
        .iter()
        // The prepared argv owns the transform options, so a tsconfig field it
        // overrides is the esbuild precedence, which esbuild applies silently.
        // Islands argv passes --jsx-import-source; the SSR pass gets the same
        // value promoted from its tsconfig (#4407). Rolldown reports these
        // outside `checks`, so they cannot be turned off there (#4404).
        .filter(|w| w.kind().to_string() != "CONFIGURATION_FIELD_CONFLICT")
        .map(|w| w.to_diagnostic().convert_to_string(false))
        .collect();
    for warning in &warnings {
        eprintln!("Rolldown warning: {warning}");
    }
    if let Some(path) = request.metafile {
        files.push((
            path,
            serde_json::to_vec_pretty(
                &json!({"inputs":inputs,"outputs":outputs,"zfbBackend":"rolldown-24bc2d0","warnings":warnings}),
            )?,
        ));
    }
    if std::env::var_os("ZFB_DEV_TIMING").is_some() {
        eprintln!(
            "[zfb-timing] rolldown-prototype: bundle={bundle_us}us metadata={}us",
            post.elapsed().as_micros()
        );
    }
    Ok(files)
}

/// Spells `path` the way esbuild's metafile does: relative to `cwd`, with `..`
/// segments for files outside it. Paths that share no root with `cwd` (another
/// Windows drive, a virtual id) stay as given.
fn esbuild_relative(path: &Path, cwd: &Path) -> String {
    use std::path::Component;
    if !path.is_absolute() {
        return path.to_string_lossy().into_owned();
    }
    let ours: Vec<_> = path.components().collect();
    let base: Vec<_> = cwd.components().collect();
    let shared = ours.iter().zip(&base).take_while(|(a, b)| a == b).count();
    let rooted = |c: &Component| matches!(c, Component::Prefix(_) | Component::RootDir);
    if !ours[..shared].iter().any(rooted) {
        return path.to_string_lossy().into_owned();
    }
    let mut parts: Vec<String> = vec!["..".to_string(); base.len() - shared];
    parts.extend(
        ours[shared..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}
