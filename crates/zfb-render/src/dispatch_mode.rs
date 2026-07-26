//! Per-dispatch mode signal for the embedded V8 host (issue #2014,
//! epic #2012).
//!
//! `crates/zfb/src/ssr_adapter.rs` drives request-time SSR through the
//! *same* `EmbeddedV8RenderHost` instance the dev tick uses to
//! prerender SSG pages, so the runtime cannot tell build-time render
//! from request-time render. A host-level flag would either deny
//! legitimate SSR or open build-time network access; the mode is
//! therefore carried **per dispatch**.
//!
//! Contract: `research/2013-request-time-capability-contract.md`,
//! "Mode distinction".
//!
//! ## Why this type lives here and not in `embedded_v8::dispatch`
//!
//! The contract names `embedded_v8/dispatch.rs` as the home of the
//! `HttpRequestLike::mode` field, and it is (re-exported from there
//! too, so `zfb_render::embedded_v8::dispatch::DispatchMode` resolves).
//! The enum itself has to sit in an **ungated** module because
//! `zfb_build::renderer::EmbeddedV8Host` — which grows the mode-bearing
//! entry point — is compiled with or without the `embed_v8` feature,
//! while `embedded_v8` exists only when the feature is on. Defining it
//! under the gate would break `cargo check --no-default-features -p zfb
//! --tests`.

/// Whether a dispatch is a build-time SSG render or a request-time SSR
/// render.
///
/// **Fail-safe default is [`DispatchMode::BuildTime`]** at every layer —
/// this Rust `Default`, [`super::HttpRequestLike::get`], and the JS
/// reader in `web_polyfills.js` when `__zfb.mode` is absent. Any call
/// site that is not explicitly updated therefore keeps the existing SSG
/// denials rather than silently gaining request-time capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DispatchMode {
    /// Build-time SSG render (`zfb build`, the dev tick's prerender
    /// pass, `paths()` evaluation, config evaluation, the dev
    /// content-trace endpoint). Network access stays denied.
    #[default]
    BuildTime,
    /// Request-time SSR render — a `prerender = false` route served
    /// per request through `crates/zfb/src/ssr_adapter.rs`. The only
    /// production call site that passes this.
    RequestTime,
}

impl DispatchMode {
    /// The wire spelling handed to `__zfb.dispatch(...)` and read back
    /// as `__zfb.mode` in `js/web_polyfills.js`. Kept as an explicit
    /// string (not a bool or an integer) so a stray value in the JS
    /// reader is inert rather than accidentally truthy.
    pub fn as_js_str(self) -> &'static str {
        match self {
            DispatchMode::BuildTime => "build-time",
            DispatchMode::RequestTime => "request-time",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_build_time_so_unupdated_call_sites_stay_denied() {
        assert_eq!(DispatchMode::default(), DispatchMode::BuildTime);
    }

    #[test]
    fn js_spellings_are_the_ones_web_polyfills_compares_against() {
        assert_eq!(DispatchMode::BuildTime.as_js_str(), "build-time");
        assert_eq!(DispatchMode::RequestTime.as_js_str(), "request-time");
    }
}
