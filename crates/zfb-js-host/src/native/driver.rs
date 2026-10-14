//! Cooperative V8/Rust execution on the isolate owner thread.
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use super::diagnostics::{exception_text, NativeError};
use super::{services, values, NativeHost};
use crate::crypto::{EntropySource, OsEntropy};
use crate::dispatch_mode::DispatchMode;
use crate::fetch::FetchConfig;

impl NativeHost {
    pub fn new_services() -> Result<Self, NativeError> {
        Self::with_services(services::Services::defaults())
    }

    /// Small-limit and entropy-failure injection for executable native evidence.
    /// Ordinary callers use the canonical defaults through new_services().
    pub fn new_services_with(
        config: FetchConfig,
        entropy: Rc<dyn EntropySource>,
    ) -> Result<Self, NativeError> {
        Self::with_services(services::Services::new(config, entropy))
    }

    pub(super) fn with_services(services: services::Services) -> Result<Self, NativeError> {
        let config = services.config;
        let mut host = Self::new_inner(crate::shims::WEB_POLYFILLS_SRC, Some(services))?;
        let mut random = [0u8; 32];
        OsEntropy.fill(&mut random).map_err(|error| {
            NativeError::Bootstrap(format!("dispatch-mode nonce entropy unavailable: {error}"))
        })?;
        host.mode_nonce = format!(
            "zfb-mode-{}",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        host.execute_script("zfb:browser_event", crate::shims::BROWSER_EVENT_SRC)?;
        let mut limits: serde_json::Value =
            serde_json::from_str(&crate::limits::limits_js_literal())
                .map_err(|error| NativeError::Bootstrap(error.to_string()))?;
        limits["fetchTimeoutMs"] = config.timeout_ms.into();
        limits["maxRedirects"] = config.max_redirects.into();
        limits["maxRequestBodyBytes"] = config.max_request_body_bytes.into();
        limits["maxResponseBodyBytes"] = config.max_response_body_bytes.into();
        limits["maxSubrequestsPerDispatch"] = config.max_subrequests.into();
        let shim = crate::shims::HOST_GLOBALS_SHIM_SRC
            .replace(crate::shims::MODE_NONCE_PLACEHOLDER, &host.mode_nonce)
            .replace(crate::shims::LIMITS_PLACEHOLDER, &limits.to_string());
        host.execute_script("zfb:host_globals", &shim)?;
        Ok(host)
    }

    /// Execute an expression under the existing default.fetch dispatch shim.
    /// The Rust mode and fresh budget are selected before any authored JS runs.
    pub async fn evaluate_script(
        &mut self,
        source: &str,
        mode: DispatchMode,
    ) -> Result<serde_json::Value, NativeError> {
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(NativeError::Evaluation(
                "native async services require a Tokio runtime".into(),
            ));
        }
        let state = self
            .services
            .as_ref()
            .ok_or_else(|| NativeError::Evaluation("native services are not installed".into()))?;
        state.borrow_mut().begin_dispatch(mode);
        // These expression bytes match the independent baseline runner's Worker
        // body. Module/Worker loading remains the next separate gate.
        let setup = format!(
            "__zfb.setBundle({{async fetch(){{return Response.json(await ({source}));}}}});"
        );
        self.execute_script("file:///zfb/service-probe.mjs", &setup)?;
        let nonce = serde_json::to_string(&self.mode_nonce)
            .map_err(|error| NativeError::Evaluation(error.to_string()))?;
        let call = format!(
            "__zfb.dispatch('http://zfb.local/service-probe','GET',{{}},undefined,'{}',{nonce})",
            mode.as_js_str()
        );
        let value = self.execute_script("zfb:service_dispatch", &call)?;
        let promise = {
            v8::scope!(let scope, &mut self.isolate);
            let context = v8::Local::new(scope, &self.context);
            let scope = &mut v8::ContextScope::new(scope, context);
            let value = v8::Local::new(scope, value);
            let promise = v8::Local::<v8::Promise>::try_from(value)
                .map_err(|_| NativeError::Evaluation("dispatch did not return a promise".into()))?;
            promise.mark_as_handled();
            v8::Global::new(scope, promise)
        };
        let result = self.drive(promise).await?;
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        let result = v8::Local::new(scope, result);
        let object = v8::Local::<v8::Object>::try_from(result)
            .map_err(|_| NativeError::Evaluation("dispatch result is not an object".into()))?;
        let key = v8::String::new(scope, "body")
            .ok_or_else(|| NativeError::Evaluation("V8 body key allocation failed".into()))?;
        let body = object
            .get(scope, key.into())
            .ok_or_else(|| NativeError::Evaluation("dispatch response body missing".into()))?;
        let bytes = values::copy_view(body, i32::MAX as usize).map_err(NativeError::Evaluation)?;
        serde_json::from_slice(&bytes).map_err(|error| {
            NativeError::Evaluation(format!("script response is not JSON: {error}"))
        })
    }

    pub fn pending_fetches(&self) -> usize {
        self.services
            .as_ref()
            .map(|state| state.borrow().pending.len())
            .unwrap_or(0)
    }

    pub(super) async fn drive(
        &mut self,
        promise: v8::Global<v8::Promise>,
    ) -> Result<v8::Global<v8::Value>, NativeError> {
        let mut sleeper: Option<Pin<Box<tokio::time::Sleep>>> = None;
        poll_fn(|cx| self.poll_drive(cx, &promise, &mut sleeper)).await
    }

    fn poll_drive(
        &mut self,
        cx: &mut Context<'_>,
        promise: &v8::Global<v8::Promise>,
        sleeper: &mut Option<Pin<Box<tokio::time::Sleep>>>,
    ) -> Poll<Result<v8::Global<v8::Value>, NativeError>> {
        loop {
            let (tasks, _) = self.foreground.take_ready(cx.waker());
            for task in tasks {
                task.run();
            }
            self.isolate.perform_microtask_checkpoint();
            let state = self.services.as_ref().cloned();
            let mut ready = Vec::new();
            let mut polled_through = 0;
            if let Some(state) = &state {
                let mut state = state.borrow_mut();
                polled_through = state.next_id;
                let mut index = 0;
                while index < state.pending.len() {
                    match state.pending[index].transport.as_mut().poll(cx) {
                        Poll::Ready(result) => {
                            let operation = state.pending.swap_remove(index);
                            state.forget(&operation);
                            ready.push((operation, result));
                        }
                        Poll::Pending => index += 1,
                    }
                }
            }
            let mut made_progress = !ready.is_empty();
            // No state borrow survives a V8 settlement: promise hooks and getters
            // are permitted to call the fixed native bridge recursively.
            {
                v8::scope!(let scope, &mut self.isolate);
                let context = v8::Local::new(scope, &self.context);
                let scope = &mut v8::ContextScope::new(scope, context);
                for (operation, result) in ready {
                    drop(operation.transport);
                    let resolver = v8::Local::new(scope, operation.resolver);
                    match result {
                        Ok(result) => {
                            if let Some(value) = values::outcome(scope, result) {
                                resolver.resolve(scope, value);
                            }
                        }
                        Err(error) => {
                            if let Some(error) = values::named_error(
                                scope,
                                error.js_error_class(),
                                &error.to_string(),
                            ) {
                                resolver.reject(scope, error);
                            }
                        }
                    }
                }
                made_progress |= super::worker_modules::poll_imports(scope);
                scope.perform_microtask_checkpoint();
            }
            let (pending, new_operations) = state
                .as_ref()
                .map(|state| {
                    let state = state.borrow();
                    (state.pending.len(), state.next_id != polled_through)
                })
                .unwrap_or_default();
            if made_progress || new_operations || self.ready_module_imports() {
                continue;
            }
            let (failure, rejection) = {
                let mut modules = self.modules.borrow_mut();
                // Host-observed dispatch rejection is handled by the explicit
                // state read below; unrelated unhandled user promises still fail.
                (
                    modules.callback_failures.pop(),
                    modules.unhandled_rejections.pop(),
                )
            };
            v8::scope!(let scope, &mut self.isolate);
            let context = v8::Local::new(scope, &self.context);
            let scope = &mut v8::ContextScope::new(scope, context);
            if let Some(error) = failure {
                return Poll::Ready(Err(NativeError::Evaluation(error)));
            }
            if let Some((_, reason)) = rejection {
                let reason = v8::Local::new(scope, reason);
                return Poll::Ready(Err(NativeError::Evaluation(exception_text(
                    scope,
                    Some(reason),
                ))));
            }
            let promise = v8::Local::new(scope, promise);
            match promise.state() {
                v8::PromiseState::Fulfilled => {
                    return Poll::Ready(Ok(v8::Global::new(scope, promise.result(scope))))
                }
                v8::PromiseState::Rejected => {
                    let reason = promise.result(scope);
                    return Poll::Ready(Err(NativeError::Evaluation(exception_text(
                        scope,
                        Some(reason),
                    ))));
                }
                v8::PromiseState::Pending => {}
            }
            // A foreground task or microtask may have posted more work after
            // the initial drain. Re-read under the same registered waker before
            // deciding that the promise has no way to make progress.
            let background_work = scope.has_pending_background_tasks();
            let (ready, deadline) = self.foreground.status();
            if ready {
                continue;
            }
            if let Some(deadline) = deadline {
                let deadline = tokio::time::Instant::from_std(deadline);
                if sleeper.as_ref().map(|sleep| sleep.deadline()) != Some(deadline) {
                    *sleeper = Some(Box::pin(tokio::time::sleep_until(deadline)));
                }
                if let Some(sleep) = sleeper.as_mut() {
                    if sleep.as_mut().poll(cx).is_ready() {
                        continue;
                    }
                }
            } else {
                *sleeper = None;
            }
            if pending == 0 && deadline.is_none() && !background_work {
                return Poll::Ready(Err(NativeError::Evaluation("dispatch promise is unresolved with no pending native work (direct-V8 prototype limitation)".into())));
            }
            return Poll::Pending;
        }
    }
}
