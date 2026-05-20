//! zfb-render: TSX → JS compile pipeline (SWC), JS runtime host, and page
//! render orchestrator.
//!
//! Module slots:
//! - [`render_host`] — `RenderHost` trait (abstraction seam; the
//!   production host is an embedded V8 host wired in by the build orchestrator).
//! - [`embedded_v8`] — in-process V8 host (`embed_v8` cargo feature, default-on).
//! - [`swc_pipeline`] — SWC parse + transform (TS strip + JSX) into ES module JS.
//! - [`loader`] — module resolver (compiles + caches imported modules).
//! - [`render`] — `Renderer` orchestrator: compile → load → execute → render.
//! - [`adapters`] — preact / react JSX runtime adapters.
//! - [`paths`] — `paths()` runtime resolution.
//! - [`paths_extract`] — static `paths()` literal extractor; the
//!   build-time fast path that pairs with [`paths::resolve_paths`] when
//!   the page's `paths()` return value is statically analyzable.
//! - [`meta`] — `meta` export extraction.
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

#[cfg(feature = "embed_v8")]
pub mod embedded_v8;

#[cfg(feature = "embed_v8")]
pub use embedded_v8::{
    AliasHook, BundleModuleLoader, EmbeddedV8RenderHost, HttpRequestLike, HttpResponseLike,
    PluginRegistryHooks, VirtualModuleHook,
};

pub use error::{RenderError, Result};
pub use loader::{read_to_string, ResolverError};
pub use render::{RenderRequest, Renderer};
pub use render_host::{ModuleHandle, RenderHost};
pub use swc_pipeline::{CompileOptions, CompiledModule, JsxRuntime, SwcPipeline};
