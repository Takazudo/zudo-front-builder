//! `zfb-islands` — the islands runtime half of the zfb build pipeline.
//!
//! Responsibilities (Epic 6 / issue #6):
//!
//! 1. Scan project sources for components carrying the `"use client"`
//!    directive and produce a deterministic islands set
//!    (Sub 1 — owned by topic-ast-scanner; lands in the `scanner` module).
//!
//! 2. Bundle each island entry point into a single hashed JS asset
//!    (`dist/assets/islands-{hash}.js`) by shelling out to the esbuild CLI
//!    (this crate's [`bundler`] module — Sub 2). The implementation mirrors
//!    the `zfb-css` crate's `TailwindSubprocessEngine` pattern: a
//!    `ClientBundler` trait, a subprocess implementation, and a placeholder
//!    native-Rust stub for the future swap-in (see [`future_rust_native`]).
//!
//! 3. Emit hydration markup and runtime glue
//!    (Sub 3 — owned by topic-hydration-emit; lands in the `emit` module).
//!
//! ## Layering
//!
//! `zfb-islands` deliberately does **not** depend on `zfb-render`. It takes
//! source paths in and returns asset paths plus URLs out. The renderer is
//! responsible for actually injecting `<script type="module" src="...">`
//! into the page. The helper [`bundler::bundle_link_href`] derives the
//! public URL from the asset path so the renderer (and Sub 3's hydration
//! emitter) can do so without re-hashing.
//!
//! ## Status
//!
//! Sub 2 (this commit) lands the bundler module + its native-rust stub +
//! the binary distribution slot at `crates/zfb/binaries/esbuild`. Sub 1
//! lands the `Island` / `BundleConfig` / `BundleOutput` / `ModuleId` /
//! `ClientBundler` types and the scanner module — call sites should prefer
//! the re-exports from this crate root so that future refinement stays a
//! single-site change.

pub mod bundler;
pub mod future_rust_native;

pub use bundler::{
    bundle_link_href, BundleConfig, BundleOutput, ClientBundler, EsbuildSubprocessBundler,
    EsbuildSubprocessConfig, Island, ModuleId,
};
pub use future_rust_native::NativeRustBundler;
