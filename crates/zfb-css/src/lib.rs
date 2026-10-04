//! `zfb-css` — the CSS half of the zudo-front-builder (zfb) build pipeline.
//!
//! Responsibilities:
//!
//! 1. Compile utility CSS with zudo-wind and authored CSS through the shared pipeline.
//!
//! 2. Compile `*.module.css` files via `lightningcss`'s CSS Modules support
//!    into scoped CSS plus a class-name map (see [`modules`]). Discovery
//!    of `*.module.css` imports inside TSX/JSX/TS/JS source files lives in
//!    [`scanner`].
//!
//! 3. Concatenate the engine output and the CSS Modules output, hash the
//!    bytes (SHA-256, truncated to 8 hex chars), and emit
//!    `dist/assets/styles-{hash}.css` (see [`pipeline`]).
//!
//! The top-level entry point is [`CssPipeline`].
//!
//! ## Layering
//!
//! `zfb-css` deliberately does **not** depend on Epic 3 (`zfb-render`). It
//! takes paths and source content as input and returns CSS + an asset URL as
//! output. The renderer is responsible for actually injecting
//! `<link href="...">` into the page. The helper [`pipeline::link_href`] is
//! provided so the renderer can derive the public URL from the asset path
//! without re-hashing.
//!
//! ## CSS Modules JS-side rewrite contract
//!
//! When [`pipeline::CssPipelineConfig::class_map_dir`] is set,
//! [`pipeline::CssPipeline::build`] writes one
//! `<sha8>__<basename>.classes.json` file per processed `.module.css`
//! into that directory. Each file is a flat JSON object mapping
//! original-class → scoped-class:
//!
//! ```json
//! { "btn": "abc12345_btn", "btn-primary": "abc12345_btn-primary" }
//! ```
//!
//! The bundler stage (esbuild plugin in `zfb-bundler`) is responsible
//! for intercepting `import styles from "./foo.module.css"` and
//! replacing it with a virtual ESM module that re-exports the JSON map
//! as the default export:
//!
//! ```text
//! const styles = <inline-or-fetched JSON>;
//! export default styles;
//! ```
//!
//! The contract is intentionally a *map* (not a live `Proxy`) so that
//! tree-shaking + minification work as expected and so SSR can render
//! the exact same class names the bundle ships.
//!
//! `zfb-css` MUST NOT do the JS rewrite itself: that's the bundler's
//! job, and it needs to happen at the same point as the rest of the
//! `import` rewrites (e.g. islands resolution) to avoid two passes
//! over the same module graph.
//!
//! In addition to the JSON files, the pipeline returns the same maps
//! in-memory via [`pipeline::CssPipelineOutput::class_maps`] so a bundler
//! that prefers to inline the maps can do so without touching the disk
//! artefacts.

pub mod authored_engine;
pub mod css_imports;
pub mod emitter;
pub mod engine;
pub mod engine_output;
pub mod leftover_directives;
pub mod modules;
pub mod pipeline;
pub mod scanner;
pub mod stub_engine;
pub mod url_attribution;
pub mod url_scanner;
pub mod wind_engine;

pub use authored_engine::AuthoredCssEngine;
pub use css_imports::{
    bundle_authored_css, bundle_authored_css_with_assets, resolve_css_imports, AuthoredCssBundle,
};
pub use emitter::{css_relative_path, CssEmitterOutput, CssProductionEmitter};
pub use engine::CssEngine;
pub use engine_output::{
    dedup_diagnostics, CssDiagnostic, CssDiagnosticOrigin, CssDiagnosticSeverity, CssEngineId,
    CssEngineOutput, CssInputDependency, CssInputDependencyKind, CssProvenance, CssProvenanceKind,
    WindDiagnosticsError,
};
pub use leftover_directives::{
    check_forbidden_directives, scan_leftover_directives, LeftoverDirective,
    FORBIDDEN_WIND_DIRECTIVES,
};
pub use modules::{CssModulesOutput, CssModulesProcessor};
pub use pipeline::{link_href, CssPipeline, CssPipelineConfig, CssPipelineOutput};
pub use scanner::{
    scan_css_module_imports, scan_css_module_imports_in_memory, ModuleImportScan, SourceModuleUsage,
};
pub use stub_engine::StubCssEngine;
pub use url_attribution::{AttributedUrl, PackageOrigin, PackageUrlAsset, UrlOrigin};
pub use url_scanner::{scan_css_urls, CssUrlOccurrence, UrlQuote};
pub use wind_engine::WindEngine;
pub use zudo_wind::{
    audit, audit_json, compile_exclusion_pattern, expand_changed_path, expand_file_set, explain,
    explain_disabled, explain_with_generation, extract_candidates, render_audit,
    render_explanation, AuditConflict, AuditInput, AuditNote, AuditOutcome, AuditReport,
    AuditSource, BreakpointConfig, CandidateIndex, DarkModeConfig, DeadClass, DeclarationView,
    DiagnosticView, DynamicConstruction, ExclusionMatcher, ExpandedFile, Explanation,
    ExplanationOutcome, ExtractedCandidate, ExtractionNote, ExtractionResult, FileSet,
    FontSizeToken, InterpolatedCandidate, NoteKind, Occurrence, Origin, OriginCandidate,
    OriginView, ParsedCandidate, PositionKind, PositiveRoot, ResetMode, SortTuple, SourceExclusion,
    SourceId, SourceKind, SourcePlan, SourcePositionKind, TokenResolution, UnrecognizedClass,
    UtilityPlacement, WalkDiagnostic, WindConfig, PACKAGE_ROOT_TRAVERSES,
};

/// The framework-shipped default `--zfb-hi-*` token stylesheet for zfb's
/// class-mode syntax highlighting (see `assets/zfb-hi.css`).
///
/// This crate only owns the artifact + this accessor. Injecting the
/// stylesheet into the combined `styles.css` (dev and prod) is a separate
/// concern wired up by the CSS pipeline's stylesheet-injection sub (issue
/// #1533).
pub fn default_hi_css() -> &'static str {
    include_str!("../assets/zfb-hi.css")
}
