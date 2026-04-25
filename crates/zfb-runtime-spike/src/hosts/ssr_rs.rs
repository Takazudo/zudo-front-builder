//! `ssr_rs` host — purpose-built thin V8 wrapper for SSR.
//!
//! `ssr_rs` is narrower than `deno_core`: it expects a single bundled JS file
//! that exposes named entry functions, and it focuses on the React/Preact
//! `renderToString` use case. We use it to test whether that narrower model is
//! a better fit than the more general `deno_core` runtime.
//!
//! Build cost note: same V8 dependency as `deno_core`, so the first build
//! takes 15-30 minutes.

use crate::host::{RenderHost, RenderInput, RenderOutput};
use anyhow::{anyhow, Result};
use ssr_rs::Ssr;

pub struct SsrRsHost {
    /// Held to keep the V8 platform initialized for the host's lifetime.
    _platform_guard: PlatformGuard,
}

struct PlatformGuard;

impl PlatformGuard {
    fn new() -> Self {
        Ssr::create_platform();
        PlatformGuard
    }
}

impl SsrRsHost {
    pub fn new() -> Result<Self> {
        Ok(Self {
            _platform_guard: PlatformGuard::new(),
        })
    }
}

impl RenderHost for SsrRsHost {
    fn name(&self) -> &'static str {
        "ssr_rs"
    }

    fn render_module(&mut self, input: RenderInput<'_>) -> Result<RenderOutput> {
        // ssr_rs expects: an IIFE bundle exposing one or more named entry
        // functions on `globalThis`. We adapt by wrapping the source so the
        // module's `default` export is exposed as `entry`, then asking ssr_rs
        // to render that.
        let wrapped = format!(
            "var __zfb_exports = {{}};\n(function(exports){{\n{src}\n}})(__zfb_exports);\n\
             function entry(_props){{ return __zfb_exports.default(_props); }}",
            src = input.source
        );
        let mut ssr = Ssr::from(wrapped, "")
            .map_err(|e| anyhow!("ssr_rs Ssr::from({}): {e:?}", input.name))?;
        let html = ssr
            .render_to_string(None)
            .map_err(|e| anyhow!("ssr_rs render_to_string: {e:?}"))?;
        Ok(RenderOutput { html })
    }
}
