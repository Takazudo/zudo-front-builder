//! Thread-pinned adapter that bridges [`EmbeddedV8RenderHost`] (which is
//! `!Send + !Sync` due to its V8 isolate) to the [`EmbeddedV8Host`] trait
//! (which requires `Send`).
//!
//! ## Design
//!
//! V8 isolates are bound to the OS thread that created them; `deno_core`'s
//! `JsRuntime` is `!Send` for exactly this reason. The renderer's
//! [`Backend::EmbeddedV8`] path expects a `Box<dyn EmbeddedV8Host + Send>`
//! so the guard can be moved into a `Mutex` for dev-mode thread safety.
//!
//! The adapter solves the impedance mismatch by parking the host on a
//! **dedicated OS thread** with its own single-thread tokio runtime. All
//! `dispatch_fetch` calls are forwarded to that thread via a
//! `std::sync::mpsc` channel; the caller blocks until the response arrives.
//!
//! Lifecycle:
//! - The dedicated thread is spawned once, when the adapter is constructed.
//! - The thread exits cleanly when the adapter is dropped: the request-
//!   channel sender is closed, causing the thread's receive loop to break.
//! - Panics inside the V8 thread are propagated back to the caller as
//!   `RendererError::EmbeddedV8` so the build surfaces a clear error
//!   rather than a silent hang.

use std::path::Path;
use std::sync::mpsc;
use std::thread;

use zfb_build::renderer::{EmbeddedV8Host, HttpResponseLike, RendererError};
use zfb_render::{
    BundleModuleLoader, EmbeddedV8RenderHost, HttpRequestLike, PluginRegistryHooks,
};

/// Request sent from the caller thread to the V8 host thread.
struct DispatchRequest {
    url_path: String,
    /// HTTP method. Defaults to `"GET"` for the legacy
    /// [`EmbeddedV8Host::dispatch_fetch`] path; the full-fidelity
    /// [`EmbeddedV8Host::dispatch_fetch_full`] path (issue #367) sets
    /// this from the caller.
    method: String,
    /// Request headers — empty map on the legacy GET path. Keys are
    /// stored lower-case on the wire; the V8 host normalises them
    /// before constructing the JS `Headers` object.
    headers: std::collections::BTreeMap<String, String>,
    /// Request body. Empty for GET/HEAD; carries POST/PUT/PATCH
    /// payload bytes on the full-fidelity path.
    body: Vec<u8>,
    /// One-shot reply channel.
    reply: mpsc::SyncSender<Result<HttpResponseLike, RendererError>>,
}

/// [`EmbeddedV8Host`] impl that forwards requests to a pinned V8 thread.
///
/// This is the production adapter used by the `DefaultRunner` in
/// `commands/build.rs` and `commands/dev.rs`. Constructing it starts the
/// V8 host on a dedicated OS thread. Drop automatically shuts the thread
/// down.
pub struct ThreadedV8Host {
    /// Sender half of the request channel. Wrapped in `Option` so `Drop`
    /// can `take()` it (move it out of `self`) to close the channel
    /// before joining the thread.
    tx: Option<mpsc::SyncSender<DispatchRequest>>,
    /// Join handle for clean shutdown on drop.
    thread: Option<thread::JoinHandle<()>>,
}

// SAFETY: `ThreadedV8Host` contains only a `SyncSender` (inherently `Send`)
// and a `JoinHandle` (inherently `Send`). The `!Send` V8 isolate lives on the
// dedicated thread and is never exposed to other threads. The channel ensures
// strict single-caller access to the isolate.
unsafe impl Send for ThreadedV8Host {}

impl Drop for ThreadedV8Host {
    /// Close the request channel first (so the V8 thread's receive loop
    /// exits), then join the thread to guarantee a clean shutdown with no
    /// leaks.
    fn drop(&mut self) {
        // Drop the sender; the V8 thread's `for req in rx` loop will see
        // the channel closed and exit its async block.
        drop(self.tx.take());
        if let Some(handle) = self.thread.take() {
            // Best-effort join: if the thread panicked, the panic has already
            // been propagated to the caller via the reply channel; swallow it.
            let _ = handle.join();
        }
    }
}

impl ThreadedV8Host {
    /// Create a new host adapter for the bundle at `bundle_path`.
    ///
    /// Boots the V8 isolate on a dedicated thread and loads the bundle.
    /// Returns an error if the V8 runtime fails to initialise or if the
    /// bundle fails to load.
    // Convenience wrapper without hooks; callers currently use new_with_hooks directly.
    // Returns Box<dyn EmbeddedV8Host> (not Self) so callers hold an opaque trait object.
    #[allow(dead_code)]
    #[allow(clippy::new_ret_no_self)]
    pub fn new(bundle_path: &Path) -> Result<Box<dyn EmbeddedV8Host>, RendererError> {
        Self::new_with_hooks(bundle_path, PluginRegistryHooks::default())
    }

    /// Same as [`Self::new`] but wires plugin-registry hooks into the
    /// V8 module loader. The hooks are consumed at module-resolve time
    /// so plugin-registered aliases / virtual modules can be imported
    /// by code running inside the V8 host. See sub-issue #260.
    pub fn new_with_hooks(
        bundle_path: &Path,
        hooks: PluginRegistryHooks,
    ) -> Result<Box<dyn EmbeddedV8Host>, RendererError> {
        // Use a rendezvous channel (bound = 0) so the request loop cannot
        // get more than one request ahead. This matches the single-threaded
        // contract of `BackendHandle::dispatch`.
        let (tx, rx) = mpsc::sync_channel::<DispatchRequest>(0);

        // Boot result channel: the spawned thread sends `Ok(())` once the
        // host is ready, or `Err(msg)` if boot fails.
        let (boot_tx, boot_rx) = mpsc::sync_channel::<Result<(), String>>(0);

        let bundle_path_owned = bundle_path.to_path_buf();

        let thread_handle = thread::Builder::new()
            .name("zfb-v8-host".into())
            .spawn(move || {
                // Each V8 host needs its own single-thread tokio runtime
                // because `dispatch_fetch` is async and calls
                // `with_event_loop_promise`. We use `current_thread` so the
                // isolate never migrates off this OS thread.
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = boot_tx.send(Err(format!("tokio runtime build failed: {e}")));
                        return;
                    }
                };

                rt.block_on(async move {
                    // Construct the host. This parses the bundle and warms
                    // the V8 snapshot — the most expensive part of boot.
                    // Wire plugin hooks (sub-issue #260) so the loader can
                    // resolve registered aliases + virtual modules.
                    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
                    let mut host = match EmbeddedV8RenderHost::with_loader(loader) {
                        Ok(h) => h,
                        Err(e) => {
                            let _ = boot_tx.send(Err(format!("V8 host init failed: {e}")));
                            return;
                        }
                    };
                    // Load the bundle as the main ESM module.
                    let bundle_src = match std::fs::read_to_string(&bundle_path_owned) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = boot_tx.send(Err(format!(
                                "could not read bundle {}: {e}",
                                bundle_path_owned.display()
                            )));
                            return;
                        }
                    };
                    let bundle_name = bundle_path_owned
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("bundle.mjs");
                    use zfb_render::RenderHost as _;
                    if let Err(e) = host.execute_module(bundle_name, &bundle_src).await {
                        let _ = boot_tx.send(Err(format!("bundle load failed: {e}")));
                        return;
                    }

                    // Signal successful boot before entering the request loop.
                    let _ = boot_tx.send(Ok(()));
                    // Drop boot_tx so the caller side's recv returns.
                    drop(boot_tx);

                    // Request loop: serve one dispatch at a time.
                    for req in rx {
                        // Construct the request shape expected by
                        // dispatch_fetch. The legacy GET path keeps
                        // method = "GET" + empty headers/body via
                        // `DispatchRequest`'s defaults; the
                        // `dispatch_fetch_full` path (issue #367)
                        // forwards method/headers/body verbatim.
                        let body_opt = if req.body.is_empty() {
                            None
                        } else {
                            Some(req.body)
                        };
                        // Issue #367: lower-case headers on the way
                        // into the V8 host so the JS `Headers` object
                        // sees a canonical shape regardless of the
                        // axum casing the caller passed in.
                        let mut headers = std::collections::BTreeMap::new();
                        for (k, v) in req.headers {
                            headers.insert(k.to_ascii_lowercase(), v);
                        }
                        let http_req = HttpRequestLike {
                            url: format!("http://localhost{}", req.url_path),
                            method: req.method,
                            headers,
                            body: body_opt,
                        };
                        let result = host
                            .dispatch_fetch(http_req)
                            .await
                            .map(|resp| {
                                // Deep-review fix (PR #376): forward ALL
                                // response headers (Cache-Control, Set-
                                // Cookie, Location, CORS, X-Trace-Id, …)
                                // so the dev SSR seam matches Cloudflare
                                // prod parity. `content_type` is duplicated
                                // out as the renderer's hot-path field;
                                // `headers` retains everything else.
                                //
                                // Multi-valued headers (notably Set-Cookie)
                                // still collapse to the last value through
                                // this BTreeMap shape — preexisting design
                                // limit of the seam, doc'd on
                                // `HttpResponseLike`.
                                let content_type = resp
                                    .headers
                                    .get("content-type")
                                    .cloned()
                                    .unwrap_or_default();
                                HttpResponseLike {
                                    status: resp.status,
                                    content_type,
                                    headers: resp.headers,
                                    body: resp.body,
                                }
                            })
                            .map_err(|e| RendererError::EmbeddedV8(e.to_string()));
                        // Best-effort: the caller may have already timed out;
                        // ignore send errors.
                        let _ = req.reply.send(result);
                    }
                    // rx is closed (ThreadedV8Host was dropped); exit cleanly.
                });
            })
            .map_err(|e| RendererError::EmbeddedV8(format!("could not spawn V8 host thread: {e}")))?;

        // Wait for the host to signal that boot succeeded.
        match boot_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(msg)) => {
                let _ = thread_handle.join();
                return Err(RendererError::EmbeddedV8(msg));
            }
            Err(_) => {
                // Thread exited before signalling — should not happen, but treat
                // as a boot failure.
                let _ = thread_handle.join();
                return Err(RendererError::EmbeddedV8(
                    "V8 host thread exited during boot without signalling".into(),
                ));
            }
        }

        Ok(Box::new(ThreadedV8Host {
            tx: Some(tx),
            thread: Some(thread_handle),
        }))
    }
}

impl EmbeddedV8Host for ThreadedV8Host {
    fn dispatch_fetch(&mut self, url_path: &str) -> Result<HttpResponseLike, RendererError> {
        self.send_request(
            url_path.to_string(),
            "GET".to_string(),
            std::collections::BTreeMap::new(),
            Vec::new(),
        )
    }

    fn dispatch_fetch_full(
        &mut self,
        url_path: &str,
        method: &str,
        headers: &std::collections::BTreeMap<String, String>,
        body: &[u8],
    ) -> Result<HttpResponseLike, RendererError> {
        self.send_request(
            url_path.to_string(),
            method.to_string(),
            headers.clone(),
            body.to_vec(),
        )
    }
}

impl ThreadedV8Host {
    /// Shared transport for both [`Self::dispatch_fetch`] and
    /// [`Self::dispatch_fetch_full`]. Builds the [`DispatchRequest`],
    /// sends it across the channel to the V8 thread, and blocks on the
    /// reply channel.
    fn send_request(
        &mut self,
        url_path: String,
        method: String,
        headers: std::collections::BTreeMap<String, String>,
        body: Vec<u8>,
    ) -> Result<HttpResponseLike, RendererError> {
        let tx = self.tx.as_ref().ok_or_else(|| {
            RendererError::EmbeddedV8("V8 host has already been shut down".into())
        })?;
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let req = DispatchRequest {
            url_path,
            method,
            headers,
            body,
            reply: reply_tx,
        };
        // Send the request; if the channel is broken the thread died.
        tx.send(req).map_err(|_| {
            RendererError::EmbeddedV8("V8 host thread has exited unexpectedly".into())
        })?;
        // Block until the reply arrives.
        reply_rx.recv().map_err(|_| {
            RendererError::EmbeddedV8("V8 host thread closed reply channel".into())
        })?
    }
}

/// Build an [`EmbeddedV8HostFactory`] that constructs a [`ThreadedV8Host`].
///
/// Returns the factory as a closure suitable for use in
/// [`zfb_build::renderer::Backend::EmbeddedV8`]. Callers that need
/// plugin-registry hooks should use [`make_v8_host_factory_with_hooks`] instead.
// Hooks-free variant; callers currently use make_v8_host_factory_with_hooks.
#[allow(dead_code)]
pub fn make_v8_host_factory() -> zfb_build::renderer::EmbeddedV8HostFactory {
    std::sync::Arc::new(|bundle_path: &Path| ThreadedV8Host::new(bundle_path))
}

/// Same as [`make_v8_host_factory`] but pre-binds plugin-registry hooks
/// the constructed host's module loader will consume. Each call to the
/// returned factory clones the hooks (cheap — they're already in shared
/// owned shape) so multiple host constructions during one build see the
/// same registered aliases / virtual modules. See sub-issue #260.
pub fn make_v8_host_factory_with_hooks(
    hooks: PluginRegistryHooks,
) -> zfb_build::renderer::EmbeddedV8HostFactory {
    std::sync::Arc::new(move |bundle_path: &Path| {
        ThreadedV8Host::new_with_hooks(bundle_path, hooks.clone())
    })
}

/// Translate a `zfb-build::SetupRegistries` (the build-side registry shape
/// from sub-issue #255) plus the pre-fetched virtual-module source map into
/// the loader-side [`PluginRegistryHooks`] (#260's owned mirror). Virtual
/// loaders are invoked async **before** this call so the loader can return
/// the pre-resolved source synchronously inside the V8 isolate.
///
/// `virtual_sources` is keyed by specifier and produced by the caller
/// (see `commands/build.rs` and `commands/dev.rs`) by walking
/// `registries.virtual_modules` and awaiting each `invoke_virtual_loader`.
pub fn translate_setup_registries_to_hooks(
    registries: &zfb_build::SetupRegistries,
    virtual_sources: &std::collections::BTreeMap<String, String>,
) -> PluginRegistryHooks {
    use std::collections::HashMap;
    let mut aliases: HashMap<String, zfb_render::AliasHook> = HashMap::new();
    for (from, entry) in registries.aliases.iter() {
        aliases.insert(
            from.clone(),
            zfb_render::AliasHook {
                target: entry.target.clone(),
                plugin: entry.plugin.clone(),
            },
        );
    }
    let mut virtual_modules: HashMap<String, zfb_render::VirtualModuleHook> = HashMap::new();
    for (specifier, vm_entry) in registries.virtual_modules.iter() {
        let source = virtual_sources
            .get(specifier)
            .cloned()
            .unwrap_or_default();
        virtual_modules.insert(
            specifier.clone(),
            zfb_render::VirtualModuleHook {
                source,
                plugin: vm_entry.plugin.clone(),
            },
        );
    }
    PluginRegistryHooks {
        aliases,
        virtual_modules,
    }
}
