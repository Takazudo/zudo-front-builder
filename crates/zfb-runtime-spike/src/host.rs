//! The `RenderHost` trait — the abstraction boundary the ADR locks in.
//!
//! Each candidate runtime (`deno_core`, `ssr_rs`, `rquickjs`) implements this
//! trait. `zfb-render` will depend only on this trait shape, never on the
//! concrete runtime, so swapping runtimes in v1.1+ is a localized change.

use anyhow::Result;

/// One unit of work the renderer can ask the runtime to perform: load an ESM
/// module by source string and call its `default` export to produce HTML.
///
/// `name` is used as the module specifier in error messages and source maps,
/// so the runtime must surface it verbatim when reporting throws/parses.
#[derive(Debug, Clone)]
pub struct RenderInput<'a> {
    pub name: &'a str,
    pub source: &'a str,
}

/// Result of one render. Held minimal on purpose — extending this struct is
/// cheap, narrowing it later is not.
#[derive(Debug, Clone)]
pub struct RenderOutput {
    pub html: String,
}

/// The contract every candidate runtime must satisfy.
///
/// Intentionally tiny. The spike is about evaluating the runtime, not its
/// surface API. `zfb-render` will widen this later (e.g. `paths()`, `meta`
/// resolution), but those additions stay behind this same trait.
// Send is intentionally not required here: deno_core's JsRuntime is bound to
// the thread that constructed it, and rquickjs Context similarly holds
// thread-local state. The renderer runs JS on the orchestration thread; if a
// future revision wants thread-pool dispatch, the host will be wrapped in a
// per-thread isolate pool rather than gaining a Send bound here.
pub trait RenderHost {
    /// Human-readable name used in metric reports and ADR tables.
    fn name(&self) -> &'static str;

    /// Load `input.source` as an ESM module under specifier `input.name`,
    /// invoke its `default` export with no args, and return the HTML string
    /// it produced.
    ///
    /// Implementations are expected to:
    /// - support ESM `import`/`export` syntax,
    /// - support top-level `await`,
    /// - surface throws with source-accurate locations referencing `input.name`.
    ///
    /// The spike's qualitative pass/fail axes test exactly these properties.
    fn render_module(&mut self, input: RenderInput<'_>) -> Result<RenderOutput>;
}
