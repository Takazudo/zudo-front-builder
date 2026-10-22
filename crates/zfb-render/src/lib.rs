//! zfb-render: TSX → JS compile pipeline (SWC) and JS runtime host.
//!
//! Module slots:
//! - [`render_host`] — `RenderHost` trait (abstraction seam; the
//!   production host is an embedded V8 host wired in by the build orchestrator).
//! - [`swc_pipeline`] — SWC parse + transform (TS strip + JSX) into ES module JS.
//! - [`paths`] — `paths()` runtime resolution.
//! - [`paths_extract`] — static `paths()` literal extractor; the
//!   build-time fast path that pairs with [`paths::resolve_paths`] when
//!   the page's `paths()` return value is statically analyzable.
//! - [`dispatch_mode`] — `DispatchMode` (build-time SSG vs request-time
//!   SSR), carried per dispatch. Deliberately NOT behind `direct-v8`:
//!   `zfb_build::renderer::EmbeddedV8Host` names it unconditionally.
//! - [`error`] — crate-wide `RenderError`.

pub mod dispatch_mode;
pub mod error;
pub mod paths;
pub mod paths_extract;
pub mod render_host;
pub mod sourcemap;
pub mod swc_pipeline;

#[cfg(feature = "direct-v8")]
pub mod config_eval;

#[cfg(feature = "direct-v8")]
pub use config_eval::{ConfigEvalError, ThreadedConfigEvaluator};

pub use dispatch_mode::DispatchMode;
pub use error::{RenderError, Result};
pub use render_host::{ModuleHandle, RenderHost};
pub use swc_pipeline::{CompileOptions, CompiledModule, SwcPipeline};

#[cfg(feature = "direct-v8")]
pub use zfb_js_host::{
    dispatch::{HttpRequestLike, HttpResponseLike},
    modules::{AliasHook, BundleModuleLoader, PluginRegistryHooks, VirtualModuleHook},
    native::NativeHost as EmbeddedV8RenderHost,
};

/// Call on the process main thread before starting any host threads.
#[cfg(feature = "direct-v8")]
pub use zfb_js_host::process::initialize as initialize_v8;
