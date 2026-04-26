//! `zfb-islands` — the islands runtime half of the zudo-front-builder build
//! pipeline.
//!
//! Responsibilities (Epic 6 / issue #6):
//!
//! 1. Scan project sources for components carrying the `"use client"`
//!    directive and produce a deterministic, sorted islands set keyed by
//!    stable component-name identity (file path + exported name). See
//!    [`scanner`] (Sub 1).
//!
//! 2. Define the public [`ClientBundler`] trait — the contract the islands
//!    bundler implements — and provide the production
//!    `EsbuildSubprocessBundler` (Sub 2) that wraps the esbuild CLI
//!    subprocess. The trait documents the swap-in story for a future
//!    Rust-native bundler (placeholder lives in [`future_rust_native`],
//!    analog of `zfb_css::native_engine`).
//!
//! 3. Emit hydration markup and the runtime glue script (Sub 3 — landed in
//!    the `hydration` module by topic-hydration-emit).
//!
//! ## Layering
//!
//! Like `zfb-css`, this crate deliberately does **not** depend on
//! `zfb-render`. It takes paths and source content as input (via a
//! [`scanner::Resolver`] the caller supplies) and returns plain Rust types
//! plus asset paths. Wiring into the full render orchestrator happens in
//! Epic 7.

pub mod bundler;
pub mod esbuild;
pub mod future_rust_native;
pub mod scanner;

pub use bundler::{
    bundle_link_href, BundleConfig, BundleOutput, ClientBundler, Island, ModuleId,
};
pub use esbuild::{hash_8, EsbuildSubprocessBundler, EsbuildSubprocessConfig};
pub use future_rust_native::NativeRustBundler;
pub use scanner::{
    is_bare_specifier, normalize_path_lexical, scan_islands, FsResolver, InMemoryResolver,
    IslandsSet, Resolver, ScanError, ScanResult,
};
