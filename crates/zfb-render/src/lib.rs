//! zfb-render: TSX → JS compile pipeline (SWC), JS runtime host, and page
//! render orchestrator.
//!
//! Module slots:
//! - [`render_host`] — `RenderHost` trait (abstraction seam; per ADR-005 the
//!   production host is a miniflare subprocess client wired in by the build
//!   orchestrator).
//! - [`swc_pipeline`] — SWC parse + transform (TS strip + JSX) into ES module JS.
//! - [`loader`] — module resolver (compiles + caches imported modules).
//! - [`render`] — `Renderer` orchestrator: compile → load → execute → render.
//! - [`adapters`] — preact / react JSX runtime adapters (Sub 4).
//! - [`paths`] — `paths()` runtime resolution (Sub 5).
//! - [`paths_extract`] — static `paths()` literal extractor; the
//!   build-time fast path that pairs with [`paths::resolve_paths`] when
//!   the page's `paths()` return value is statically analyzable.
//! - [`meta`] — `meta` export extraction (Sub 6).
//! - [`error`] — crate-wide `RenderError`.

pub mod adapters;
pub mod error;
pub mod loader;
pub mod meta;
pub mod paths;
pub mod paths_extract;
pub mod render;
pub mod render_host;
pub mod sourcemap;
pub mod swc_pipeline;

pub use error::{RenderError, Result};
pub use render::{RenderRequest, Renderer};
pub use render_host::{ModuleHandle, RenderHost};
pub use swc_pipeline::{CompileOptions, CompiledModule, JsxRuntime, SwcPipeline};
