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
//! [`load_from_ts_file`] bundles a `zfb.config.ts` (local imports and the
//! `zfb/config` / `@takazudo/zfb/config` helper included) into one ESM file
//! through the supervised native compiler worker, evaluates it, and returns
//! the user's `default` export as a [`serde_json::Value`] plus the resolved
//! plugin module specifiers. The caller deserialises the value into its own
//! config struct (`zfb::config::Config`, or `zfb-server`'s embed-relevant
//! `EmbedConfig`). Keeping the loader config-shape-agnostic is what lets the
//! same evaluator serve both consumers — neither config struct is reachable
//! from this leaf.
//!
//! The host supplies the worker through [`LoadOptions::compiler_worker`]: the
//! zfb CLI passes [`WorkerLauncher::zfb_cli`], an embedder a launcher for its
//! own executable whose `main` calls `zfb_compiler_worker::run_if_requested()`
//! first. Without one a `.ts` load fails with an actionable error.
//!
//! ## Evaluation path (feature `direct-v8`)
//!
//! - **Default builds** (`direct-v8`): the config is bundled for the neutral
//!   platform and evaluated **in-process** by the embedded V8 isolate that
//!   ships with `zfb-render`. No runtime Node dependency. `zfb.config.ts` is
//!   self-contained data config: a `node:*` or bare Node builtin import is
//!   rejected before evaluation.
//! - **Slim builds** (`--no-default-features`): the in-process evaluator is
//!   compiled out; the config is bundled for the Node platform and evaluated
//!   by a `node` subprocess, where builtins stay available. This path
//!   requires `node` in `PATH` and surfaces a clean error when it is absent.

mod loader;
pub mod node_resolve;

pub use loader::{
    load_from_ts_file, output_bounded, output_bounded_with, resolve_plugin_path_to_file_url,
    AttemptFut, LoadOptions, LoadedTsConfig, CONFIG_SUBPROCESS_TIMEOUT, ETXTBSY_MAX_RETRIES,
    ETXTBSY_RETRY_DELAY,
};
pub use node_resolve::{
    resolve_node_bare_specifier, resolve_package_dir, resolve_plugin_from_anchors,
};
pub use zfb_compiler_worker::WorkerLauncher;
