//! Thread-owned isolate/context and scoped execution primitives.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::diagnostics::{exception_text, NativeError};
use super::{bootstrap, bridge, diagnostics, modules, platform, services};

thread_local! {
    static HOST_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

// rusty_v8 enters an isolate on creation and requires LIFO disposal. The narrow
// prototype allows exactly one live host on each thread rather than exposing an
// API where ordinary non-LIFO Rust drops can violate that native precondition.
struct HostThreadGuard;
impl HostThreadGuard {
    fn acquire() -> Result<Self, NativeError> {
        if HOST_ACTIVE.with(|active| active.replace(true)) {
            Err(NativeError::Bootstrap(
                "a native host is already alive on this thread".into(),
            ))
        } else {
            Ok(Self)
        }
    }
}
impl Drop for HostThreadGuard {
    fn drop(&mut self) {
        HOST_ACTIVE.with(|active| active.set(false));
    }
}

pub struct NativeHost {
    // Rust drops fields in declaration order. Persistent roots must disappear
    // before OwnedIsolate, including on every early-return/error path.
    pub(super) context: v8::Global<v8::Context>,
    pub(super) modules: Rc<RefCell<modules::ModuleState>>,
    pub(super) services: Option<Rc<RefCell<services::Services>>>,
    pub(super) foreground: std::sync::Arc<platform::ForegroundQueue>,
    foreground_key: usize,
    pub(super) mode_nonce: String,
    pub(super) bundle_installed: bool,
    pub(super) last_install_error: Option<String>,
    pub(super) isolate: v8::OwnedIsolate,
    _thread_guard: HostThreadGuard,
}

impl NativeHost {
    pub fn new_config() -> Result<Self, NativeError> {
        Self::new_config_with_polyfills(crate::shims::WEB_POLYFILLS_SRC)
    }

    pub(super) fn new_config_with_polyfills(polyfills: &str) -> Result<Self, NativeError> {
        Self::new_inner(polyfills, None)
    }

    pub(super) fn new_inner(
        polyfills: &str,
        services: Option<services::Services>,
    ) -> Result<Self, NativeError> {
        let thread_guard = HostThreadGuard::acquire()?;
        bootstrap::initialize().map_err(NativeError::Bootstrap)?;
        let mut isolate = v8::Isolate::new(Default::default());
        let (foreground_key, foreground) = platform::ForegroundQueue::register(&isolate);
        let services = services.map(|state| Rc::new(RefCell::new(state)));
        if let Some(state) = &services {
            isolate.set_slot(state.clone());
        }
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
        isolate.set_capture_stack_trace_for_uncaught_exceptions(true, 10);
        isolate.add_message_listener(diagnostics::uncaught_message);
        isolate.set_promise_reject_callback(diagnostics::promise_rejected);
        isolate.set_host_import_module_dynamically_callback(modules::dynamic_import);
        isolate.set_host_initialize_import_meta_object_callback(modules::initialize_import_meta);
        let modules = Rc::new(RefCell::new(modules::ModuleState::default()));
        isolate.set_slot(modules.clone());
        let context = {
            v8::scope!(let scope, &mut isolate);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);
            bootstrap::install(scope).map_err(NativeError::Bootstrap)?;
            if services.is_some() {
                bridge::install(scope).map_err(NativeError::Bootstrap)?;
            }
            v8::Global::new(scope, context)
        };
        let mut host = Self {
            context,
            modules,
            services,
            foreground,
            foreground_key,
            mode_nonce: String::new(),
            bundle_installed: false,
            last_install_error: None,
            isolate,
            _thread_guard: thread_guard,
        };
        host.execute_script("zfb:config_eval_polyfills", polyfills)
            .map_err(|error| {
                NativeError::Evaluation(format!("config-eval polyfill init failed: {error}"))
            })?;
        Ok(host)
    }

    pub(super) fn execute_script(
        &mut self,
        name: &str,
        source: &str,
    ) -> Result<v8::Global<v8::Value>, NativeError> {
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let source = v8::String::new(tc, source)
            .ok_or_else(|| NativeError::Evaluation("V8 source allocation failed".into()))?;
        let origin = script_origin(tc, name, false)?;
        let result =
            v8::Script::compile(tc, source, Some(&origin)).and_then(|script| script.run(tc));
        match result {
            Some(result) => Ok(v8::Global::new(tc, result)),
            None => {
                let exception = tc.exception();
                Err(NativeError::Evaluation(exception_text(tc, exception)))
            }
        }
    }
}

impl Drop for NativeHost {
    fn drop(&mut self) {
        // Callbacks cannot outlive a synchronous V8 entry. Clear module roots
        // and remove the callback state's duplicate Rc while the isolate lives.
        self.foreground.close(self.foreground_key);
        self.isolate
            .remove_slot::<Rc<RefCell<services::Services>>>();
        if let Some(services) = &self.services {
            let mut state = services.borrow_mut();
            // Transports drop before persistent resolvers and before the isolate.
            for operation in state.pending.drain(..) {
                drop(operation.transport);
                drop(operation.resolver);
            }
            state.cancellations.clear();
        }
        self.isolate
            .remove_slot::<Rc<RefCell<modules::ModuleState>>>();
        let mut modules = self.modules.borrow_mut();
        modules.main = None;
        modules.worker = None;
        modules.dynamic_self_imports.clear();
        modules.unhandled_rejections.clear();
    }
}

pub(super) fn script_origin<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
    is_module: bool,
) -> Result<v8::ScriptOrigin<'s>, NativeError> {
    let name = v8::String::new(scope, name)
        .ok_or_else(|| NativeError::Evaluation("V8 resource-name allocation failed".into()))?;
    Ok(v8::ScriptOrigin::new(
        scope,
        name.into(),
        0,
        0,
        false,
        -1,
        None,
        false,
        false,
        is_module,
        None,
    ))
}
