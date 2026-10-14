//! Config-only ESM loading; the shared authorized Worker loader follows later.
use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

use super::core::script_origin;
use super::diagnostics::{exception_text, throw_error, NativeError};
use super::NativeHost;

pub(super) const MAIN_SPECIFIER: &str = "file:///zfb-config-eval/main.mjs";
pub(super) const NO_BARE_IMPORTS_ERROR: &str =
    "zfb.config.ts: bare imports and node:* specifiers are not available inside \
    the config evaluator. zfb.config.ts is a data config, not a general Node \
    script — move runtime imports to your application code.";

#[derive(Default)]
pub(super) struct ModuleState {
    pub main: Option<v8::Global<v8::Module>>,
    pub main_evaluation_complete: bool,
    pub callback_failures: Vec<String>,
    pub unhandled_rejections: Vec<(v8::Global<v8::Promise>, v8::Global<v8::Value>)>,
    pub dynamic_self_imports: Vec<v8::Global<v8::PromiseResolver>>,
}

impl NativeHost {
    pub(super) fn evaluate_module(
        &mut self,
        source: &str,
    ) -> Result<v8::Global<v8::Module>, NativeError> {
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let code = v8::String::new(tc, source)
            .ok_or_else(|| NativeError::ModuleLoad("V8 source allocation failed".into()))?;
        let origin = script_origin(tc, MAIN_SPECIFIER, true)?;
        let mut source = v8::script_compiler::Source::new(code, Some(&origin));
        let Some(module) = v8::script_compiler::compile_module(tc, &mut source) else {
            let exception = tc.exception();
            return Err(NativeError::ModuleLoad(exception_text(tc, exception)));
        };
        self.modules.borrow_mut().main = Some(v8::Global::new(tc, module));
        if module.instantiate_module(tc, resolve_config_import) != Some(true) {
            let exception = tc.exception();
            let message = exception_text(tc, exception);
            return Err(if message.contains(NO_BARE_IMPORTS_ERROR) {
                NativeError::BareImportRejected(message)
            } else {
                NativeError::ModuleLoad(message)
            });
        }
        let Some(value) = module.evaluate(tc) else {
            let exception = tc.exception();
            return Err(NativeError::Evaluation(exception_text(tc, exception)));
        };
        let promise = v8::Local::<v8::Promise>::try_from(value)
            .map_err(|_| NativeError::Evaluation("module evaluation returned no promise".into()))?;
        // Explicit checkpoints drain finite chains completely; no socket work
        // exists in this config core. Never busy-poll an unresolved promise.
        tc.perform_microtask_checkpoint();
        match promise.state() {
            v8::PromiseState::Rejected => {
                let exception = promise.result(tc);
                return Err(NativeError::Evaluation(exception_text(tc, Some(exception))));
            }
            v8::PromiseState::Pending => {
                return Err(NativeError::Evaluation(
                    "module evaluation promise is unresolved with no pending native work (direct-V8 prototype limitation)".into(),
                ));
            }
            v8::PromiseState::Fulfilled => {}
        }
        // A dynamic import of the exact main URL completes only once that
        // module's evaluation has fulfilled. An awaited cyclic self-import
        // stays unresolved and takes the explicit limitation above.
        let resolvers = {
            let mut state = self.modules.borrow_mut();
            state.main_evaluation_complete = true;
            std::mem::take(&mut state.dynamic_self_imports)
        };
        for resolver in resolvers {
            let resolver = v8::Local::new(tc, resolver);
            let namespace = module.get_module_namespace();
            resolver.resolve(tc, namespace);
        }
        tc.perform_microtask_checkpoint();
        let (failure, rejection) = {
            let mut state = self.modules.borrow_mut();
            (
                state.callback_failures.pop(),
                state.unhandled_rejections.pop(),
            )
        };
        if let Some(failure) = failure {
            return Err(NativeError::Evaluation(failure));
        }
        if let Some((_promise, reason)) = rejection {
            let reason = v8::Local::new(tc, reason);
            return Err(NativeError::Evaluation(exception_text(tc, Some(reason))));
        }
        Ok(v8::Global::new(tc, module))
    }
}

fn resolve_config_import<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _attributes: v8::Local<'s, v8::FixedArray>,
    _referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    // SAFETY: V8 invokes the callback with the currently entered context.
    v8::callback_scope!(unsafe scope, context);
    match catch_unwind(AssertUnwindSafe(|| {
        if specifier.to_rust_string_lossy(scope) == MAIN_SPECIFIER {
            let state = scope.get_slot::<Rc<RefCell<ModuleState>>>()?.clone();
            let state = state.try_borrow().ok()?;
            return state
                .main
                .as_ref()
                .map(|module| v8::Local::new(scope, module));
        }
        throw_error(scope, NO_BARE_IMPORTS_ERROR);
        None
    })) {
        Ok(result) => result,
        Err(_) => {
            throw_error(scope, "native config import callback failed");
            None
        }
    }
}

pub(super) fn dynamic_import<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _options: v8::Local<'s, v8::Data>,
    _resource: v8::Local<'s, v8::Value>,
    specifier: v8::Local<'s, v8::String>,
    _attributes: v8::Local<'s, v8::FixedArray>,
) -> Option<v8::Local<'s, v8::Promise>> {
    match catch_unwind(AssertUnwindSafe(|| {
        let resolver = v8::PromiseResolver::new(scope)?;
        let promise = resolver.get_promise(scope);
        if specifier.to_rust_string_lossy(scope) == MAIN_SPECIFIER {
            let state = scope.get_slot::<Rc<RefCell<ModuleState>>>()?.clone();
            let completed_module = {
                let mut state = state.try_borrow_mut().ok()?;
                if state.main_evaluation_complete {
                    state
                        .main
                        .as_ref()
                        .map(|module| v8::Local::new(scope, module))
                } else {
                    state
                        .dynamic_self_imports
                        .push(v8::Global::new(scope, resolver));
                    None
                }
            };
            if let Some(module) = completed_module {
                resolver.resolve(scope, module.get_module_namespace());
            }
        } else {
            let message = v8::String::new(scope, NO_BARE_IMPORTS_ERROR)?;
            let exception = v8::Exception::type_error(scope, message);
            resolver.reject(scope, exception);
        }
        Some(promise)
    })) {
        Ok(result) => result,
        Err(_) => {
            throw_error(scope, "native dynamic import callback failed");
            None
        }
    }
}

pub(super) extern "C" fn initialize_import_meta(
    context: v8::Local<v8::Context>,
    _module: v8::Local<v8::Module>,
    meta: v8::Local<v8::Object>,
) {
    // SAFETY: V8 supplies the currently entered context and module metadata.
    v8::callback_scope!(unsafe scope, context);
    if catch_unwind(AssertUnwindSafe(|| {
        for (key, value) in [
            ("url", MAIN_SPECIFIER),
            ("filename", "/zfb-config-eval/main.mjs"),
            ("dirname", "/zfb-config-eval"),
        ] {
            if let (Some(key), Some(value)) =
                (v8::String::new(scope, key), v8::String::new(scope, value))
            {
                meta.create_data_property(scope, key.into(), value.into());
            }
        }
        if let Some(key) = v8::String::new(scope, "main") {
            let value = v8::Boolean::new(scope, true);
            meta.create_data_property(scope, key.into(), value.into());
        }
        if let (Some(key), Some(resolve)) = (
            v8::String::new(scope, "resolve"),
            v8::Function::new(scope, import_meta_resolve),
        ) {
            meta.create_data_property(scope, key.into(), resolve.into());
        }
    }))
    .is_err()
    {
        throw_error(scope, "native import.meta callback failed");
    }
}

fn import_meta_resolve(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    if catch_unwind(AssertUnwindSafe(|| {
        if args.length() > 1 {
            throw_invalid_arguments(scope);
            return;
        }
        let Some(specifier) = args.get(0).to_string(scope) else {
            throw_invalid_arguments(scope);
            return;
        };
        if specifier.to_rust_string_lossy(scope) == MAIN_SPECIFIER {
            rv.set(specifier.into());
        } else {
            throw_error(scope, NO_BARE_IMPORTS_ERROR);
        }
    }))
    .is_err()
    {
        throw_error(scope, "native import.meta.resolve callback failed");
    }
}

fn throw_invalid_arguments(scope: &mut v8::PinScope) {
    if let Some(message) = v8::String::new(scope, "Invalid arguments") {
        let error = v8::Exception::type_error(scope, message);
        scope.throw_exception(error);
    }
}
