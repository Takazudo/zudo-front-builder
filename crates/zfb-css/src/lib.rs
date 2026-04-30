//! `zfb-css` — the CSS half of the zudo-front-builder (zfb) build pipeline.
//!
//! Responsibilities:
//!
//! 1. Run a "CSS engine" that produces Tailwind utility CSS for the project.
//!    Today this is a subprocess wrapper around the official `tailwindcss` v4
//!    CLI binary (see [`engine::TailwindSubprocessEngine`]). The trait
//!    [`engine::CssEngine`] documents the swap-in story for a future
//!    Rust-native engine (placeholder lives in [`native_engine`]).
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
//! ## Tailwind v4 entry CSS contract
//!
//! [`engine::TailwindSubprocessEngine`] generates a synthesised entry CSS
//! every time it spawns the Tailwind binary. The synthesised file is the
//! single source of truth for `@source` directives + the user's authored
//! global stylesheet + an optional inline `@theme` block. See
//! [`engine::build_synthesised_entry_css`] for the exact ordering rules.
//! Cross-package content globs (e.g. `packages/zudo-doc-v2/**`) live in
//! [`engine::TailwindSubprocessConfig::framework_package_globs`] —
//! framework classes survive Tailwind's tree-shake because they show up
//! in the `@source` set.
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

pub mod emitter;
pub mod engine;
pub mod modules;
pub mod native_engine;
pub mod pipeline;
pub mod scanner;

pub use emitter::{css_relative_path, CssEmitterOutput, CssProductionEmitter};
pub use engine::{
    build_synthesised_entry_css, CssEngine, TailwindSubprocessConfig,
    TailwindSubprocessEngine,
};
pub use modules::{CssModulesOutput, CssModulesProcessor};
pub use native_engine::NativeRustEngine;
pub use pipeline::{link_href, CssPipeline, CssPipelineConfig, CssPipelineOutput};
pub use scanner::{
    scan_css_module_imports, scan_css_module_imports_in_memory, ModuleImportScan,
    SourceModuleUsage,
};
