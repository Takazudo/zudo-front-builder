//! `zfb-islands` — the islands runtime half of the zudo-front-builder build
//! pipeline.
//!
//! Responsibilities (this crate covers Sub 1 of Epic #6):
//!
//! 1. Detect React/Preact `"use client"` components reachable from page
//!    entries and produce a deterministic, sorted islands set keyed by
//!    stable component-name identity (file path + exported name). See
//!    [`scanner`].
//!
//! 2. Define the public [`ClientBundler`] trait — the contract the islands
//!    bundler (Sub 2) implements. The trait wraps the future esbuild
//!    subprocess engine and documents the swap-in story for a future
//!    Rust-native bundler (placeholder lives in [`future_rust_native`],
//!    analog of `zfb_css::native_engine`).
//!
//! ## Layering
//!
//! Like `zfb-css`, this crate deliberately does **not** depend on
//! `zfb-render`. It takes paths and source content as input (via a
//! [`scanner::Resolver`] the caller supplies) and returns plain Rust types.
//! Wiring into the full render orchestrator happens in Epic 7.

pub mod bundler;
pub mod future_rust_native;
pub mod scanner;

pub use bundler::{
    bundle_link_href, BundleConfig, BundleOutput, ClientBundler, Island, ModuleId,
};
pub use future_rust_native::NativeRustBundler;
pub use scanner::{
    is_bare_specifier, normalize_path_lexical, scan_islands, FsResolver, InMemoryResolver,
    IslandsSet, Resolver, ScanError, ScanResult,
};
