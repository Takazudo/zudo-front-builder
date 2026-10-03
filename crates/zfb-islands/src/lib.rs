//! `zfb-islands` — the islands runtime half of the zudo-front-builder build
//! pipeline.
//!
//! Responsibilities (Epic 6 / issue #6):
//!
//! 1. Keep the reachable module/resource graph and discover the concrete
//!    functions used by SDK `Island` boundaries in client-eligible modules.
//!    Return a deterministic target set with binding-based alias dedup and
//!    public-marker collision checks. See [`scanner`].
//!
//! 2. Define the public [`ClientBundler`] trait — the contract the islands
//!    bundler implements — and provide the production
//!    `EsbuildSubprocessBundler` (Sub 2) that wraps the esbuild CLI
//!    subprocess. The trait documents the swap-in story for a future
//!    Rust-native bundler (placeholder lives in [`future_rust_native`],
//!    analog of `zfb_css::native_engine`).
//!
//! ## Layering
//!
//! Like `zfb-css`, this crate deliberately does **not** depend on
//! `zfb-render`. It takes paths and source content as input (via a
//! [`scanner::Resolver`] the caller supplies) and returns plain Rust types
//! plus asset paths. Wiring into the full render orchestrator happens in
//! Epic 7.

pub mod bundler;
pub mod client_scripts;
pub mod esbuild;
pub mod future_rust_native;
pub mod html_tree;
pub mod manifest;
pub mod scanner;

pub use bundler::{
    build_production_islands_asset, bundle_link_href, BundleChunk, BundleConfig, BundleMode,
    BundleOutput, BundleResource, ClientBundler, Island, IslandsChunk, IslandsResource, ModuleId,
    ModuleWorkerBundleEntry, ProductionIslandsAsset,
};
pub use client_scripts::{
    build_production_client_scripts, build_production_client_scripts_with_workers,
    client_script_entry_name, discover_client_scripts, is_client_script_file,
    ClientScriptCollision, ClientScriptEntry, ClientScriptWorkerEntry, ProductionClientScriptAsset,
    CLIENT_SCRIPT_DISCOVERY_ROOTS, CLIENT_SCRIPT_EXTENSIONS, CLIENT_SCRIPT_INFIX,
};
pub use esbuild::{
    hash_8, is_zfb_islands_temp_file, render_shared_bundle_entry_source, ClientScriptBundleOutput,
    EsbuildSubprocessBundler, EsbuildSubprocessConfig, IslandsBundleBuildIdentityError,
    StageAuditPolicy, EXPECTED_ESBUILD_SHA256, EXPECTED_ESBUILD_VERSION,
};
pub use future_rust_native::NativeRustBundler;
pub use html_tree::HtmlTree;
pub use manifest::{is_same_package_duplicate, manifest_json, write_manifest, Collision, Manifest};
pub use scanner::{
    is_bare_specifier, scan_islands, scan_islands_with_meta,
    scan_islands_with_meta_and_first_party_root, scan_reachable_modules,
    scan_reachable_modules_with_meta, scan_reachable_modules_with_meta_and_first_party_root,
    FsResolver, InMemoryResolver, IslandsSet, ModuleWorkerEdge, RawImportEdge,
    ReachableModulesMeta, Resolver, ScanError, ScanMeta, ScanResult, WorkspacePackageImportEdge,
};
pub use zfb_types::{
    module_worker_content_hash, module_worker_filename, module_worker_url_specifier,
    ModuleWorkerPathError, MODULE_WORKER_CSP_GLOB, MODULE_WORKER_FILENAME_PREFIX,
};
// Re-export from zfb-types so downstream crates get a stable path.
pub use zfb_types::normalize_path_lexical;
