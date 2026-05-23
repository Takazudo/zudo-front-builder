//! Core `evaluate_config_bundle` function and the rejecting module loader
//! used by the config-eval isolate.
//!
//! The module loader accepts ONLY the main-module specifier — every other
//! import is rejected with the no-Node-globals error message. This enforces
//! the product decision that `zfb.config.ts` is a data config, not a general
//! Node script.

use std::rc::Rc;

use deno_core::{
    v8, JsRuntime, ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader,
    ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, PollEventLoopOptions,
    ResolutionKind, RuntimeOptions,
    error::ModuleLoaderError,
};

use super::{ConfigEvalError, NO_BARE_IMPORTS_ERROR};

/// Synthetic specifier used as the main ESM module URL in the config evaluator.
/// A stable synthetic URL so V8 stack frames are readable.
pub(super) const MAIN_SPECIFIER: &str = "file:///zfb-config-eval/main.mjs";

/// Evaluate a pre-bundled JS config module and return its `default` export
/// as a `serde_json::Value`.
///
/// The `js_source` must be a self-contained ESM module (no bare imports,
/// no `node:*` imports). The returned `serde_json::Value` reflects the
/// `JSON.stringify` semantics: functions and symbols are silently dropped,
/// `Date` objects are serialised as ISO strings.
///
/// # Errors
///
/// Returns `ConfigEvalError` if:
/// - V8 initialisation fails
/// - The module source throws at top level
/// - The `default` export is not a plain object
/// - The source contains non-main-module imports
pub async fn evaluate_config_bundle(
    js_source: &str,
) -> Result<serde_json::Value, ConfigEvalError> {
    let main_spec =
        ModuleSpecifier::parse(MAIN_SPECIFIER).expect("MAIN_SPECIFIER is a valid URL; qed");
    let loader = Rc::new(ConfigModuleLoader::new(MAIN_SPECIFIER, js_source));

    let runtime = JsRuntime::new(RuntimeOptions {
        module_loader: Some(loader),
        ..Default::default()
    });

    evaluate_in_runtime(runtime, main_spec).await
}

async fn evaluate_in_runtime(
    mut runtime: JsRuntime,
    main_spec: ModuleSpecifier,
) -> Result<serde_json::Value, ConfigEvalError> {
    // Load + evaluate the main module.
    let module_id = runtime
        .load_main_es_module(&main_spec)
        .await
        .map_err(|e| ConfigEvalError::ModuleLoad(e.to_string()))?;

    let evaluate = runtime.mod_evaluate(module_id);
    runtime
        .run_event_loop(PollEventLoopOptions::default())
        .await
        .map_err(|e| ConfigEvalError::Evaluation(e.to_string()))?;
    evaluate
        .await
        .map_err(|e| ConfigEvalError::Evaluation(e.to_string()))?;

    // Read the `default` export and JSON.stringify it inside the isolate.
    let namespace = runtime
        .get_module_namespace(module_id)
        .map_err(|e| ConfigEvalError::Evaluation(format!("get_module_namespace failed: {e}")))?;

    // Enter a v8 scope to read the default export.
    deno_core::scope!(scope, &mut runtime);
    let local_ns: v8::Local<v8::Object> = v8::Local::new(scope, namespace);

    // Read `.default`.
    let key = v8::String::new(scope, "default")
        .ok_or_else(|| ConfigEvalError::Evaluation("v8 string alloc failed".into()))?;
    let default_val = local_ns
        .get(scope, key.into())
        .ok_or_else(|| ConfigEvalError::NotAPlainObject)?;

    // The default export must be a plain object (not a function, array,
    // primitive, etc.). Functions are `is_object()` in V8, so check both.
    // Dates and arrays are also `is_object()` — those are tolerated because
    // JSON.stringify handles them deterministically (Date → ISO string,
    // array → JSON array). The product spec's "plain object" means
    // non-function, non-primitive at the top level; nested functions inside
    // an object are silently dropped by JSON.stringify (test e).
    if !default_val.is_object() || default_val.is_function() {
        return Err(ConfigEvalError::NotAPlainObject);
    }

    // JSON.stringify(default) inside the isolate so edge-case behaviour
    // matches today's config-loader.mjs wire contract. This means:
    //   - functions are silently dropped (not a hard error)
    //   - Date → ISO string
    //   - BigInt / Symbol / cycles → JSON.stringify runtime error
    //
    // Access: globalThis.JSON.stringify(defaultVal)
    let global = scope.get_current_context().global(scope);
    let json_key = v8::String::new(scope, "JSON")
        .ok_or_else(|| ConfigEvalError::Evaluation("v8 string alloc failed".into()))?;
    let json_obj = global
        .get(scope, json_key.into())
        .ok_or_else(|| ConfigEvalError::Evaluation("JSON not on globalThis".into()))?;
    let json_obj: v8::Local<v8::Object> = json_obj
        .try_into()
        .map_err(|_| ConfigEvalError::Evaluation("JSON is not an object".into()))?;

    let stringify_key = v8::String::new(scope, "stringify")
        .ok_or_else(|| ConfigEvalError::Evaluation("v8 string alloc failed".into()))?;
    let stringify_fn = json_obj
        .get(scope, stringify_key.into())
        .ok_or_else(|| ConfigEvalError::Evaluation("JSON.stringify not found".into()))?;
    let stringify_fn: v8::Local<v8::Function> = stringify_fn
        .try_into()
        .map_err(|_| ConfigEvalError::Evaluation("JSON.stringify is not a function".into()))?;

    // Call JSON.stringify(defaultVal) with a tc_scope to surface throws.
    v8::tc_scope!(tc, scope);
    let recv: v8::Local<v8::Value> = json_obj.into();
    let result = stringify_fn.call(tc, recv, &[default_val]);
    if tc.has_caught() {
        let exc = tc.exception();
        let msg = exc
            .map(|e| e.to_rust_string_lossy(tc))
            .unwrap_or_else(|| "<no exception>".to_string());
        return Err(ConfigEvalError::Evaluation(format!(
            "JSON.stringify threw: {msg}"
        )));
    }
    let result = result
        .ok_or_else(|| ConfigEvalError::Evaluation("JSON.stringify returned no value".into()))?;

    // JSON.stringify returns a v8 String — pull it back to Rust.
    if !result.is_string() {
        // JSON.stringify returns undefined only for top-level undefined/function/Symbol,
        // all of which were caught by the is_object() / is_function() guard above.
        return Err(ConfigEvalError::Evaluation(
            "JSON.stringify returned a non-string value (unexpected)".into(),
        ));
    }
    let json_str: v8::Local<v8::String> = result.try_into().map_err(|_| {
        ConfigEvalError::Evaluation("JSON.stringify result is not a String".into())
    })?;
    let json_rust = json_str.to_rust_string_lossy(tc);

    // Parse the JSON string back into a serde_json::Value.
    serde_json::from_str::<serde_json::Value>(&json_rust).map_err(|e| {
        ConfigEvalError::Evaluation(format!("serde_json parsing of stringify result failed: {e}"))
    })
}

// ---------------------------------------------------------------------------
// ConfigModuleLoader — rejects everything except the main-module specifier
// ---------------------------------------------------------------------------

/// In-memory module loader for the config evaluator.
///
/// Serves only the main module (by its synthetic URL). Every other specifier
/// — bare imports, `node:*`, relative imports — is rejected with the
/// no-Node-globals error message. This is the explicit product decision:
/// `zfb.config.ts` is a data config, not a general Node script.
struct ConfigModuleLoader {
    /// The synthetic main-module URL string (e.g. `"file:///zfb-config-eval/main.mjs"`).
    main_specifier: String,
    /// The JS source of the main module.
    main_source: String,
}

impl ConfigModuleLoader {
    fn new(main_specifier: &str, main_source: &str) -> Self {
        Self {
            main_specifier: main_specifier.to_string(),
            main_source: main_source.to_string(),
        }
    }
}

impl ModuleLoader for ConfigModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        _referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        // Allow only the main module specifier through.
        if specifier == self.main_specifier {
            return ModuleSpecifier::parse(specifier)
                .map_err(|e| ModuleLoaderError::generic(format!("parse error: {e}")));
        }
        // Every other import — bare, relative, node:*, etc. — is rejected.
        Err(ModuleLoaderError::generic(NO_BARE_IMPORTS_ERROR))
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let spec_str = module_specifier.as_str();
        if spec_str == self.main_specifier {
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                ModuleType::JavaScript,
                ModuleSourceCode::String(self.main_source.clone().into()),
                module_specifier,
                None,
            )));
        }
        // Should only be reached if resolve() was bypassed.
        ModuleLoadResponse::Sync(Err(ModuleLoaderError::generic(NO_BARE_IMPORTS_ERROR)))
    }
}
