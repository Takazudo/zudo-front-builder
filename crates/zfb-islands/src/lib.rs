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
//! 3. Server-side HTML rewrite that turns each rendered island's
//!    marker-bracketed output into the `<div data-zfb-island="…"
//!    data-props="…">` wrapper the client-side hydration runtime walks
//!    (Sub 3 — see [`hydration`]).
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
pub mod hydration;
pub mod manifest;
pub mod scanner;

pub use bundler::{
    bundle_link_href, island_link_href, BundleConfig, BundleOutput, ClientBundler, FrameworkKind,
    Island, IslandBundle, ModuleId, PerIslandBundleOutput,
};
pub use esbuild::{
    hash_8, render_island_entry_source, render_runtime_entry_source, EsbuildSubprocessBundler,
    EsbuildSubprocessConfig, EXPECTED_ESBUILD_SHA256, EXPECTED_ESBUILD_VERSION,
};
pub use future_rust_native::NativeRustBundler;
pub use hydration::{
    hydration_script_tag, inject_runtime_script_into_head, islands_runtime_script_tag,
    rewrite_islands, rewrite_islands_in_attr_skeleton, IslandDescriptor, IslandRewriteError,
    IslandSkeletonRewriteError,
};
pub use manifest::{manifest_json, write_manifest, Collision, Manifest};
pub use scanner::{
    is_bare_specifier, normalize_path_lexical, scan_islands, FsResolver, InMemoryResolver,
    IslandsSet, Resolver, ScanError, ScanResult,
};
