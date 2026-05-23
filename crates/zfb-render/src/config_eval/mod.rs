//! In-process V8 config evaluator for `zfb.config.ts` bundles.
//!
//! ## Purpose
//!
//! Evaluates a pre-bundled ESM JavaScript module (the output of bundling
//! `zfb.config.ts` with esbuild `--platform=neutral`) and returns its
//! `default` export as a `serde_json::Value`.
//!
//! ## Serialisation contract — JSON.stringify inside the isolate
//!
//! The default export is serialised via `JSON.stringify` running inside the
//! V8 isolate, not via `serde_v8::from_v8`. This preserves byte-identical
//! behaviour with today's `config-loader.mjs` wire contract:
//! - Functions in objects are silently dropped (not a hard error).
//! - `Date` values become ISO strings.
//! - `undefined`, top-level functions, and Symbols were rejected earlier.
//!
//! ## No-Node-globals contract
//!
//! The config isolate has no `process`, no `Buffer`, and no `node:*` modules.
//! The in-memory module loader rejects every non-main-module specifier with a
//! clear error message (see [`NO_BARE_IMPORTS_ERROR`]). This is the explicit
//! product decision: `zfb.config.ts` is a data config, not a general Node
//! script — move runtime imports to your application code.
//!
//! ## Thread pinning
//!
//! `deno_core::JsRuntime` is `!Send` because V8 isolates are bound to the OS
//! thread that created them. [`ThreadedConfigEvaluator`] wraps the one-shot
//! evaluation in a dedicated OS thread + `current_thread` tokio runtime,
//! making the async result available to `Send` callers. The caller is
//! responsible for wrapping `eval_bundle` in `tokio::task::spawn_blocking`
//! so the async tokio worker is not pinned during V8 boot.

use std::sync::mpsc;
use std::thread;

use thiserror::Error;

mod runtime;

pub use runtime::evaluate_config_bundle;

/// The exact error message emitted by the config-eval module loader when a
/// non-main-module specifier is encountered.
///
/// This text is exposed as a public constant so tests can assert against the
/// exact bytes without copy-pasting.
// product-decision: zfb.config.ts is a data config; bare/node:* imports are
// forbidden. The loader rejects them with this exact message.
pub const NO_BARE_IMPORTS_ERROR: &str =
    "zfb.config.ts: bare imports and node:* specifiers are not available inside \
    the config evaluator. zfb.config.ts is a data config, not a general Node \
    script — move runtime imports to your application code.";

// ---------------------------------------------------------------------------
// ConfigEvalError
// ---------------------------------------------------------------------------

/// Errors produced by the config evaluator.
#[derive(Debug, Error)]
pub enum ConfigEvalError {
    /// The `default` export is not a plain object (e.g. a number, string,
    /// function, `undefined`, or `null`).
    #[error("default export must be a plain object")]
    NotAPlainObject,

    /// A bare import or `node:*` specifier was encountered. The inner string
    /// contains the loader's rejection message.
    #[error("{0}")]
    BareImportRejected(String),

    /// The module source threw an error at top level, or the event loop
    /// propagated a rejection.
    #[error("config evaluation error: {0}")]
    Evaluation(String),

    /// The main ESM module could not be loaded (e.g. specifier resolution
    /// failed, or the loader rejected an import).
    #[error("config module load error: {0}")]
    ModuleLoad(String),

    /// The dedicated V8 thread failed to produce a result (e.g. the thread
    /// panicked or the channel was unexpectedly closed).
    #[error("config eval thread error: {0}")]
    ThreadError(String),
}

// ---------------------------------------------------------------------------
// ThreadedConfigEvaluator
// ---------------------------------------------------------------------------

/// Synchronous, thread-safe wrapper around [`evaluate_config_bundle`].
///
/// V8 isolates are `!Send`; this helper pins the evaluation on a dedicated
/// OS thread with a `current_thread` tokio runtime, communicates the result
/// back via a one-shot channel, and returns synchronously to the caller.
///
/// Each call to [`Self::eval_bundle`] spawns a new OS thread for a fresh
/// isolate. The thread exits after producing exactly one result. There is no
/// long-lived state.
///
/// ## Caller contract
///
/// Wrap calls to [`Self::eval_bundle`] in `tokio::task::spawn_blocking` when
/// calling from an async context — V8 boot may take hundreds of milliseconds
/// and should not pin a tokio worker thread. Sub 3 (`zfb::config`) is
/// responsible for this.
pub struct ThreadedConfigEvaluator;

impl ThreadedConfigEvaluator {
    /// Evaluate `js_source` as a self-contained ESM config module and return
    /// its `default` export as a `serde_json::Value`.
    ///
    /// Spawns a dedicated OS thread, builds a `current_thread` tokio runtime,
    /// evaluates the bundle, and sends the result back over a one-shot channel.
    /// The thread exits after the result is sent.
    pub fn eval_bundle(js_source: &str) -> Result<serde_json::Value, ConfigEvalError> {
        let js_source_owned = js_source.to_string();

        // One-shot channel: the V8 thread sends exactly one message.
        let (tx, rx) = mpsc::sync_channel::<Result<serde_json::Value, ConfigEvalError>>(1);

        let handle = thread::Builder::new()
            .name("zfb-config-eval".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(Err(ConfigEvalError::ThreadError(format!(
                            "tokio runtime build failed: {e}"
                        ))));
                        return;
                    }
                };

                let result = rt.block_on(evaluate_config_bundle(&js_source_owned));
                let _ = tx.send(result);
            })
            .map_err(|e| {
                ConfigEvalError::ThreadError(format!("could not spawn eval thread: {e}"))
            })?;

        // Wait for the result from the V8 thread.
        let result = rx.recv().map_err(|_| {
            ConfigEvalError::ThreadError(
                "V8 eval thread closed the channel without sending a result".into(),
            )
        })?;

        // Join the thread to ensure clean shutdown (no thread leak).
        let _ = handle.join();

        result
    }
}
