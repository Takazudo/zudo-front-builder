//! Raw configuration evaluation, deliberately distinct from outer typed validation.
use serde_json::Value;

use super::diagnostics::{exception_text, NativeError};
use super::NativeHost;

pub async fn evaluate_config_bundle(source: &str) -> Result<Value, NativeError> {
    let mut host = NativeHost::new_config()?;
    host.evaluate_config(source).await
}

impl NativeHost {
    /// A config host evaluates one main module. Create a new host for the next
    /// independent config so globals and module identity cannot leak between inputs.
    pub async fn evaluate_config(&mut self, source: &str) -> Result<Value, NativeError> {
        if self.modules.borrow().main.is_some() {
            return Err(NativeError::Evaluation(
                "config host already evaluated its main module; create a fresh host".into(),
            ));
        }
        let module = self.evaluate_module(source).await?;
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let tc, scope);
        let module = v8::Local::new(tc, module);
        let namespace = module.get_module_namespace();
        let namespace = v8::Local::<v8::Object>::try_from(namespace)
            .map_err(|_| NativeError::Evaluation("get_module_namespace failed".into()))?;
        let key = v8::String::new(tc, "default")
            .ok_or_else(|| NativeError::Evaluation("v8 string alloc failed".into()))?;
        let default = namespace
            .get(tc, key.into())
            .ok_or(NativeError::NotAPlainObject)?;
        // Preserve the baseline's raw guard: arrays, Dates and boxed primitives
        // are objects. The outer loader validates its own typed schema later.
        if !default.is_object() || default.is_function() {
            return Err(NativeError::NotAPlainObject);
        }
        let global = context.global(tc);
        let key = v8::String::new(tc, "JSON")
            .ok_or_else(|| NativeError::Evaluation("v8 string alloc failed".into()))?;
        let json = global
            .get(tc, key.into())
            .ok_or_else(|| NativeError::Evaluation("JSON not on globalThis".into()))?;
        let json = v8::Local::<v8::Object>::try_from(json)
            .map_err(|_| NativeError::Evaluation("JSON is not an object".into()))?;
        let key = v8::String::new(tc, "stringify")
            .ok_or_else(|| NativeError::Evaluation("v8 string alloc failed".into()))?;
        let stringify = json
            .get(tc, key.into())
            .ok_or_else(|| NativeError::Evaluation("JSON.stringify not found".into()))?;
        let stringify = v8::Local::<v8::Function>::try_from(stringify)
            .map_err(|_| NativeError::Evaluation("JSON.stringify is not a function".into()))?;
        let result = stringify.call(tc, json.into(), &[default]);
        if tc.has_caught() {
            let exception = tc.exception();
            return Err(NativeError::Evaluation(format!(
                "JSON.stringify threw: {}",
                exception_text(tc, exception)
            )));
        }
        let result = result
            .ok_or_else(|| NativeError::Evaluation("JSON.stringify returned no value".into()))?;
        if !result.is_string() {
            return Err(NativeError::Evaluation(
                "JSON.stringify returned a non-string value (unexpected)".into(),
            ));
        }
        let json = result.to_rust_string_lossy(tc);
        serde_json::from_str(&json).map_err(|error| {
            NativeError::Evaluation(format!(
                "serde_json parsing of stringify result failed: {error}"
            ))
        })
    }
}
