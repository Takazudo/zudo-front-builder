//! `deno_core` host — V8 via Deno's reusable core.
//!
//! Plan's recommended runtime. The implementation here is intentionally small
//! and sticks to documented Deno APIs so it's easy to port into `zfb-render`.
//!
//! Build cost note: enabling the `deno` feature triggers a multi-minute first
//! compile because `deno_core` pulls a full V8 source bundle. That's one of
//! the trade-offs ADR-001 explicitly weighs.
//!
//! Bench shape: same as the rquickjs host — fixtures are scripts that define
//! `globalThis.renderHTML`, evaluated as plain JS, with the V8 isolate
//! reused across iterations so the warm-render loop is what we measure.

use crate::host::{RenderHost, RenderInput, RenderOutput};
use anyhow::{anyhow, Context as _, Result};
use deno_core::{serde_v8, v8, JsRuntime, RuntimeOptions};

pub struct DenoCoreHost {
    runtime: JsRuntime,
}

impl DenoCoreHost {
    pub fn new() -> Result<Self> {
        let runtime = JsRuntime::new(RuntimeOptions::default());
        Ok(Self { runtime })
    }
}

impl RenderHost for DenoCoreHost {
    fn name(&self) -> &'static str {
        "deno_core"
    }

    fn render_module(&mut self, input: RenderInput<'_>) -> Result<RenderOutput> {
        // Run the fixture as a top-level script. The fixture defines
        // `renderHTML` as a function declaration; we then call it. This
        // mirrors the pattern the rquickjs host uses, so the warm-render
        // loop measures the same shape across hosts.
        let script = format!(
            "(function() {{\n{src}\n; return renderHTML(); }})()",
            src = input.source
        );
        let scope = &mut self.runtime.handle_scope();
        let code = v8::String::new(scope, &script)
            .ok_or_else(|| anyhow!("v8 string alloc"))?;
        let resource_name = v8::String::new(scope, input.name)
            .ok_or_else(|| anyhow!("v8 resource_name alloc"))?;
        let origin = v8::ScriptOrigin::new(
            scope,
            resource_name.into(),
            0,
            0,
            false,
            0,
            None,
            false,
            false,
            false,
            None,
        );
        let mut try_catch = v8::TryCatch::new(scope);
        let compiled = v8::Script::compile(&mut try_catch, code, Some(&origin))
            .ok_or_else(|| {
                anyhow!(
                    "deno_core compile({}) failed: {}",
                    input.name,
                    extract_exception(&mut try_catch)
                )
            })?;
        let result = compiled.run(&mut try_catch).ok_or_else(|| {
            anyhow!(
                "deno_core run({}) failed: {}",
                input.name,
                extract_exception(&mut try_catch)
            )
        })?;
        if !result.is_string() {
            return Err(anyhow!(
                "deno_core renderHTML({}) returned non-string ({:?})",
                input.name,
                result.type_repr()
            ));
        }
        let html: String =
            serde_v8::from_v8(&mut try_catch, result).context("serde_v8 from_v8")?;
        Ok(RenderOutput { html })
    }
}

/// Pull the JS exception message out of a `TryCatch` so the host's error
/// chain shows the real failure instead of a generic "run failed". This is
/// what makes V8's debuggability advantage actually visible to the caller.
fn extract_exception(try_catch: &mut v8::TryCatch<v8::HandleScope>) -> String {
    if let Some(ex) = try_catch.exception() {
        if let Some(msg) = ex.to_string(try_catch) {
            return msg.to_rust_string_lossy(try_catch);
        }
    }
    "<no exception captured>".to_string()
}
