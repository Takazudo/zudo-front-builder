use anyhow::{bail, Context, Result};
use rolldown::plugin::{
    HookBuildEndArgs, HookLoadArgs, HookLoadOutput, HookResolveFileUrlArgs, HookUsage, Plugin,
    PluginContext, SharedLoadPluginContext,
};
use rolldown::{Bundler, BundlerOptions};
use rolldown_common::{EmittedAsset, ModuleType, Output};
use serde_json::{json, Value};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::Instant,
};

pub fn enabled() -> bool {
    std::env::var_os("ZFB_ROLLDOWN_PROTOTYPE").as_deref() == Some(std::ffi::OsStr::new("1"))
}

/// Owned boundary: the already prepared job's policies, not a public backend API.
/// The argv decoder intentionally rejects options outside this prototype's scope.
struct Request {
    cwd: PathBuf,
    options: BundlerOptions,
    metafile: Option<PathBuf>,
    file_extensions: Vec<String>,
}

#[derive(Debug)]
struct GraphPlugin {
    inputs: Arc<Mutex<BTreeMap<String, Value>>>,
    file_extensions: Vec<String>,
}
impl Plugin for GraphPlugin {
    fn name(&self) -> Cow<'static, str> {
        "zfb-prototype-provenance".into()
    }
    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::BuildEnd | HookUsage::Load | HookUsage::ResolveFileUrl
    }
    async fn load(
        &self,
        ctx: SharedLoadPluginContext,
        args: &HookLoadArgs<'_>,
    ) -> Result<Option<HookLoadOutput>> {
        if !self
            .file_extensions
            .iter()
            .any(|ext| args.id.ends_with(ext))
        {
            return Ok(None);
        }
        let path = Path::new(args.id);
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
        for id in ctx.get_module_ids() {
            let Some(info) = ctx.get_module_info(&id) else {
                bail!("missing resolved module info: {id}")
            };
            let mut imports = Vec::new();
            for (ids, kind) in [
                (&info.imported_ids, "import-statement"),
                (&info.dynamically_imported_ids, "dynamic-import"),
            ] {
                for imported in ids {
                    if let Some(reference) = imported.strip_prefix("__ROLLDOWN_COPY_MODULE__#") {
                        imports.push(json!({"path": imported.as_str(), "kind":kind, "external":false, "zfbCopiedOutput":ctx.get_file_name(reference)?.as_str()}));
                    } else {
                        imports.push(json!({"path": imported.as_str(), "kind":kind, "external": ctx.get_module_info(imported).is_none()}));
                    }
                }
            }
            imports.sort_by_key(|v| v["path"].as_str().unwrap().to_owned());
            inputs.insert(
                id.to_string(),
                json!({"bytes":info.code.as_ref().map_or(0, |s|s.len()), "imports": imports}),
            );
        }
        Ok(())
    }
}

impl Request {
    fn decode(cmd: &Command) -> Result<Self> {
        let cwd = cmd
            .get_current_dir()
            .map(Path::to_path_buf)
            .unwrap_or(std::env::current_dir()?);
        let mut options = BundlerOptions {
            cwd: Some(cwd.clone()),
            code_splitting: Some(rolldown_common::CodeSplittingMode::Bool(false)),
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
                    .push((k.to_string(), vec![Some(v.to_string())]));
            } else if let Some(value) = arg.strip_prefix("--external:") {
                if value.strip_suffix('*').unwrap_or(value).contains('*') {
                    bail!("Rolldown prototype only supports suffix external wildcards: {value}");
                }
                external.push(value.to_string());
            } else if let Some(value) = arg.strip_prefix("--outfile=") {
                options.file = Some(cwd.join(value).to_string_lossy().into_owned());
            } else if let Some(value) = arg.strip_prefix("--outdir=") {
                options.dir = Some(cwd.join(value).to_string_lossy().into_owned());
            } else if let Some(value) = arg.strip_prefix("--entry-names=") {
                options.entry_filenames = Some(format!("{value}.js").into());
            } else if let Some(value) = arg.strip_prefix("--chunk-names=") {
                options.chunk_filenames = Some(format!("{value}.js").into());
            } else if let Some(value) = arg.strip_prefix("--asset-names=") {
                options.asset_filenames = Some(format!("{value}[extname]").into());
            } else if let Some(value) = arg.strip_prefix("--metafile=") {
                metafile = Some(cwd.join(value));
            } else if let Some(value) = arg.strip_prefix("--tsconfig=") {
                options.tsconfig = Some(rolldown_common::TsConfig::Manual(cwd.join(value)));
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
                        entry = Some(cwd.join(arg).to_string_lossy().into_owned())
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
                    modules
                        .extend(std::env::split_paths(v).map(|p| p.to_string_lossy().into_owned()));
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

/// Execute one selected prepared job in process. Runtime construction is isolated
/// on a scoped thread, so callers already running a Tokio/V8 host remain valid.
pub fn run_prepared(cmd: &Command) -> Result<()> {
    let request = Request::decode(cmd)?;
    std::thread::scope(|scope| {
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
    })
}
async fn run(request: Request) -> Result<()> {
    let start = Instant::now();
    let inputs = Arc::new(Mutex::new(BTreeMap::new()));
    let plugin = GraphPlugin {
        inputs: inputs.clone(),
        file_extensions: request.file_extensions,
    };
    let outdir = request
        .options
        .file
        .as_ref()
        .map(|p| Path::new(p).parent().unwrap().to_path_buf())
        .or_else(|| request.options.dir.as_ref().map(PathBuf::from))
        .context("output location")?;
    let mut bundler = Bundler::with_plugins(request.options, vec![GraphPlugin::new_shared(plugin)])
        .map_err(|e| anyhow::anyhow!("Rolldown options: {e:?}"))?;
    let result = bundler.write().await.map_err(|e| {
        anyhow::anyhow!(
            "Rolldown native diagnostic: {}",
            e.into_vec()
                .iter()
                .map(|d| d.to_diagnostic().convert_to_string(false))
                .collect::<Vec<_>>()
                .join("\n")
        )
    })?;
    for warning in &result.warnings {
        if warning.kind().to_string() == "UNRESOLVED_IMPORT" {
            bail!(
                "Rolldown refused unresolved import: {}",
                warning.to_diagnostic().convert_to_string(false)
            );
        }
    }
    let bundle_us = start.elapsed().as_micros();
    let post = Instant::now();
    let mut outputs = BTreeMap::new();
    let mut copied_sources = BTreeMap::new();
    let mut inputs = inputs.lock().unwrap().clone();
    for output in result.assets {
        let path = outdir.join(output.filename());
        match &output {
            Output::Chunk(chunk) => {
                let provenance: BTreeMap<_, _> = chunk
                    .module_ids
                    .iter()
                    .map(|id| (id.to_string(), json!({})))
                    .collect();
                let imports: Vec<_> = chunk
                    .imports
                    .iter()
                    .map(|id| json!({"path":outdir.join(id.as_str()),"kind":"import-statement"}))
                    .chain(
                        chunk.dynamic_imports.iter().map(
                            |id| json!({"path":outdir.join(id.as_str()),"kind":"dynamic-import"}),
                        ),
                    )
                    .collect();
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
    for input in inputs.values_mut() {
        for import in input["imports"]
            .as_array_mut()
            .context("resolved imports")?
        {
            if let Some(filename) = import
                .get("zfbCopiedOutput")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                let sources = copied_sources
                    .get(&filename)
                    .context("copy output lacks native provenance")?;
                if sources.len() != 1 {
                    bail!("copy output {filename} has ambiguous source provenance");
                }
                import["path"] = json!(sources[0]);
                import.as_object_mut().unwrap().remove("zfbCopiedOutput");
            }
        }
    }
    // The existing audit/dependency schema keys staged routes relative to cwd.
    // Change only representation, preserving the engine's exact resolved edges.
    let key = |id: &str| {
        Path::new(id).strip_prefix(&request.cwd).map_or_else(
            |_| id.to_string(),
            |p| p.to_string_lossy().replace('\\', "/"),
        )
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
    if inputs.is_empty() {
        bail!("Rolldown returned no resolved input graph");
    }
    let warnings: Vec<_> = result
        .warnings
        .iter()
        .map(|w| w.to_diagnostic().convert_to_string(false))
        .collect();
    for warning in &warnings {
        eprintln!("Rolldown warning: {warning}");
    }
    if let Some(path) = request.metafile {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(
                &json!({"inputs":inputs,"outputs":outputs,"zfbBackend":"rolldown-24bc2d0","warnings":warnings}),
            )?,
        )?;
    }
    if std::env::var_os("ZFB_DEV_TIMING").is_some() {
        eprintln!(
            "[zfb-timing] rolldown-prototype: bundle={bundle_us}us metadata={}us",
            post.elapsed().as_micros()
        );
    }
    Ok(())
}
