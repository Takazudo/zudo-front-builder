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
//!    into scoped CSS plus a class-name map (see [`modules`]).
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

pub mod engine;
pub mod modules;
pub mod native_engine;
pub mod pipeline;

pub use engine::{CssEngine, TailwindSubprocessConfig, TailwindSubprocessEngine};
pub use modules::{CssModulesOutput, CssModulesProcessor};
pub use native_engine::NativeRustEngine;
pub use pipeline::{link_href, CssPipeline, CssPipelineConfig, CssPipelineOutput};
