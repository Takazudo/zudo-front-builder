//! `zfb-config-loader` — the `zfb.config.ts` evaluator, hoisted out of the
//! `zfb` bin crate so both `zfb` and `zfb-server` can call it without a
//! circular dependency (issue #1037).
//!
//! `zfb` depends on `zfb-server`, not the reverse, so the TS evaluator could
//! not live in either crate and be shared. It is a leaf crate here, depended
//! on by both.
//!
//! ## What it does
//!
//! [`load_from_ts_file`] bundles a `zfb.config.ts` with the pinned `esbuild`
//! binary and evaluates the resulting ESM, returning the user's `default`
//! export as a [`serde_json::Value`] plus the resolved plugin module
//! specifiers. The caller deserialises the value into its own config struct
//! (`zfb::config::Config`, or `zfb-server`'s embed-relevant `EmbedConfig`).
//! Keeping the loader config-shape-agnostic is what lets the same evaluator
//! serve both consumers — neither config struct is reachable from this leaf.
//!
//! ## Evaluation path (feature `embed_v8`)
//!
//! - **Default builds** (`embed_v8`): the bundle is evaluated **in-process**
//!   by the embedded V8 isolate that ships with `zfb-render`. No runtime Node
//!   dependency. esbuild bundles with `--platform=neutral`, so `node:*`
//!   imports are bundle-time errors — `zfb.config.ts` is a self-contained
//!   data config.
//! - **Slim builds** (`--no-default-features`): the in-process evaluator is
//!   compiled out; evaluation falls back to a `node` subprocess
//!   ([`load_ts_via_subprocess`]). This path requires `node` in `PATH` and
//!   surfaces a clean error when it is absent.

mod loader;
pub mod node_resolve;

pub use loader::{
    load_from_ts_file, output_bounded, output_bounded_with, resolve_esbuild_binary_with_handle,
    resolve_plugin_path_to_file_url, AttemptFut, EmbeddedEsbuildGetter, LoadOptions,
    LoadedTsConfig, CONFIG_SUBPROCESS_TIMEOUT, ETXTBSY_MAX_RETRIES, ETXTBSY_RETRY_DELAY,
};
pub use node_resolve::{
    resolve_node_bare_specifier, resolve_package_dir, resolve_plugin_from_anchors,
};
