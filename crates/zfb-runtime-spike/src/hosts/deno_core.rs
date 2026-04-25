//! `deno_core` host — V8 via Deno's reusable core.
//!
//! Plan's recommended runtime. The implementation here is intentionally small
//! and sticks to documented Deno APIs so it's easy to port into `zfb-render`.
//!
//! Build cost note: enabling the `deno` feature triggers a 15-30 minute first
//! compile because `deno_core` pulls a full V8 source bundle. That's one of
//! the trade-offs ADR-001 explicitly weighs.

use crate::host::{RenderHost, RenderInput, RenderOutput};
use anyhow::{anyhow, Context as _, Result};
use deno_core::{
    serde_v8, v8, JsRuntime, ModuleCodeString, ModuleSpecifier, RuntimeOptions,
};

pub struct DenoCoreHost {
    runtime: tokio::runtime::Runtime,
}

impl DenoCoreHost {
    pub fn new() -> Result<Self> {
        // deno_core mandates a Tokio current-thread reactor. We construct one
        // upfront so each `render_module` call is just a `block_on`.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("tokio Runtime::new")?;
        Ok(Self { runtime })
    }
}

impl RenderHost for DenoCoreHost {
    fn name(&self) -> &'static str {
        "deno_core"
    }

    fn render_module(&mut self, input: RenderInput<'_>) -> Result<RenderOutput> {
        let name = input.name.to_string();
        let source = input.source.to_string();
        let html = self.runtime.block_on(async move {
            let mut js_runtime = JsRuntime::new(RuntimeOptions::default());
            let specifier = ModuleSpecifier::parse(&format!("file:///{}", name))
                .map_err(|e| anyhow!("module specifier parse: {e}"))?;
            let mod_id = js_runtime
                .load_main_es_module_from_code(&specifier, ModuleCodeString::from(source))
                .await
                .map_err(|e| anyhow!("deno_core load_main_es_module: {e}"))?;
            let result_rx = js_runtime.mod_evaluate(mod_id);
            js_runtime
                .run_event_loop(Default::default())
                .await
                .map_err(|e| anyhow!("deno_core run_event_loop: {e}"))?;
            result_rx
                .await
                .map_err(|e| anyhow!("deno_core mod_evaluate: {e}"))?;

            let module_namespace = js_runtime.get_module_namespace(mod_id)?;
            let scope = &mut js_runtime.handle_scope();
            let module_namespace = v8::Local::<v8::Object>::new(scope, module_namespace);
            let default_key = v8::String::new(scope, "default")
                .ok_or_else(|| anyhow!("v8 string alloc"))?;
            let default_fn = module_namespace
                .get(scope, default_key.into())
                .ok_or_else(|| anyhow!("module has no `default` export"))?;
            let default_fn: v8::Local<v8::Function> = default_fn
                .try_into()
                .map_err(|_| anyhow!("`default` export is not a function"))?;
            let undefined = v8::undefined(scope).into();
            let result = default_fn
                .call(scope, undefined, &[])
                .ok_or_else(|| anyhow!("default() returned no value"))?;
            let html: String = serde_v8::from_v8(scope, result)
                .map_err(|e| anyhow!("serde_v8 from_v8: {e}"))?;
            anyhow::Ok(html)
        })?;
        Ok(RenderOutput { html })
    }
}
