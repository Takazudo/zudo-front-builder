//! Narrow Worker/utility adapter. Public handles and responses own no V8 locals.
use super::diagnostics::exception_text;
use super::{services, values, worker_modules, NativeError, NativeHost};
use crate::dispatch::{HttpRequestLike, HttpResponseLike};
use crate::dispatch_mode::DispatchMode;
use crate::modules::BundleModuleLoader;

#[derive(Debug, Clone)]
pub struct ModuleHandle {
    specifier: String,
}

impl NativeHost {
    pub fn with_loader(loader: BundleModuleLoader) -> Result<Self, NativeError> {
        let host = Self::with_services(services::Services::defaults())?;
        host.modules.borrow_mut().worker = Some(worker_modules::WorkerModules::new(loader));
        Ok(host)
    }

    pub async fn execute_worker_module(
        &mut self,
        name: &str,
        source: &str,
    ) -> Result<ModuleHandle, NativeError> {
        let specifier = self.load_worker_module(name, source, true).await?;
        self.check_worker_shape(&specifier, name, true)?;
        self.last_install_error = None;
        Ok(ModuleHandle { specifier })
    }
    pub async fn execute_module(
        &mut self,
        name: &str,
        source: &str,
    ) -> Result<ModuleHandle, NativeError> {
        let specifier = self.load_worker_module(name, source, true).await?;
        match self.check_worker_shape(&specifier, name, true) {
            Ok(()) => self.last_install_error = None,
            Err(error) => self.last_install_error = Some(error.to_string()),
        }
        Ok(ModuleHandle { specifier })
    }
    pub async fn validate_worker_module_shape(
        &mut self,
        name: &str,
        source: &str,
    ) -> Result<(), NativeError> {
        let specifier = self.load_worker_module(name, source, false).await?;
        self.check_worker_shape(&specifier, name, false)
    }
    pub(super) fn reset_module_mode(&mut self) -> Result<(), NativeError> {
        if let Some(state) = &self.services {
            state.borrow_mut().mode = DispatchMode::BuildTime;
        }
        let nonce = serde_json::to_string(&self.mode_nonce)
            .map_err(|e| NativeError::Evaluation(e.to_string()))?;
        self.execute_script("zfb:reset_mode",&format!("globalThis.__zfb && typeof globalThis.__zfb.resetMode === 'function' && globalThis.__zfb.resetMode({nonce})"))?;
        Ok(())
    }
    pub(super) fn ready_module_imports(&mut self) -> bool {
        v8::scope!(let scope,&mut self.isolate);
        let state = self.modules.borrow();
        state.worker.as_ref().is_some_and(|worker| {
            worker.imports.iter().any(|import| {
                worker.entries[import.id]
                    .evaluation
                    .as_ref()
                    .map(|promise| {
                        v8::Local::new(scope, promise).state() != v8::PromiseState::Pending
                    })
                    .unwrap_or(true)
            })
        })
    }
    fn module_root(&self, specifier: &str) -> Result<v8::Global<v8::Module>, NativeError> {
        let state = self.modules.borrow();
        let worker = state
            .worker
            .as_ref()
            .ok_or_else(|| NativeError::ModuleLoad("Worker loader is not installed".into()))?;
        let id = worker.urls.get(specifier).ok_or_else(|| {
            NativeError::ModuleLoad(format!("no such module handle `{specifier}`"))
        })?;
        Ok(worker.entries[*id].module.clone())
    }
    fn check_worker_shape(
        &mut self,
        specifier: &str,
        name: &str,
        install: bool,
    ) -> Result<(), NativeError> {
        let root = self.module_root(specifier)?;
        v8::scope!(let scope,&mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc,scope);
        let module = v8::Local::new(tc, root);
        let namespace = v8::Local::<v8::Object>::try_from(module.get_module_namespace())
            .map_err(|_| NativeError::Evaluation("get_module_namespace failed".into()))?;
        let default = get(tc, namespace, "default")?;
        if default.is_undefined() {
            return Err(NativeError::Evaluation(format!("bundle `{name}` has no `default` export — expected workerd shape `export default {{ fetch }}`")));
        }
        if !default.is_object() {
            return Err(NativeError::Evaluation(format!("bundle `{name}` `default` export is not an object (workerd shape requires `export default {{ fetch }}`)")));
        }
        let object = v8::Local::<v8::Object>::try_from(default)
            .map_err(|_| NativeError::Evaluation("default export is not an object".into()))?;
        let fetch = get(tc, object, "fetch");
        if tc.has_caught() {
            let error = tc.exception();
            return Err(NativeError::Evaluation(exception_text(tc, error)));
        }
        let fetch = fetch?;
        if fetch.is_undefined() {
            return Err(NativeError::Evaluation(format!("bundle `{name}` `default` export has no `fetch` property (workerd shape requires `export default {{ fetch }}`)")));
        }
        if !fetch.is_function() {
            return Err(NativeError::Evaluation(format!("bundle `{name}` `default.fetch` is not callable (workerd shape requires `export default {{ fetch }}`)")));
        }
        if install {
            let result = call_zfb(tc, "setBundle", &[default]);
            if tc.has_caught() {
                let error = tc.exception();
                return Err(NativeError::Evaluation(exception_text(tc, error)));
            }
            result?;
            self.bundle_installed = true;
        }
        Ok(())
    }

    pub async fn dispatch_fetch(
        &mut self,
        request: HttpRequestLike,
    ) -> Result<HttpResponseLike, NativeError> {
        if !self.bundle_installed {
            let mut error="embedded V8 host: dispatch_fetch called before any bundle was loaded (call execute_module() first)".to_string();
            if let Some(previous) = &self.last_install_error {
                error.push_str(&format!("; last install error: {previous}"));
            }
            return Err(NativeError::Evaluation(error));
        }
        self.services
            .as_ref()
            .ok_or_else(|| NativeError::Evaluation("native services are not installed".into()))?
            .borrow_mut()
            .begin_dispatch(request.mode);
        let promise = {
            v8::scope!(let scope,&mut self.isolate);
            let context = v8::Local::new(scope, &self.context);
            let scope = &mut v8::ContextScope::new(scope, context);
            v8::tc_scope!(let tc,scope);
            let url = string(tc, &request.url)?;
            let method = string(tc, &request.method)?;
            let headers = json_input(tc, &serde_json::json!(request.headers))?;
            let body: v8::Local<v8::Value> = match request.body {
                Some(bytes) if !bytes.is_empty() => values::bytes(tc, bytes)
                    .ok_or_else(|| NativeError::Evaluation("body allocation failed".into()))?
                    .into(),
                _ => v8::undefined(tc).into(),
            };
            let mode = string(tc, request.mode.as_js_str())?;
            let nonce = string(tc, &self.mode_nonce)?;
            let result = call_zfb(
                tc,
                "dispatch",
                &[
                    url.into(),
                    method.into(),
                    headers,
                    body,
                    mode.into(),
                    nonce.into(),
                ],
            );
            if tc.has_caught() {
                let error = tc.exception();
                return Err(NativeError::Evaluation(exception_text(tc, error)));
            }
            let result = result?;
            let promise = v8::Local::<v8::Promise>::try_from(result)
                .map_err(|_| NativeError::Evaluation("dispatch did not return a promise".into()))?;
            promise.mark_as_handled();
            v8::Global::new(tc, promise)
        };
        let result = self.drive(promise).await?;
        v8::scope!(let scope,&mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc,scope);
        let result = v8::Local::new(tc, result);
        let object = v8::Local::<v8::Object>::try_from(result)
            .map_err(|_| NativeError::Evaluation("dispatch result is not an object".into()))?;
        let status = get(tc, object, "status")?
            .uint32_value(tc)
            .filter(|v| *v <= u16::MAX as u32)
            .ok_or_else(|| NativeError::Evaluation("invalid response status".into()))?
            as u16;
        let header_value = get(tc, object, "headers")?;
        let header_json = values::json_value(tc, header_value)?;
        let headers: Vec<(String, String)> = serde_json::from_value(header_json)
            .map_err(|e| NativeError::Evaluation(format!("invalid response headers: {e}")))?;
        let body = values::copy_view(get(tc, object, "body")?, i32::MAX as usize)
            .map_err(NativeError::Evaluation)?;
        Ok(HttpResponseLike {
            status,
            headers: headers
                .into_iter()
                .map(|(k, v)| (k.to_lowercase(), v))
                .collect(),
            body,
        })
    }
    pub fn drain_console_logs(&mut self) -> String {
        match self.execute_script(
            "zfb:drain_console_logs",
            "globalThis.__zfb.drainConsoleLogs()",
        ) {
            Ok(value) => {
                v8::scope!(let scope,&mut self.isolate);
                let context = v8::Local::new(scope, &self.context);
                let scope = &mut v8::ContextScope::new(scope, context);
                v8::Local::new(scope, value).to_rust_string_lossy(scope)
            }
            Err(_) => String::new(),
        }
    }
    pub async fn get_export(
        &mut self,
        handle: &ModuleHandle,
        name: &str,
    ) -> Result<serde_json::Value, NativeError> {
        let root = self.module_root(&handle.specifier)?;
        v8::scope!(let scope,&mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc,scope);
        let module = v8::Local::new(tc, root);
        let namespace = v8::Local::<v8::Object>::try_from(module.get_module_namespace())
            .map_err(|_| NativeError::Evaluation("get_module_namespace failed".into()))?;
        let value = get(tc, namespace, name)?;
        values::json_value(tc, value)
    }
    pub async fn call_default(
        &mut self,
        handle: &ModuleHandle,
        props: serde_json::Value,
    ) -> Result<String, NativeError> {
        let root = self.module_root(&handle.specifier)?;
        let value = {
            v8::scope!(let scope,&mut self.isolate);
            let context = v8::Local::new(scope, &self.context);
            let scope = &mut v8::ContextScope::new(scope, context);
            v8::tc_scope!(let tc,scope);
            let module = v8::Local::new(tc, root);
            let namespace = v8::Local::<v8::Object>::try_from(module.get_module_namespace())
                .map_err(|_| NativeError::Evaluation("get_module_namespace failed".into()))?;
            let function = v8::Local::<v8::Function>::try_from(get(tc, namespace, "default")?)
                .map_err(|_| {
                    NativeError::Evaluation(format!(
                        "missing callable default export: {}",
                        handle.specifier
                    ))
                })?;
            let props = json_input(tc, &props)?;
            let result = function.call(tc, namespace.into(), &[props]);
            let Some(result) = result else {
                let error = tc.exception();
                return Err(NativeError::Evaluation(exception_text(tc, error)));
            };
            v8::Global::new(tc, result)
        };
        let promise = {
            v8::scope!(let scope,&mut self.isolate);
            let value = v8::Local::new(scope, &value);
            v8::Local::<v8::Promise>::try_from(value).ok().map(|p| {
                p.mark_as_handled();
                v8::Global::new(scope, p)
            })
        };
        let value = match promise {
            Some(promise) => self.drive(promise).await?,
            None => value,
        };
        v8::scope!(let scope,&mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc,scope);
        let value = v8::Local::new(tc, value);
        let result = value.to_string(tc);
        match result {
            Some(result) => Ok(result.to_rust_string_lossy(tc)),
            None => {
                let error = tc.exception();
                Err(NativeError::Evaluation(exception_text(tc, error)))
            }
        }
    }
}
fn string<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: &str,
) -> Result<v8::Local<'s, v8::String>, NativeError> {
    v8::String::new(scope, value)
        .ok_or_else(|| NativeError::Evaluation("V8 string allocation failed".into()))
}
fn get<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<v8::Object>,
    key: &str,
) -> Result<v8::Local<'s, v8::Value>, NativeError> {
    let key = string(scope, key)?;
    object
        .get(scope, key.into())
        .ok_or_else(|| NativeError::Evaluation("V8 property read failed".into()))
}
fn json_input<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: &serde_json::Value,
) -> Result<v8::Local<'s, v8::Value>, NativeError> {
    let source = string(scope, &value.to_string())?;
    v8::json::parse(scope, source)
        .ok_or_else(|| NativeError::Evaluation("V8 JSON argument conversion failed".into()))
}
fn call_zfb<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
    args: &[v8::Local<v8::Value>],
) -> Result<v8::Local<'s, v8::Value>, NativeError> {
    let global = scope.get_current_context().global(scope);
    let zfb = v8::Local::<v8::Object>::try_from(get(scope, global, "__zfb")?)
        .map_err(|_| NativeError::Evaluation("__zfb is not an object".into()))?;
    let function = v8::Local::<v8::Function>::try_from(get(scope, zfb, name)?)
        .map_err(|_| NativeError::Evaluation(format!("__zfb.{name} is not a function")))?;
    function
        .call(scope, zfb.into(), args)
        .ok_or_else(|| NativeError::Evaluation(format!("__zfb.{name} failed")))
}
