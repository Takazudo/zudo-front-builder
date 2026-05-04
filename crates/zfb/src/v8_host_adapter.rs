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
use zfb_render::{EmbeddedV8RenderHost, HttpRequestLike};

/// Request sent from the caller thread to the V8 host thread.
struct DispatchRequest {
    url_path: String,
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
    /// Sender half of the request channel.
    tx: mpsc::SyncSender<DispatchRequest>,
    /// Join handle so we wait for a clean shutdown on drop.
    _thread: Option<thread::JoinHandle<()>>,
}

// SAFETY: `ThreadedV8Host` contains only a `SyncSender` (inherently `Send`)
// and a `JoinHandle` (inherently `Send`). The `!Send` V8 isolate lives on the
// dedicated thread and is never exposed to other threads. The channel ensures
// strict single-caller access to the isolate.
unsafe impl Send for ThreadedV8Host {}

impl ThreadedV8Host {
    /// Create a new host adapter for the bundle at `bundle_path`.
    ///
    /// Boots the V8 isolate on a dedicated thread and loads the bundle.
    /// Returns an error if the V8 runtime fails to initialise or if the
    /// bundle fails to load.
    pub fn new(bundle_path: &Path) -> Result<Box<dyn EmbeddedV8Host>, RendererError> {
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
                    let mut host = match EmbeddedV8RenderHost::new() {
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
                        // Construct the request shape expected by dispatch_fetch.
                        let http_req = HttpRequestLike {
                            url: format!("http://localhost{}", req.url_path),
                            method: "GET".into(),
                            headers: Default::default(),
                            body: None,
                        };
                        let result = host
                            .dispatch_fetch(http_req)
                            .await
                            .map(|resp| HttpResponseLike {
                                status: resp.status,
                                content_type: resp
                                    .headers
                                    .get("content-type")
                                    .cloned()
                                    .unwrap_or_default(),
                                body: resp.body,
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
            tx,
            _thread: Some(thread_handle),
        }))
    }
}

impl EmbeddedV8Host for ThreadedV8Host {
    fn dispatch_fetch(&mut self, url_path: &str) -> Result<HttpResponseLike, RendererError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let req = DispatchRequest {
            url_path: url_path.to_string(),
            reply: reply_tx,
        };
        // Send the request; if the channel is broken the thread died.
        self.tx.send(req).map_err(|_| {
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
/// [`zfb_build::renderer::Backend::EmbeddedV8`]. This is the canonical
/// production factory — callers in `build.rs` and `dev.rs` use it to wire
/// the embedded V8 path.
pub fn make_v8_host_factory() -> zfb_build::renderer::EmbeddedV8HostFactory {
    std::sync::Arc::new(|bundle_path: &Path| ThreadedV8Host::new(bundle_path))
}
