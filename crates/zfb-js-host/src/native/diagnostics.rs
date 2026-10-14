//! Owned exception diagnostics at the native boundary.

#[derive(Debug, Clone, thiserror::Error)]
pub enum NativeError {
    #[error("native host bootstrap failed: {0}")]
    Bootstrap(String),
    #[error("default export must be a plain object")]
    NotAPlainObject,
    #[error("{0}")]
    BareImportRejected(String),
    #[error("config module load error: {0}")]
    ModuleLoad(String),
    #[error("config evaluation error: {0}")]
    Evaluation(String),
}

impl NativeError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::Bootstrap(_) => "bootstrap",
            Self::NotAPlainObject => "not_object",
            Self::BareImportRejected(_) => "bare_import",
            Self::ModuleLoad(_) => "module_load",
            Self::Evaluation(_) => "evaluation",
        }
    }
}

pub(super) fn exception_text(
    scope: &mut v8::PinScope,
    exception: Option<v8::Local<v8::Value>>,
) -> String {
    let Some(exception) = exception else {
        return "V8 returned no value or exception".into();
    };
    // V8's Error.stack includes authored source URLs/line/column. Capture it
    // before crossing into Rust rather than retaining exception handles.
    if let Ok(object) = v8::Local::<v8::Object>::try_from(exception) {
        if let Some(key) = v8::String::new(scope, "stack") {
            if let Some(stack) = object.get(scope, key.into()) {
                if stack.is_string() {
                    return stack.to_rust_string_lossy(scope);
                }
            }
        }
    }
    exception.to_rust_string_lossy(scope)
}

pub(super) fn throw_error(scope: &mut v8::PinScope, message: &str) {
    if let Some(message) = v8::String::new(scope, message) {
        let error = v8::Exception::error(scope, message);
        scope.throw_exception(error);
    }
}

/// Microtask exceptions are reported by V8's message listener instead of by
/// perform_microtask_checkpoint's return value. Preserve them in owned state.
pub(super) extern "C" fn uncaught_message(
    message: v8::Local<v8::Message>,
    _exception: v8::Local<v8::Value>,
) {
    use super::modules::ModuleState;
    use std::cell::RefCell;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::rc::Rc;
    // SAFETY: the message is supplied by V8 on this isolate's owner thread.
    v8::callback_scope!(unsafe scope, message);
    let state = scope.get_slot::<Rc<RefCell<ModuleState>>>().cloned();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let text = message.get(scope).to_rust_string_lossy(scope);
        let resource = message
            .get_script_resource_name(scope)
            .map(|name| name.to_rust_string_lossy(scope))
            .unwrap_or_default();
        format!(
            "{text}\n    at {resource}:{}:{}",
            message.get_line_number(scope).unwrap_or(0),
            message.get_start_column() + 1
        )
    }));
    if let Some(state) = state {
        if let Ok(mut state) = state.try_borrow_mut() {
            state
                .callback_failures
                .push(outcome.unwrap_or_else(|_| "native exception listener failed".into()));
        }
    }
}

/// Track rejection ownership until the checkpoint has given JS a chance to
/// install a handler. Root promise/reason values only inside this isolate.
pub(super) extern "C" fn promise_rejected(message: v8::PromiseRejectMessage) {
    use super::modules::ModuleState;
    use std::cell::RefCell;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::rc::Rc;
    // SAFETY: V8's PromiseRejectMessage belongs to the currently entered isolate.
    v8::callback_scope!(unsafe scope, &message);
    let state = scope.get_slot::<Rc<RefCell<ModuleState>>>().cloned();
    if let Some(state) = state {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let promise = message.get_promise();
            if let Ok(mut state) = state.try_borrow_mut() {
                match message.get_event() {
                    v8::PromiseRejectEvent::PromiseRejectWithNoHandler => {
                        if let Some(reason) = message.get_value() {
                            state.unhandled_rejections.push((
                                v8::Global::new(scope, promise),
                                v8::Global::new(scope, reason),
                            ));
                        }
                    }
                    v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject => {
                        state
                            .unhandled_rejections
                            .retain(|(root, _)| v8::Local::new(scope, root) != promise);
                    }
                    _ => {}
                }
            }
        }));
        if outcome.is_err() {
            if let Ok(mut state) = state.try_borrow_mut() {
                state
                    .callback_failures
                    .push("native promise rejection listener failed".into());
            }
        }
    }
}
