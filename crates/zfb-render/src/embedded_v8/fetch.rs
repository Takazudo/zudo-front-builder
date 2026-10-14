//! Deno-specific transport adapter and cancellation bookkeeping.
#[cfg(test)]
use super::limits;
use crate::dispatch_mode::DispatchMode;
use deno_core::{op2, CancelFuture, CancelHandle, JsBuffer, OpState};
use deno_error::JsErrorBox;
use serde::Serialize;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
pub use zfb_js_host::fetch::*;
trait IntoJsError {
    fn into_js_error_box(self) -> JsErrorBox;
}
impl IntoJsError for FetchError {
    fn into_js_error_box(self) -> JsErrorBox {
        JsErrorBox::new(self.js_error_class(), self.to_string())
    }
}
/// In-flight fetches that carry a cancellation token, keyed by the id
/// the JS side minted for them.
///
/// `RefCell<HashMap<..>>` rather than anything atomic for the same
/// reason [`SubrequestCounter`] uses `Cell`: the host is `!Send +
/// !Sync` by construction.
#[derive(Debug, Default)]
pub struct CancelRegistry {
    handles: RefCell<HashMap<u32, Rc<CancelHandle>>>,
    /// Cancellations flagged but not yet observed by the op they were
    /// aimed at.
    ///
    /// `CancelHandle::cancel()` only marks the handle and wakes its
    /// waker — the transport is dropped when the cancelable future is
    /// **polled again**. A handler that aborts and immediately returns
    /// its `Response` resolves the dispatch promise first, and
    /// `with_event_loop_promise` can return on that without polling the
    /// event loop, leaving the socket open and the entry live while the
    /// host sits idle. This counter is what lets
    /// [`super::EmbeddedV8RenderHost::drain_cancelled_fetches`] know it
    /// still owes the loop a poll.
    pending: Cell<usize>,
}

impl CancelRegistry {
    /// Register a fresh handle for `id` and hand it back. A duplicate
    /// id replaces the older entry — the JS side mints ids from a
    /// monotonic counter, so that can only happen after a wrap, by
    /// which point the older call is long gone.
    pub(crate) fn register(&self, id: u32) -> Rc<CancelHandle> {
        let handle = Rc::new(CancelHandle::new());
        self.handles.borrow_mut().insert(id, handle.clone());
        handle
    }

    pub(crate) fn forget(&self, id: u32) {
        self.handles.borrow_mut().remove(&id);
    }

    /// Cancel the in-flight fetch registered under `id`, if any. An
    /// unknown id is a no-op: the call may have already finished, and a
    /// late abort must never be an error.
    pub(crate) fn cancel(&self, id: u32) {
        if let Some(handle) = self.handles.borrow_mut().remove(&id) {
            self.pending.set(self.pending.get() + 1);
            handle.cancel();
        }
    }

    /// The op saw its cancellation and dropped the transport.
    pub(crate) fn note_cancel_observed(&self) {
        self.pending.set(self.pending.get().saturating_sub(1));
    }

    /// Cancellations flagged but not yet observed. Drops to zero once
    /// every cancelled op has been polled to completion.
    pub fn pending_cancellations(&self) -> usize {
        self.pending.get()
    }

    /// Forget the outstanding count without waiting for it.
    ///
    /// Called at the end of a drain pass so a cancellation that raced a
    /// natural completion — the op returned `Ok` on the same turn the
    /// abort fired, so nothing will ever observe the flag — cannot make
    /// every later dispatch pay for the same phantom poll budget.
    pub(crate) fn clear_pending(&self) {
        self.pending.set(0);
    }

    /// Number of registered in-flight cancellable fetches. Tests use it
    /// to prove the registry does not leak an entry per call.
    pub fn len(&self) -> usize {
        self.handles.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Serde-facing mirror of [`FetchOutcome`] for the op's return value.
/// `body` becomes a `Uint8Array` on the JS side via
/// [`deno_core::ToJsBuffer`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchResponseSpec {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub url: String,
    pub redirected: bool,
    pub body: deno_core::ToJsBuffer,
}

impl From<FetchOutcome> for FetchResponseSpec {
    fn from(outcome: FetchOutcome) -> Self {
        Self {
            status: outcome.status,
            status_text: outcome.status_text,
            headers: outcome.headers,
            url: outcome.url,
            redirected: outcome.redirected,
            body: outcome.body.into(),
        }
    }
}

/// The shared non-blocking client, parked in `OpState` at extension
/// init so every op call reuses one connection pool.
pub struct FetchClient(pub reqwest::Client);

/// Resolve the shared client + counter out of `OpState`.
///
/// A missing entry is a **host-op failure** — the runtime is shutting
/// down, or the extension was never installed — and rejects. It must
/// never fall back to a fresh client or a synthetic response.
fn state_handles(
    state: &Rc<std::cell::RefCell<OpState>>,
    url: &str,
) -> Result<(reqwest::Client, Rc<SubrequestCounter>, Rc<CancelRegistry>), FetchError> {
    let state = state.borrow();
    let client = state
        .try_borrow::<FetchClient>()
        .ok_or_else(|| FetchError::HostUnavailable {
            url: url.to_string(),
            detail: "outbound HTTP client is not installed in this runtime".to_string(),
        })?
        .0
        .clone();
    let counter = state
        .try_borrow::<Rc<SubrequestCounter>>()
        .ok_or_else(|| FetchError::HostUnavailable {
            url: url.to_string(),
            detail: "subrequest counter is not installed in this runtime".to_string(),
        })?
        .clone();
    let cancels = state
        .try_borrow::<Rc<CancelRegistry>>()
        .ok_or_else(|| FetchError::HostUnavailable {
            url: url.to_string(),
            detail: "cancellation registry is not installed in this runtime".to_string(),
        })?
        .clone();
    Ok((client, counter, cancels))
}

/// The mode of the dispatch currently on the isolate, read out of
/// `OpState`. Absent means [`DispatchMode::BuildTime`] — the denying
/// default, so a runtime that never installed the state cannot become
/// the one that grants network access.
fn current_dispatch_mode(state: &Rc<std::cell::RefCell<OpState>>) -> DispatchMode {
    state
        .borrow()
        .try_borrow::<DispatchModeState>()
        .map(|m| m.0)
        .unwrap_or_default()
}

/// Issue one outbound HTTP request. **Asynchronous by construction** —
/// see this module's header for why that is non-negotiable.
///
/// `body` is a `Uint8Array`; `spec.has_body` says whether it means
/// anything (an empty payload and "no payload" are different requests).
///
/// ## This op — not the polyfill — is the trust boundary
///
/// `globalThis.Deno.core.ops.op_zfb_fetch` is reachable from bundle
/// code, so every policy that matters is checked here, before the
/// transport is entered:
///
/// 1. **Build-time denial** (guardrail 4). Consulted first, off
///    [`DispatchModeState`] in `OpState`, which no JS value can reach.
/// 2. **Request-body ceiling**, checked against the still-borrowed
///    `JsBuffer` so an oversized payload is refused *before* it is
///    copied into a `Vec` — `#[buffer(copy)]` would have performed that
///    allocation during argument decoding, which is exactly the
///    allocation the cap exists to prevent.
#[op2]
#[serde]
pub async fn op_zfb_fetch(
    state: Rc<std::cell::RefCell<OpState>>,
    #[serde] spec: FetchRequestSpec,
    #[buffer] body: JsBuffer,
) -> Result<FetchResponseSpec, JsErrorBox> {
    if current_dispatch_mode(&state) != DispatchMode::RequestTime {
        return Err(FetchError::BuildTimeDenied {
            url: spec.url.clone(),
        }
        .into_js_error_box());
    }
    let (client, counter, cancels) =
        state_handles(&state, &spec.url).map_err(FetchError::into_js_error_box)?;
    let config = FetchConfig::default();
    if spec.has_body && body.len() > config.max_request_body_bytes {
        return Err(FetchError::RequestBodyTooLarge {
            url: spec.url.clone(),
            limit: config.max_request_body_bytes,
        }
        .into_js_error_box());
    }
    let body = body.to_vec();

    // Registered on this first poll, which `deno_core` runs eagerly
    // inside the JS `op(...)` call — so the id is live before the
    // caller's `await` can yield, and any later abort finds it.
    let handle = spec.cancel_id.map(|id| cancels.register(id));
    let transport = perform_fetch(&client, &counter, &config, &spec, body);
    let result = match handle {
        Some(handle) => match transport.or_cancel(handle).await {
            Ok(result) => result,
            // Dropping `transport` here is the whole point: it closes
            // the socket instead of letting the request run on to the
            // wall-clock deadline with its result destined for the bin.
            Err(_canceled) => {
                cancels.note_cancel_observed();
                Err(FetchError::Aborted {
                    url: spec.url.clone(),
                })
            }
        },
        None => transport.await,
    };
    if let Some(id) = spec.cancel_id {
        cancels.forget(id);
    }
    Ok(result.map_err(FetchError::into_js_error_box)?.into())
}

/// Cancel the in-flight [`op_zfb_fetch`] registered under `cancel_id`.
///
/// Synchronous and infallible: an abort must take effect on the turn it
/// happens, and an id that has already completed is a no-op rather than
/// an error. It grants no capability — the only thing it can do is stop
/// a request the same isolate started — so it is deliberately NOT
/// mode-gated.
#[op2(fast)]
pub fn op_zfb_fetch_cancel(state: &mut OpState, #[smi] cancel_id: u32) {
    if let Some(cancels) = state.try_borrow::<Rc<CancelRegistry>>() {
        cancels.cancel(cancel_id);
    }
}

deno_core::extension!(
    zfb_fetch,
    ops = [op_zfb_fetch, op_zfb_fetch_cancel],
    state = |state| {
        // A client that cannot be built leaves `FetchClient` absent, so
        // every op call reports the host-op failure above rather than
        // panicking the isolate at boot.
        if let Ok(client) = build_fetch_client() {
            state.put(FetchClient(client));
        }
        state.put(Rc::new(SubrequestCounter::new()));
        state.put(Rc::new(CancelRegistry::default()));
        // Build-time until a dispatch says otherwise. A host that only
        // ever evaluates modules (config eval, `paths()`) therefore
        // never leaves the denying default.
        state.put(DispatchModeState(DispatchMode::BuildTime));
    },
);

/// Whether [`op_zfb_fetch`] is registered as an **async** op.
///
/// Read straight off deno_core's own `OpDecl`, so this cannot be
/// satisfied by a comment or a name: if the op ever became synchronous —
/// the one regression that would put a blocking network call on the
/// isolate thread — this flips to `false`.
pub fn op_is_async() -> bool {
    // `#[op2]` rewrites the annotated fn into a `const fn() -> OpDecl`,
    // so calling it yields the declaration deno_core itself registers.
    op_zfb_fetch().is_async
}

#[cfg(test)]
mod tests;
