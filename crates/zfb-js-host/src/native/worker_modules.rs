//! V8 module identity and evaluation; all source authority stays in BundleModuleLoader.
use super::{
    core::script_origin,
    diagnostics::{exception_text, throw_error, NativeError},
    modules::ModuleState,
};
use crate::modules::BundleModuleLoader;
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

pub(super) struct Entry {
    pub url: String,
    pub main: bool,
    pub module: v8::Global<v8::Module>,
    pub evaluation: Option<v8::Global<v8::Promise>>,
}
pub(super) struct Import {
    pub id: usize,
    pub resolver: v8::Global<v8::PromiseResolver>,
}
pub(super) struct WorkerModules {
    pub loader: Rc<BundleModuleLoader>,
    pub entries: Vec<Entry>,
    pub urls: HashMap<String, usize>,
    pub imports: Vec<Import>,
}
impl WorkerModules {
    pub fn new(loader: BundleModuleLoader) -> Self {
        Self {
            loader: Rc::new(loader),
            entries: Vec::new(),
            urls: HashMap::new(),
            imports: Vec::new(),
        }
    }
}
fn state(scope: &v8::PinScope) -> Option<Rc<RefCell<ModuleState>>> {
    scope.get_slot::<Rc<RefCell<ModuleState>>>().cloned()
}
pub(super) fn enabled(scope: &v8::PinScope) -> bool {
    state(scope).is_some_and(|s| s.borrow().worker.is_some())
}

pub(super) fn load<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    url: &str,
    main: bool,
) -> Option<(usize, v8::Local<'s, v8::Module>)> {
    let state = state(scope)?;
    let loader = {
        let state = state.borrow();
        let worker = state.worker.as_ref()?;
        if let Some(id) = worker.urls.get(url) {
            return Some((*id, v8::Local::new(scope, &worker.entries[*id].module)));
        }
        worker.loader.clone()
    };
    let url = match url::Url::parse(url) {
        Ok(url) => url,
        Err(error) => {
            throw_error(scope, &error.to_string());
            return None;
        }
    };
    let source = match loader.load(&url) {
        Ok(source) => source,
        Err(error) => {
            throw_error(scope, &error);
            return None;
        }
    };
    let found = source.found_url.to_string();
    {
        let mut state = state.borrow_mut();
        let worker = state.worker.as_mut()?;
        if let Some(id) = worker.urls.get(&found).copied() {
            worker.urls.insert(url.to_string(), id);
            return Some((id, v8::Local::new(scope, &worker.entries[id].module)));
        }
    }
    let code = v8::String::new(scope, &source.source)?;
    let origin = match script_origin(scope, &found, true) {
        Ok(origin) => origin,
        Err(error) => {
            throw_error(scope, &error.to_string());
            return None;
        }
    };
    let mut source = v8::script_compiler::Source::new(code, Some(&origin));
    let module = v8::script_compiler::compile_module(scope, &mut source)?;
    let root = v8::Global::new(scope, module);
    let id = {
        let mut state = state.borrow_mut();
        let worker = state.worker.as_mut()?;
        let id = worker.entries.len();
        worker.urls.insert(url.to_string(), id);
        worker.urls.insert(found.clone(), id);
        worker.entries.push(Entry {
            url: found,
            main,
            module: root,
            evaluation: None,
        });
        id
    };
    Some((id, module))
}

fn resolve<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _attributes: v8::Local<'s, v8::FixedArray>,
    referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    // SAFETY: V8 enters this callback on the owner thread with the current context.
    v8::callback_scope!(unsafe scope,context);
    match catch_unwind(AssertUnwindSafe(|| {
        let state = state(scope)?;
        let (loader, referrer) = {
            let state = state.borrow();
            let worker = state.worker.as_ref()?;
            let entry = worker
                .entries
                .iter()
                .find(|entry| v8::Local::new(scope, &entry.module) == referrer)?;
            (worker.loader.clone(), entry.url.clone())
        };
        let requested = specifier.to_rust_string_lossy(scope);
        if !allow_internal_import(scope, &requested, &referrer) {
            return None;
        }
        let url = match loader.resolve(&requested, &referrer) {
            Ok(url) => url,
            Err(error) => {
                throw_error(scope, &error);
                return None;
            }
        };
        load(scope, url.as_str(), false).map(|(_, module)| module)
    })) {
        Ok(module) => module,
        Err(_) => {
            throw_error(scope, "native module resolver failed");
            None
        }
    }
}

pub(super) fn evaluate<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    id: usize,
) -> Option<v8::Local<'s, v8::Promise>> {
    let state = state(scope)?;
    let module = {
        let state = state.borrow();
        let worker = state.worker.as_ref()?;
        let entry = worker.entries.get(id)?;
        if let Some(promise) = &entry.evaluation {
            return Some(v8::Local::new(scope, promise));
        }
        v8::Local::new(scope, &entry.module)
    };
    if module.get_status() == v8::ModuleStatus::Uninstantiated
        && module.instantiate_module(scope, resolve) != Some(true)
    {
        return None;
    }
    if module.get_status() == v8::ModuleStatus::Errored {
        scope.throw_exception(module.get_exception());
        return None;
    }
    // Called after the synchronous evaluate entry has unwound, never recursively
    // while V8 is still instantiating/evaluating this graph.
    if matches!(
        module.get_status(),
        v8::ModuleStatus::Instantiating | v8::ModuleStatus::Evaluating
    ) {
        return None;
    }
    let value = module.evaluate(scope)?;
    let promise = v8::Local::<v8::Promise>::try_from(value).ok()?;
    promise.mark_as_handled();
    let root = v8::Global::new(scope, promise);
    state.borrow_mut().worker.as_mut()?.entries[id].evaluation = Some(root);
    Some(promise)
}

pub(super) fn dynamic<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    resource: v8::Local<'s, v8::Value>,
    specifier: v8::Local<'s, v8::String>,
) -> Option<v8::Local<'s, v8::Promise>> {
    let resolver = v8::PromiseResolver::new(scope)?;
    let promise = resolver.get_promise(scope);
    // Isolate exceptions are converted to a rejected import promise here.
    v8::tc_scope!(let tc,scope);
    let outcome = (|| {
        let state = state(tc)?;
        let loader = state.borrow().worker.as_ref()?.loader.clone();
        let requested = specifier.to_rust_string_lossy(tc);
        let referrer = resource.to_rust_string_lossy(tc);
        if !allow_internal_import(tc, &requested, &referrer) {
            return None;
        }
        let url = match loader.resolve(&requested, &referrer) {
            Ok(url) => url,
            Err(error) => {
                throw_error(tc, &error);
                return None;
            }
        };
        let (id, _) = load(tc, url.as_str(), false)?;
        let resolver = v8::Global::new(tc, resolver);
        state
            .borrow_mut()
            .worker
            .as_mut()?
            .imports
            .push(Import { id, resolver });
        Some(())
    })();
    if outcome.is_none() {
        let error = tc.exception().unwrap_or_else(|| {
            let s = v8::String::new(tc, "native dynamic import failed").unwrap();
            v8::Exception::error(tc, s)
        });
        tc.reset();
        resolver.reject(tc, error);
    }
    Some(promise)
}

/// One finite pass. Settlement runs with no ModuleState borrow, since namespace
/// `then` exports and promise hooks may recursively import or call the bridge.
pub(super) fn poll_imports(scope: &mut v8::PinScope) -> bool {
    let Some(state) = state(scope) else {
        return false;
    };
    let imports = {
        let mut state = state.borrow_mut();
        let Some(worker) = state.worker.as_mut() else {
            return false;
        };
        std::mem::take(&mut worker.imports)
    };
    let mut pending = Vec::new();
    let mut progress = false;
    for import in imports {
        v8::tc_scope!(let tc,scope);
        let evaluation = evaluate(tc, import.id);
        let resolver = v8::Local::new(tc, &import.resolver);
        match evaluation {
            Some(promise) => match promise.state() {
                v8::PromiseState::Pending => pending.push(import),
                v8::PromiseState::Fulfilled => {
                    let module = {
                        let state = state.borrow();
                        v8::Local::new(
                            tc,
                            &state.worker.as_ref().unwrap().entries[import.id].module,
                        )
                    };
                    resolver.resolve(tc, module.get_module_namespace());
                    progress = true;
                }
                v8::PromiseState::Rejected => {
                    let error = promise.result(tc);
                    resolver.reject(tc, error);
                    progress = true;
                }
            },
            None if tc.has_caught() => {
                let error = tc.exception().unwrap();
                tc.reset();
                resolver.reject(tc, error);
                progress = true;
            }
            None => pending.push(import),
        }
    }
    state
        .borrow_mut()
        .worker
        .as_mut()
        .unwrap()
        .imports
        .extend(pending);
    progress
}

pub(super) fn meta(
    scope: &mut v8::PinScope,
    module: v8::Local<v8::Module>,
    meta: v8::Local<v8::Object>,
) {
    let Some(state) = state(scope) else {
        return;
    };
    let info = {
        let state = state.borrow();
        let worker = state.worker.as_ref().unwrap();
        worker
            .entries
            .iter()
            .find(|entry| v8::Local::new(scope, &entry.module) == module)
            .map(|entry| (entry.url.clone(), entry.main))
    };
    let Some((url, main)) = info else {
        return;
    };
    let Some(value) = v8::String::new(scope, &url) else {
        return;
    };
    property(scope, meta, "url", value.into());
    let main = v8::Boolean::new(scope, main);
    property(scope, meta, "main", main.into());
    if let Some(resolve) = v8::Function::builder(import_meta_resolve)
        .data(value.into())
        .build(scope)
    {
        property(scope, meta, "resolve", resolve.into());
    }
    if let Ok(path) = url::Url::parse(&url)
        .ok()
        .filter(|u| u.scheme() == "file")
        .ok_or(())
        .and_then(|u| u.to_file_path().map_err(|_| ()))
    {
        for (name, path) in [
            ("filename", path.clone()),
            (
                "dirname",
                path.parent()
                    .unwrap_or(std::path::Path::new("/"))
                    .to_path_buf(),
            ),
        ] {
            if let Some(value) = v8::String::new(scope, &path.display().to_string()) {
                property(scope, meta, name, value.into());
            }
        }
    }
}
fn property(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
    name: &str,
    value: v8::Local<v8::Value>,
) {
    if let Some(key) = v8::String::new(scope, name) {
        object.create_data_property(scope, key.into(), value);
    }
}
fn import_meta_resolve(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    if catch_unwind(AssertUnwindSafe(|| {
        if args.length() > 1 {
            super::values::throw_named(scope, "TypeError", "Invalid arguments");
            return;
        }
        let Some(specifier) = args.get(0).to_string(scope) else {
            super::values::throw_named(scope, "TypeError", "Invalid arguments");
            return;
        };
        let Some(state) = state(scope) else {
            return;
        };
        let loader = state.borrow().worker.as_ref().unwrap().loader.clone();
        match loader.resolve(
            &specifier.to_rust_string_lossy(scope),
            &args.data().to_rust_string_lossy(scope),
        ) {
            Ok(url) => {
                if let Some(url) = v8::String::new(scope, url.as_str()) {
                    rv.set(url.into());
                }
            }
            Err(error) => throw_error(scope, &error),
        }
    }))
    .is_err()
    {
        throw_error(scope, "native import.meta.resolve callback failed");
    }
}

impl super::NativeHost {
    pub(super) async fn load_worker_module(
        &mut self,
        name: &str,
        source: &str,
        main: bool,
    ) -> Result<String, NativeError> {
        self.reset_module_mode()?;
        let url = synthesise_specifier(name);
        {
            let state = self.modules.borrow();
            let worker = state
                .worker
                .as_ref()
                .ok_or_else(|| NativeError::ModuleLoad("Worker loader is not installed".into()))?;
            worker.loader.register_module(&url, source);
        }
        let promise = {
            v8::scope!(let scope,&mut self.isolate);
            let context = v8::Local::new(scope, &self.context);
            let scope = &mut v8::ContextScope::new(scope, context);
            v8::tc_scope!(let tc,scope);
            let Some((id, _)) = load(tc, &url, main) else {
                let error = tc.exception();
                return Err(NativeError::ModuleLoad(exception_text(tc, error)));
            };
            let Some(promise) = evaluate(tc, id) else {
                let error = tc.exception();
                return Err(NativeError::Evaluation(exception_text(tc, error)));
            };
            v8::Global::new(tc, promise)
        };
        self.drive(promise).await?;
        // Module boot drains imported module evaluations, unlike request
        // dispatch, which may legitimately leave unrelated HTTP work orphaned.
        loop {
            let promise = {
                v8::scope!(let scope, &mut self.isolate);
                let context = v8::Local::new(scope, &self.context);
                let scope = &mut v8::ContextScope::new(scope, context);
                let state = self.modules.borrow();
                state
                    .worker
                    .as_ref()
                    .and_then(|worker| worker.imports.first())
                    .map(|import| {
                        let resolver = v8::Local::new(scope, &import.resolver);
                        v8::Global::new(scope, resolver.get_promise(scope))
                    })
            };
            let Some(promise) = promise else {
                break;
            };
            self.drive(promise).await?;
        }
        Ok(url)
    }
}

pub(super) fn synthesise_specifier(name: &str) -> String {
    if name.starts_with("file://") {
        return name.to_string();
    }
    let mut out = String::from("file:///zfb/");
    for byte in name.trim_start_matches('/').bytes() {
        match byte {
            b'a'..=b'z'
            | b'A'..=b'Z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b'['
            | b']' => out.push(byte as char),
            byte => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// Deno's existing engine boundary reserves ext: helpers to node:/ext: modules.
// This is independent of source authorization in the shared loader.
fn allow_internal_import(scope: &mut v8::PinScope, specifier: &str, referrer: &str) -> bool {
    if specifier.starts_with("ext:")
        && !referrer.starts_with("ext:")
        && !referrer.starts_with("node:")
    {
        super::values::throw_named(scope,"TypeError",&format!("Importing ext: modules is only allowed from ext: and node: modules. Tried to import {specifier} from {referrer}"));
        false
    } else {
        true
    }
}
