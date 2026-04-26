//! `rquickjs` host — non-V8 control candidate.
//!
//! QuickJS is a small, embeddable JS engine (Bellard). The Rust bindings
//! `rquickjs` give us a synchronous-friendly API. We use it as the
//! lightweight control case for the spike: it compiles in seconds, has no V8
//! dependency, and exposes the smallest plausible footprint.
//!
//! The spike's bench-js fixtures use a *script* shape rather than full ESM —
//! they define `globalThis.renderHTML` as the entry point — because ESM
//! evaluation in rquickjs returns a microtask-driven promise that needs the
//! `parallel`/`async` runtime feature to drive cleanly. The qualitative ESM
//! axis in ADR-001 captures that distinction; the bench measures pure
//! synchronous render throughput as a lower bound.

use crate::host::{RenderHost, RenderInput, RenderOutput};
use anyhow::{anyhow, Context as _, Result};
use rquickjs::{Context, Function, Runtime, Value};

pub struct QuickJsHost {
    _runtime: Runtime,
    context: Context,
}

impl QuickJsHost {
    pub fn new() -> Result<Self> {
        let runtime = Runtime::new().context("rquickjs Runtime::new")?;
        runtime.set_memory_limit(256 * 1024 * 1024);
        let context = Context::full(&runtime).context("rquickjs Context::full")?;
        Ok(Self { _runtime: runtime, context })
    }
}

impl RenderHost for QuickJsHost {
    fn name(&self) -> &'static str {
        "rquickjs"
    }

    fn render_module(&mut self, input: RenderInput<'_>) -> Result<RenderOutput> {
        let source = input.source.to_string();
        let name = input.name.to_string();
        let html: String = self.context.with(|ctx| -> Result<String> {
            // We wrap the fixture as an IIFE so each call evaluates in its
            // own lexical scope. The fixture defines a top-level
            // `renderHTML()` function which the IIFE returns.
            let wrapped = format!(
                "(function() {{\n{source}\n; return typeof renderHTML === 'function' ? renderHTML : null; }})()"
            );
            // Use the typed value first so we can distinguish a missing
            // `renderHTML` (returns null) from a real eval error. Without
            // this guard, a fixture that forgot to define renderHTML would
            // surface as a confusing "value is not a function" panic
            // downstream.
            let value: Value = ctx
                .eval::<Value, _>(wrapped)
                .map_err(|e| anyhow!("rquickjs eval({name}): {e}"))?;
            let func = Function::from_value(value).map_err(|_| {
                anyhow!(
                    "rquickjs fixture {name}: did not define a top-level `renderHTML` function"
                )
            })?;
            let html: String = func
                .call(())
                .map_err(|e| anyhow!("rquickjs renderHTML({name}): {e}"))?;
            Ok(html)
        })?;
        Ok(RenderOutput { html })
    }
}
