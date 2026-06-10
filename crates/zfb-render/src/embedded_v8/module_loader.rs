//! Custom `deno_core::ModuleLoader` for the embedded V8 host.
//!
//! Resolves four families of specifier:
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
//! 4. Plugin-registered specifiers wired in via [`PluginRegistryHooks`]
//!    (sub-issue #260 — Astro-migration epic #253):
//!    - **Aliases**: exact-match bare specifiers rewritten to a
//!      filesystem path. `resolve()` maps the specifier to a
//!      `file://` URL; `load()` reads the file from disk.
//!    - **Virtual modules**: bare specifiers whose JS source was
//!      pre-resolved by the plugin host before the V8 runtime started.
//!      `load()` serves the cached source string directly so the
//!      loader closure is invoked exactly once per build regardless of
//!      how many pages import the same virtual specifier.
//!
//! ## Why pre-resolved virtual modules (not async invocation)
//!
//! `deno_core::ModuleLoader::load` returns `ModuleLoadResponse`, which
//! can be `Future`-based; however the plugin host's
//! `invoke_virtual_loader` is an async JSON-RPC call that lives in
//! `zfb-build`, which depends on `zfb-render` and therefore cannot
//! be imported here without creating a dependency cycle. Callers that
//! hold a `PluginHost` must call `invoke_virtual_loader` for each
//! registered specifier **before** constructing the host, then pass
//! the resolved sources into [`PluginRegistryHooks`]. This also
//! naturally satisfies acceptance criterion 4 ("loader invoked exactly
//! once per build per specifier").
//!
//! ## Alias resolution and filesystem access
//!
//! Aliases resolve to absolute filesystem paths produced by the
//! `PluginSetupAccumulator` in `zfb-build`. The loader reads those
//! files from disk via `std::fs::read_to_string`. This is an
//! intentional exception to "bundles must be self-contained" — plugin
//! aliases are a deliberate escape hatch for user-land libraries that
//! the esbuild bundler did not inline (typically because they are
//! registered at plugin `setup` time after the bundle entry is
//! already determined).
//!
//! Transitive `./sibling.js` imports from an aliased file are also
//! disk-loaded: any `file://` URL whose path sits in the same
//! directory as a registered alias target is permitted. This is the
//! minimum scope needed to support multi-file aliased libraries
//! without widening disk access to the entire filesystem.
//!
//! ### TS/TSX caveat (v1 limitation)
//!
//! The V8 host expects ESM-compatible JavaScript at this layer; it
//! does NOT route alias targets through the SWC pipeline. Aliases
//! that point at `.ts` / `.tsx` source files will fail to parse
//! inside V8. The expected pattern is to alias to already-emitted
//! `.js` modules (or have the esbuild bundler pre-process them at
//! bundle time before the V8 host sees them). A follow-up may extend
//! this loader to run alias targets through SWC; until then the
//! loader emits a targeted error for `.ts` / `.tsx` paths so the
//! failure mode is clear.
//!
//! Every other specifier is rejected with a clear error so the host
//! does not silently absorb a typo'd import.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use deno_core::error::ModuleLoaderError;
use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleSource,
    ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind,
};

use super::extensions::{ext_node_stubs_source, node_stub_source};

// ---------------------------------------------------------------------------
// PluginRegistryHooks — the alias + virtual-module data consumed by Wave 2
// ---------------------------------------------------------------------------

/// One alias entry as the loader sees it: a bare `from` specifier (exact
/// match only) and the absolute filesystem path it should resolve to.
///
/// The `plugin` field is carried for diagnostics — it surfaces in any
/// error the loader produces when the file cannot be read.
#[derive(Debug, Clone)]
pub struct AliasHook {
    /// Absolute path of the target file on disk.
    pub target: PathBuf,
    /// Display name of the plugin that registered the alias.
    pub plugin: String,
}

/// One virtual-module entry as the loader sees it: the bare specifier and
/// its pre-resolved JS source string, plus the registering plugin name for
/// diagnostics.
///
/// The source is computed by calling `PluginHost::invoke_virtual_loader`
/// **before** the V8 host is constructed so the loader closure runs exactly
/// once per build regardless of how many pages import the specifier.
#[derive(Debug, Clone)]
pub struct VirtualModuleHook {
    /// Pre-resolved ESM source text for this specifier.
    pub source: String,
    /// Display name of the plugin that registered the loader.
    pub plugin: String,
}

/// Plugin-registry hooks wired into [`BundleModuleLoader`] at construction
/// time (sub-issue #260).
///
/// Build with [`PluginRegistryHooks::builder`] and pass the result to
/// [`BundleModuleLoader::with_plugin_hooks`].
#[derive(Debug, Default, Clone)]
pub struct PluginRegistryHooks {
    /// Exact-match alias map: bare `from` specifier → [`AliasHook`].
    pub aliases: HashMap<String, AliasHook>,
    /// Virtual-module map: bare specifier → [`VirtualModuleHook`].
    pub virtual_modules: HashMap<String, VirtualModuleHook>,
}

impl PluginRegistryHooks {
    /// Create an empty hooks bundle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an alias. The `from` key is the exact bare specifier the
    /// bundle may import (e.g. `"@/components/foo"`); `target` is the
    /// resolved absolute path of the file to load.
    pub fn add_alias(
        &mut self,
        from: impl Into<String>,
        target: PathBuf,
        plugin: impl Into<String>,
    ) {
        self.aliases.insert(
            from.into(),
            AliasHook {
                target,
                plugin: plugin.into(),
            },
        );
    }

    /// Insert a virtual module. The `specifier` is the exact bare specifier
    /// the bundle may import (e.g. `"virtual:my-data"`); `source` is the
    /// pre-resolved ESM source string.
    pub fn add_virtual_module(
        &mut self,
        specifier: impl Into<String>,
        source: impl Into<String>,
        plugin: impl Into<String>,
    ) {
        self.virtual_modules.insert(
            specifier.into(),
            VirtualModuleHook {
                source: source.into(),
                plugin: plugin.into(),
            },
        );
    }
}

// ---------------------------------------------------------------------------
// BundleModuleLoader
// ---------------------------------------------------------------------------

/// Loader that serves: the main bundle source (in-memory), the
/// `node:*` stubs, the internal `ext:zfb_node_stubs/*` helpers, and
/// optionally plugin-registered aliases and virtual modules via
/// [`PluginRegistryHooks`].
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
    /// Optional plugin-registry hooks (aliases + virtual modules).
    /// `None` when no plugins registered any contributions.
    hooks: Option<PluginRegistryHooks>,
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

    /// Attach plugin-registry hooks (aliases and virtual modules) to
    /// this loader (sub-issue #260). Replaces any previously attached
    /// hooks. Returns `self` for chaining.
    pub fn with_plugin_hooks(mut self, hooks: PluginRegistryHooks) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Register an in-memory module. Called by the host on demand
    /// (e.g. tests that want to inject a Hono shim).
    pub fn register_module(&self, specifier: &str, source: impl Into<String>) {
        self.modules
            .borrow_mut()
            .insert(specifier.to_string(), source.into());
    }

    // -----------------------------------------------------------------------
    // Internal helpers for alias / virtual-module resolution
    // -----------------------------------------------------------------------

    /// Look up a bare `specifier` in the alias map. Returns the resolved
    /// `file://` URL string if the specifier has an exact-match alias.
    fn resolve_alias(&self, specifier: &str) -> Option<String> {
        let hooks = self.hooks.as_ref()?;
        let entry = hooks.aliases.get(specifier)?;
        // Convert the absolute PathBuf to a `file://` URL. The path is
        // guaranteed absolute by `PluginSetupAccumulator::resolve_against_root`.
        let url = url_from_path(&entry.target)?;
        Some(url)
    }

    /// Look up a resolved specifier (after `resolve()` ran) in the
    /// virtual-module map. The specifier stored in the hook is the bare
    /// form (e.g. `"virtual:my-data"`); after `deno_core::resolve_import`
    /// it becomes a full URL (e.g. `"virtual:my-data"` is a valid URI
    /// scheme+path). We match on the original bare form by checking
    /// whether the URL's string form equals the bare specifier directly
    /// (virtual specifiers survive URL parsing unchanged).
    fn find_virtual_module(&self, spec_str: &str) -> Option<&VirtualModuleHook> {
        let hooks = self.hooks.as_ref()?;
        hooks.virtual_modules.get(spec_str)
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
        // Alias lookup (exact match, plugin-registered). Rewrite the
        // bare specifier to the `file://` URL of the aliased file so
        // `load()` can read it from disk.
        if let Some(file_url) = self.resolve_alias(specifier) {
            return ModuleSpecifier::parse(&file_url).map_err(|e| {
                ModuleLoaderError::generic(format!(
                    "embedded V8 host: alias `{specifier}` resolved to `{file_url}` \
                     which is not a valid URL: {e}"
                ))
            });
        }
        // Virtual module (exact match, plugin-registered). These are
        // valid URI shapes (scheme + path), so `ModuleSpecifier::parse`
        // accepts them directly — no `resolve_import` base-URL
        // arithmetic needed.
        if self.find_virtual_module(specifier).is_some() {
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
        // 3. Virtual module (plugin-registered, pre-resolved source)?
        //    The specifier survived URL parsing in `resolve()` unchanged
        //    so `spec_str` here is the original bare form.
        if let Some(vm) = self.find_virtual_module(spec_str) {
            return ok_js(module_specifier, &vm.source);
        }
        // 4. Alias-resolved file URL (plugin-registered)?
        //    `resolve()` rewrote the bare alias to a `file://` URL; the
        //    URL's path component is the absolute filesystem path.
        //
        //    Disk reads are permitted for:
        //      (a) the alias's exact target file, and
        //      (b) any file in the **same directory** as a registered
        //          alias target — so a top-level alias to
        //          `/proj/src/lib/foo.js` can transitively `import
        //          './bar.js'` and have `bar.js` resolve via disk.
        //
        //    (b) is the minimum scope needed to support multi-file
        //    aliased libraries without widening disk access to the
        //    entire filesystem. Aliases are trusted (registered by
        //    plugin `setup`, not user data), so sibling-file reads
        //    inherit that trust.
        if spec_str.starts_with("file://") {
            // First check the in-memory map (registered modules and the
            // bundle's own entry take precedence over disk).
            let modules = self.modules.borrow();
            if let Some(src) = modules.get(spec_str) {
                return ok_js(module_specifier, src.as_str());
            }
            drop(modules);
            if let Some(hooks) = &self.hooks {
                if let Some(plugin_name) = alias_disk_read_authority(hooks, module_specifier) {
                    let path = match module_specifier.to_file_path() {
                        Ok(p) => p,
                        Err(_) => {
                            return ModuleLoadResponse::Sync(Err(ModuleLoaderError::generic(
                                format!(
                                    "embedded V8 host: alias-rooted target `{spec_str}` \
                                     is not a valid file:// URL"
                                ),
                            )));
                        }
                    };
                    // v1 limitation: the V8 host does not transpile
                    // TS/TSX. Reject these targets with a clear error
                    // rather than letting V8 cough up an opaque syntax
                    // error on the user's source.
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if matches!(ext, "ts" | "tsx" | "mts" | "cts") {
                            return ModuleLoadResponse::Sync(Err(ModuleLoaderError::generic(
                                format!(
                                    "embedded V8 host: alias target `{}` (plugin `{plugin_name}`) \
                                     has a TypeScript extension, but the V8 host only accepts \
                                     pre-compiled ESM JavaScript. Point the alias at a `.js` \
                                     file or have your bundler pre-process the target.",
                                    path.display()
                                ),
                            )));
                        }
                    }
                    let src = match std::fs::read_to_string(&path) {
                        Ok(s) => s,
                        Err(e) => {
                            return ModuleLoadResponse::Sync(Err(ModuleLoaderError::generic(
                                format!(
                                    "embedded V8 host: alias-rooted file `{}` (under plugin \
                                     `{plugin_name}`'s alias) could not be read: {e}",
                                    path.display()
                                ),
                            )));
                        }
                    };
                    return ok_js(module_specifier, &src);
                }
            }
            // file:// URL that is neither in-memory nor reachable from
            // an alias target — fall through to the catch-all below.
        }
        // 5. In-memory module (the main bundle, etc.)?
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

/// Parse a `node:*` / `ext:*` / `virtual:*` specifier as a
/// `ModuleSpecifier`. `Url::parse` accepts all of these because they
/// are valid URI shapes (scheme + path); we just bypass the
/// relative-base machinery.
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

/// Convert an absolute [`PathBuf`] to a `file://` URL string.
/// Returns `None` if the path is not absolute (shouldn't happen — the
/// `PluginSetupAccumulator` always produces absolute paths).
fn url_from_path(path: &std::path::Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    // Use `url::Url::from_file_path` to get the canonical `file://`
    // encoding including percent-escaping of special characters.
    // `deno_core` re-exports `url`, so we go through `ModuleSpecifier`
    // (which is `url::Url`) to avoid a separate `url` dep.
    let url = ModuleSpecifier::from_file_path(path).ok()?;
    Some(url.to_string())
}

/// Decide whether a `file://` URL is within an alias's authority — i.e.
/// the URL points at an alias target file or any file in the same
/// directory as a target. Returns the plugin name of the closest matching
/// alias when authorised, so error messages can attribute disk-read
/// failures to the plugin that effectively owns the directory.
///
/// Why parent-directory (not deeper-subtree) match: the conservative
/// scope handles the common case ("alias points at one entry file, which
/// imports siblings in the same folder") without expanding disk access
/// to arbitrary descendants. If a real-world case needs deeper reach,
/// widen this in a follow-up — but document the trust boundary first.
fn alias_disk_read_authority(
    hooks: &PluginRegistryHooks,
    module_specifier: &ModuleSpecifier,
) -> Option<String> {
    let candidate_path = module_specifier.to_file_path().ok()?;
    let candidate_dir = candidate_path.parent()?;
    for entry in hooks.aliases.values() {
        // Exact-target match takes precedence in attribution.
        if entry.target == candidate_path {
            return Some(entry.plugin.clone());
        }
        // Same-directory match (transitive imports inside the
        // alias-rooted module folder).
        if let Some(target_dir) = entry.target.parent() {
            if target_dir == candidate_dir {
                return Some(entry.plugin.clone());
            }
        }
    }
    None
}
