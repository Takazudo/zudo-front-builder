//! Temporary phase-1 evidence runner; not a CLI/backend selector.
//!
//! Feed one JSON request per stdin line. Each request gets a fresh isolate.
//! Receipts are prefixed with `ZFB_HOST_RECEIPT ` because the unchanged
//! baseline console also writes to stdout. Worker source bytes are untouched.
//! This example never selects another executor when `embed_v8` is unavailable.

#[cfg(not(feature = "embed_v8"))]
fn main() {
    eprintln!("direct_v8_baseline requires the embed_v8 feature");
    std::process::exit(2);
}

#[cfg(feature = "embed_v8")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    baseline::run()
}

#[cfg(feature = "embed_v8")]
mod baseline {
    use std::collections::BTreeMap;
    use std::io::{self, BufRead, Write};
    use std::path::PathBuf;

    use serde::Deserialize;
    use serde_json::{json, Value};
    use zfb_render::{
        BundleModuleLoader, DispatchMode, EmbeddedV8RenderHost, HttpRequestLike,
        PluginRegistryHooks, RenderHost,
    };

    const ENGINE: &str = "deno_core 0.399.0 / v8 147.4.0";
    const PROBE_URL: &str = "file:///zfb/baseline-probe.mjs";
    const IDENTITY_URL: &str = "file:///zfb/identity.mjs";
    const IDENTITY_SOURCE: &str = r#"
        globalThis.__baselineEvaluations = (globalThis.__baselineEvaluations || 0) + 1;
        export const marker = 42;
        export const meta = { url: import.meta.url, main: import.meta.main ?? null };
    "#;
    const PROBE_SOURCE: &str = r#"
        import * as staticNamespace from './identity.mjs';
        const first = await import('./identity.mjs');
        const second = await import('./identity.mjs');
        export const probe = {
            url: import.meta.url,
            main: import.meta.main ?? null,
            resolveType: typeof import.meta.resolve,
            resolved: typeof import.meta.resolve === 'function'
                ? import.meta.resolve('./identity.mjs') : null,
            metaKeys: Object.keys(import.meta).sort(),
            dynamicSameNamespace: first === second && first === staticNamespace,
            evaluations: globalThis.__baselineEvaluations,
            marker: first.marker,
            dependencyMeta: first.meta,
        };
    "#;

    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "kebab-case")]
    enum Request {
        Config { source: String },
        ModuleProbe { source: Option<String> },
        Script { source: String, mode: Mode },
        Worker(Box<WorkerInput>),
    }

    #[derive(Deserialize)]
    struct ModuleInput {
        specifier: String,
        source: String,
    }

    #[derive(Deserialize)]
    struct NamedSource {
        name: String,
        source: String,
    }

    #[derive(Deserialize)]
    struct AliasInput {
        specifier: String,
        target: PathBuf,
        plugin: String,
    }

    #[derive(Deserialize)]
    struct VirtualInput {
        specifier: String,
        source: String,
        plugin: String,
    }

    #[derive(Default, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum Mode {
        #[default]
        BuildTime,
        RequestTime,
    }

    #[derive(Deserialize)]
    struct DispatchInput {
        url: String,
        #[serde(default = "get_method")]
        method: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        body: Option<Vec<u8>>,
        #[serde(default)]
        mode: Mode,
    }

    fn get_method() -> String {
        "GET".to_owned()
    }

    #[derive(Deserialize)]
    struct WorkerInput {
        name: String,
        source: String,
        #[serde(default)]
        modules: Vec<ModuleInput>,
        #[serde(default)]
        aliases: Vec<AliasInput>,
        #[serde(default)]
        virtual_modules: Vec<VirtualInput>,
        asset_root: Option<PathBuf>,
        inner: Option<NamedSource>,
        #[serde(default)]
        requests: Vec<DispatchInput>,
    }

    fn success(value: Value) -> Value {
        json!({ "engine": ENGINE, "ok": true, "value": value })
    }

    fn failure(error: impl std::fmt::Display) -> Value {
        json!({ "engine": ENGINE, "ok": false, "error": error.to_string() })
    }

    fn host_failure(host: &mut EmbeddedV8RenderHost, error: impl std::fmt::Display) -> Value {
        let mut receipt = failure(error);
        receipt["console"] = json!(host.drain_console_logs());
        receipt
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        for line in io::stdin().lock().lines() {
            let line = line?;
            let receipt = match serde_json::from_str::<Request>(&line) {
                Ok(request) => runtime.block_on(execute(request)),
                Err(error) => failure(format!("invalid runner request: {error}")),
            };
            // Lock only while writing a receipt: Deno's console is allowed
            // to write during execution and must not deadlock on this lock.
            let mut stdout = io::stdout().lock();
            writeln!(
                stdout,
                "ZFB_HOST_RECEIPT {}",
                serde_json::to_string(&receipt)?
            )?;
            stdout.flush()?;
        }
        Ok(())
    }

    async fn execute(request: Request) -> Value {
        match request {
            Request::Config { source } => {
                match zfb_render::config_eval::evaluate_config_bundle(&source).await {
                    Ok(value) => success(value),
                    Err(error) => failure(error),
                }
            }
            Request::ModuleProbe { source } => module_probe(source.as_deref()).await,
            Request::Script { source, mode } => script(&source, mode).await,
            Request::Worker(input) => worker(*input).await,
        }
    }

    async fn script(source: &str, mode: Mode) -> Value {
        let mut host = match EmbeddedV8RenderHost::new() {
            Ok(host) => host,
            Err(error) => return failure(error),
        };
        // Keep this wrapper byte-identical to the direct-V8 evidence runner.
        let worker =
            format!("export default {{async fetch(){{return Response.json(await ({source}));}}}}");
        if let Err(error) = host
            .execute_worker_module("file:///zfb/service-probe.mjs", &worker)
            .await
        {
            return host_failure(&mut host, error);
        }
        let request = HttpRequestLike {
            url: "http://zfb.local/service-probe".to_owned(),
            method: "GET".to_owned(),
            headers: BTreeMap::new(),
            body: None,
            mode: match mode {
                Mode::BuildTime => DispatchMode::BuildTime,
                Mode::RequestTime => DispatchMode::RequestTime,
            },
        };
        match host.dispatch_fetch(request).await {
            Ok(response) => match serde_json::from_slice::<Value>(&response.body) {
                Ok(value) => {
                    let mut receipt = success(value);
                    receipt["console"] = json!(host.drain_console_logs());
                    receipt
                }
                Err(error) => host_failure(&mut host, error),
            },
            Err(error) => host_failure(&mut host, error),
        }
    }

    async fn module_probe(source: Option<&str>) -> Value {
        let loader = BundleModuleLoader::new();
        loader.register_module(IDENTITY_URL, IDENTITY_SOURCE);
        let mut host = match EmbeddedV8RenderHost::with_loader(loader) {
            Ok(host) => host,
            Err(error) => return failure(error),
        };
        let handle = match host
            .execute_module(PROBE_URL, source.unwrap_or(PROBE_SOURCE))
            .await
        {
            Ok(handle) => handle,
            Err(error) => return host_failure(&mut host, error),
        };
        match host.get_export(&handle, "probe").await {
            Ok(value) => {
                let mut receipt = success(value);
                receipt["console"] = json!(host.drain_console_logs());
                receipt
            }
            Err(error) => host_failure(&mut host, error),
        }
    }

    async fn worker(input: WorkerInput) -> Value {
        let mut hooks = PluginRegistryHooks::new();
        for alias in input.aliases {
            hooks.add_alias(alias.specifier, alias.target, alias.plugin);
        }
        for module in input.virtual_modules {
            hooks.add_virtual_module(module.specifier, module.source, module.plugin);
        }
        let mut loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
        if let Some(root) = input.asset_root {
            loader = loader.with_bundle_asset_root(root);
        }
        for module in input.modules {
            loader.register_module(&module.specifier, module.source);
        }
        let mut host = match EmbeddedV8RenderHost::with_loader(loader) {
            Ok(host) => host,
            Err(error) => return failure(error),
        };
        if let Some(inner) = input.inner {
            if let Err(error) = host
                .validate_worker_module_shape(&inner.name, &inner.source)
                .await
            {
                return host_failure(&mut host, error);
            }
        }
        if let Err(error) = host.execute_worker_module(&input.name, &input.source).await {
            return host_failure(&mut host, error);
        }
        let mut responses = Vec::with_capacity(input.requests.len());
        for request in input.requests {
            let request = HttpRequestLike {
                url: request.url,
                method: request.method,
                headers: request.headers,
                body: request.body,
                mode: match request.mode {
                    Mode::BuildTime => DispatchMode::BuildTime,
                    Mode::RequestTime => DispatchMode::RequestTime,
                },
            };
            match host.dispatch_fetch(request).await {
                Ok(response) => responses.push(json!({
                    "status": response.status,
                    "headers": response.headers,
                    "body": response.body,
                })),
                Err(error) => {
                    let mut receipt = host_failure(&mut host, error);
                    receipt["responses"] = json!(responses);
                    return receipt;
                }
            }
        }
        success(json!({ "responses": responses, "console": host.drain_console_logs() }))
    }
}
