//! Custom `deno_core::ModuleLoader` for the embedded V8 host.
//!
//! Resolves three families of specifier:
//!
//! 1. The bundle's main entry — registered up front via
//!    [`BundleModuleLoader::with_main`] and served from an in-memory
//!    [`String`].
//! 2. `node:*` stubs — resolved from
//!    [`super::extensions::node_stub_source`]. v1 list lives in
//!    [`super::extensions::NODE_STUB_SPECIFIERS`].
//! 3. `ext:zfb_node_stubs/*` synthetic — internal helper imports the
//!    `node:*` stubs themselves use to share the throwing-proxy
//!    factory.
//!
//! Every other specifier is rejected with a clear error so the host
//! does not silently absorb a typo'd import.

use std::cell::RefCell;
use std::collections::HashMap;

use deno_core::error::ModuleLoaderError;
use deno_core::{
    ModuleLoadResponse, ModuleLoadOptions, ModuleLoadReferrer, ModuleLoader, ModuleSource,
    ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind,
};

use super::extensions::{ext_node_stubs_source, node_stub_source};

/// Loader that serves: the main bundle source (in-memory), the
/// `node:*` stubs, and the internal `ext:zfb_node_stubs/*` helpers.
///
/// Additional in-memory modules can be registered via
/// [`Self::register_module`] — handy for tests that load a stub
/// "Hono"-shaped helper without a separate file.
#[derive(Default)]
pub struct BundleModuleLoader {
    /// `specifier-string → source-string` for in-memory modules. Uses
    /// `RefCell` because `ModuleLoader::load` takes `&self` and we
    /// occasionally lazily insert the main bundle the first time it's
    /// asked for (via [`Self::with_main`]).
    modules: RefCell<HashMap<String, String>>,
}

impl BundleModuleLoader {
    /// Create an empty loader. Use [`Self::with_main`] to register the
    /// bundle entry, or [`Self::register_module`] for additional in-
    /// memory modules.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the bundle's main entry. The specifier is what the
    /// caller passes to `JsRuntime::load_main_es_module_from_code`;
    /// the source is the workerd-shape bundle JS.
    pub fn with_main(self, specifier: &str, source: impl Into<String>) -> Self {
        self.register_module(specifier, source);
        self
    }

    /// Register an in-memory module. Called by the host on demand
    /// (e.g. tests that want to inject a Hono shim).
    pub fn register_module(&self, specifier: &str, source: impl Into<String>) {
        self.modules
            .borrow_mut()
            .insert(specifier.to_string(), source.into());
    }
}

impl ModuleLoader for BundleModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        // `node:*` stays as-is — the URL parser would fail on the
        // colon-without-scheme-host shape, but `deno_core` will accept
        // any URL-shaped string we hand it back as a specifier.
        if specifier.starts_with("node:") {
            return parse_synthetic(specifier);
        }
        // Same story for our internal `ext:` namespace.
        if specifier.starts_with("ext:") {
            return parse_synthetic(specifier);
        }
        // Otherwise fall back to deno_core's standard URL-relative
        // resolution. The bundle's main entry is registered with a
        // `file://` specifier (see [`crate::embedded_v8::main_specifier`])
        // so relative imports inside the bundle (rare for the workerd
        // shape, since esbuild has already inlined them) still resolve.
        deno_core::resolve_import(specifier, referrer).map_err(|e| {
            ModuleLoaderError::generic(format!(
                "embedded V8 host: failed to resolve `{specifier}` from `{referrer}`: {e}"
            ))
        })
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let spec_str = module_specifier.as_str();
        // 1. node:* stub?
        if let Some(src) = node_stub_source(spec_str) {
            return ok_js(module_specifier, src);
        }
        // 2. ext:zfb_node_stubs/* helper?
        if let Some(src) = ext_node_stubs_source(spec_str) {
            return ok_js(module_specifier, src);
        }
        // 3. In-memory module (the main bundle, etc.)?
        let modules = self.modules.borrow();
        if let Some(src) = modules.get(spec_str) {
            return ok_js(module_specifier, src.as_str());
        }
        ModuleLoadResponse::Sync(Err(ModuleLoaderError::generic(format!(
            "embedded V8 host: no in-memory source for `{spec_str}`. \
             Bundles loaded by this host must be self-contained ESM \
             (the bundler inlines all imports); top-level \
             `import` of an unknown specifier is unsupported."
        ))))
    }
}

/// Parse a `node:*` / `ext:*` specifier as a `ModuleSpecifier`.
/// `Url::parse` accepts both because they are valid URI shapes (with
/// the scheme component); we just don't go through the relative-base
/// machinery.
fn parse_synthetic(specifier: &str) -> Result<ModuleSpecifier, ModuleLoaderError> {
    ModuleSpecifier::parse(specifier).map_err(|e| {
        ModuleLoaderError::generic(format!(
            "embedded V8 host: failed to parse synthetic specifier `{specifier}`: {e}"
        ))
    })
}

/// Build a `ModuleLoadResponse::Sync` carrying a JavaScript module.
fn ok_js(specifier: &ModuleSpecifier, src: &str) -> ModuleLoadResponse {
    ModuleLoadResponse::Sync(Ok(ModuleSource::new(
        ModuleType::JavaScript,
        ModuleSourceCode::String(src.to_string().into()),
        specifier,
        None,
    )))
}
