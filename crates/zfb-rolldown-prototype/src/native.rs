mod copy_reference;
mod prepared;
use anyhow::{bail, Context, Result};
use prepared::Request;
use rolldown::plugin::{
    HookBuildEndArgs, HookLoadArgs, HookLoadOutput, HookResolveFileUrlArgs, HookUsage, Plugin,
    PluginContext, SharedLoadPluginContext,
};
use rolldown::Bundler;
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
    let copy_outputs = copy_outputs.lock().unwrap().clone();
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
                        let path = copy_reference::output_name(id.as_str(), &copy_outputs)?;
                        Ok(json!({"path":outdir.join(path),"kind":kind}))
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
    copy_reference::validate_metadata(&inputs, &outputs)?;
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
