//! Deno-specific entropy registration.
#[cfg(test)]
use super::limits;
use deno_core::{op2, OpState};
use deno_error::JsErrorBox;
use std::rc::Rc;
#[cfg(test)]
use std::sync::atomic::Ordering;
pub mod digest;
pub use zfb_js_host::crypto::{
    fill_random_bytes, map_os_entropy_result, EntropyError, EntropySource, OsEntropy,
    OS_CSPRNG_KIND, OS_ENTROPY_CALLS,
};
trait IntoJsError {
    fn into_js_error_box(self) -> JsErrorBox;
}
impl IntoJsError for EntropyError {
    fn into_js_error_box(self) -> JsErrorBox {
        JsErrorBox::new(self.js_error_class(), self.to_string())
    }
}
/// The entropy source parked in `OpState` at extension init.
pub struct HostEntropySource(pub Rc<dyn EntropySource>);

/// Fill a caller-provided buffer from the OS CSPRNG.
///
/// **Synchronous on purpose** — see this module's header. `#[op2(fast)]`
/// is the synchronous op form; the buffer is written in place through
/// V8's backing store, so no copy crosses the boundary.
#[op2(fast)]
pub fn op_zfb_random_bytes(
    state: &mut OpState,
    #[buffer] out: &mut [u8],
) -> std::result::Result<(), JsErrorBox> {
    // The zero-length no-op succeeds before the source is even looked
    // up, so it cannot fail on a host missing the extension either —
    // see [`fill_random_bytes`] for why this is not a fail-closed hole.
    if out.is_empty() {
        return Ok(());
    }
    let source = state
        .try_borrow::<HostEntropySource>()
        .ok_or_else(|| EntropyError::Unavailable {
            detail: "OS entropy source is not installed in this runtime".to_string(),
        })
        .map_err(EntropyError::into_js_error_box)?
        .0
        .clone();
    fill_random_bytes(source.as_ref(), out).map_err(EntropyError::into_js_error_box)
}

deno_core::extension!(
    zfb_crypto,
    // `op_zfb_digest` needs no state of its own — it is a pure function
    // of (algorithm, bytes), so there is nothing to park in `OpState`
    // and nothing that can be half-installed.
    ops = [op_zfb_random_bytes, digest::op_zfb_digest],
    state = |state| {
        state.put(HostEntropySource(Rc::new(OsEntropy)));
    },
);

/// Whether [`op_zfb_random_bytes`] is registered as a **synchronous**
/// op.
///
/// Read straight off deno_core's own `OpDecl`, so the "deliberately
/// synchronous" decision documented above cannot be quietly reversed:
/// an async op here would hand `crypto.getRandomValues` a promise it
/// cannot await.
pub fn op_is_sync() -> bool {
    // `#[op2]` rewrites the annotated fn into a `const fn() -> OpDecl`,
    // so calling it yields the declaration deno_core itself registers.
    !op_zfb_random_bytes().is_async
}

#[cfg(test)]
mod tests;
