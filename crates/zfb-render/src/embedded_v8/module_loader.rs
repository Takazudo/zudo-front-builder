//! Custom `deno_core::ModuleLoader` for the embedded V8 host.
//!
//! Resolves five families of specifier:
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
//! 5. Copied `.wasm` assets imported relatively from the synthetic bundle
//!    URL. Their bytes are read only from the bundle's asset root and exposed
//!    as `export default new WebAssembly.Module(...)`, matching workerd's
//!    module-export shape rather than Deno's Wasm-ESM instance exports.
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
//! disk-loaded: any `file://` URL that resolves into the same
//! directory as a registered alias target is permitted. This is the
//! minimum scope needed to support multi-file aliased libraries
//! without widening disk access to the entire filesystem.
//!
//! ### Canonical containment + trust semantics
//!
//! Containment is decided on **canonical** paths (both the candidate and
//! the configured target are `canonicalize`d, resolving symlinks fully).
//! The authority is exactly two arms: a canonical exact-target match, or a
//! canonical same-parent-directory match — never a bare `starts_with`
//! prefix, which would silently widen same-directory reach to every
//! descendant. An alias whose **configured** target is (or passes through)
//! a symlink is trusted — explicit plugin config expresses intent — and its
//! canonicalised location becomes the authority. The escape this closes is
//! a symlink *inside* the authorised directory that resolves *outside* it:
//! canonicalising the candidate collapses the whole chain, so its real
//! parent no longer matches the target's real parent and the read is denied.
//! A configured target that does not exist keeps the plugin-named
//! "could not be read" error rather than falling through to the generic
//! unresolved-module error.
//!
//! This closes the static-symlink escape. It does not close a TOCTOU race
//! against an attacker who can mutate the authorised directory *concurrently
//! with the build* (canonicalise-then-read is inherently a check/read pair);
//! aliases are registered by trusted plugin `setup`, and the build owns its
//! output tree, so that residual boundary is out of scope here — a
//! descriptor-relative no-follow open would be the fix if it ever matters.
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
use std::path::{Component, Path, PathBuf};

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
    /// Directory that contains the emitted bundle and its copied Wasm assets.
    ///
    /// This is deliberately opt-in: only the production threaded host has an
    /// on-disk bundle root. The synthetic `file:///zfb/...` URLs used by
    /// ordinary in-memory tests remain incapable of reading arbitrary files.
    bundle_asset_root: Option<PathBuf>,
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

    /// Allow relative `.wasm` imports from the synthetic bundle URL to read
    /// copied assets under `bundle_asset_root`.
    ///
    /// The loader URL-decodes the synthetic module path, rejects traversal and
    /// absolute-path forms, then canonicalizes both this root and the selected
    /// asset before reading. It never grants this authority to arbitrary
    /// `file://` specifiers.
    pub fn with_bundle_asset_root(mut self, bundle_asset_root: impl Into<PathBuf>) -> Self {
        self.bundle_asset_root = Some(bundle_asset_root.into());
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

    /// Resolve a synthetic bundle-relative `.wasm` specifier to an on-disk
    /// asset path. Returns `Ok(None)` unless this is exactly the narrowly
    /// allowed Wasm-import path.
    fn resolve_bundle_wasm_asset(
        &self,
        module_specifier: &ModuleSpecifier,
    ) -> Result<Option<PathBuf>, ModuleLoaderError> {
        const SYNTHETIC_BUNDLE_PATH_PREFIX: &str = "/zfb/";
        const SYNTHETIC_BUNDLE_URL_PREFIX: &str = "file:///zfb/";

        let Some(bundle_asset_root) = self.bundle_asset_root.as_deref() else {
            return Ok(None);
        };
        if !module_specifier
            .as_str()
            .starts_with(SYNTHETIC_BUNDLE_URL_PREFIX)
        {
            return Ok(None);
        }
        let Some(encoded_relative_path) = module_specifier
            .path()
            .strip_prefix(SYNTHETIC_BUNDLE_PATH_PREFIX)
        else {
            // Do not turn general file URLs into a disk-read capability.
            return Ok(None);
        };

        // `Url::path()` preserves percent escapes. Decode before looking at
        // components: `%2e%2e%2fsecret.wasm` must be rejected as traversal,
        // not treated as an ordinary filename.
        let decoded_relative_path = percent_decode_path(encoded_relative_path)?;
        let relative_path = Path::new(&decoded_relative_path);
        if relative_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("wasm")
        {
            return Ok(None);
        }
        if !is_safe_bundle_asset_relative_path(relative_path) {
            return Err(ModuleLoaderError::generic(format!(
                "embedded V8 host: Wasm import `{}` escapes the bundle asset root `{}`",
                module_specifier,
                bundle_asset_root.display()
            )));
        }

        let expected_path = bundle_asset_root.join(relative_path);
        let canonical_root = std::fs::canonicalize(bundle_asset_root).map_err(|error| {
            ModuleLoaderError::generic(format!(
                "embedded V8 host: could not resolve bundle asset root `{}`: {error}",
                bundle_asset_root.display()
            ))
        })?;
        let canonical_path = std::fs::canonicalize(&expected_path).map_err(|error| {
            ModuleLoaderError::generic(format!(
                "embedded V8 host: Wasm asset expected at `{}` could not be read: {error}",
                expected_path.display()
            ))
        })?;
        if !canonical_path.starts_with(&canonical_root) {
            return Err(ModuleLoaderError::generic(format!(
                "embedded V8 host: Wasm asset `{}` escapes the bundle asset root `{}`",
                expected_path.display(),
                canonical_root.display()
            )));
        }

        Ok(Some(canonical_path))
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
        // Explicitly registered in-memory modules always win over disk-backed
        // sources. Besides preserving the existing loader contract, this keeps
        // tests and callers able to inject a synthetic `.wasm` module without
        // granting a filesystem read.
        if spec_str.starts_with("file://") {
            let modules = self.modules.borrow();
            if let Some(src) = modules.get(spec_str) {
                return ok_js(module_specifier, src.as_str());
            }
        }
        // 4. Copied `.wasm` asset imported from the synthetic bundle URL?
        // This is intentionally before the generic file:// alias branch: the
        // asset authority is separate from, and narrower than, plugin aliases.
        match self.resolve_bundle_wasm_asset(module_specifier) {
            Ok(Some(asset_path)) => {
                let bytes = match std::fs::read(&asset_path) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        return ModuleLoadResponse::Sync(Err(ModuleLoaderError::generic(
                            format!(
                                "embedded V8 host: Wasm asset expected at `{}` could not be read: {error}",
                                asset_path.display()
                            ),
                        )));
                    }
                };
                return ok_wasm_module(module_specifier, &bytes);
            }
            Ok(None) => {}
            Err(error) => return ModuleLoadResponse::Sync(Err(error)),
        }
        // 5. Alias-resolved file URL (plugin-registered)?
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
                match alias_disk_read_authority(hooks, module_specifier) {
                    AliasDiskAuthority::Authorized {
                        canonical_path,
                        plugin,
                    } => {
                        // Check the extension and read from the CANONICAL path
                        // — canonicalise-for-check / read-original would be a
                        // check/read race that weakens the containment fix.
                        return read_alias_target(module_specifier, &canonical_path, &plugin);
                    }
                    AliasDiskAuthority::MissingTarget { plugin } => {
                        // Read the original (missing) path so the failure keeps
                        // the plugin-named "could not be read" diagnostic.
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
                        return read_alias_target(module_specifier, &path, &plugin);
                    }
                    AliasDiskAuthority::Denied => {}
                }
            }
            // file:// URL that is neither in-memory nor reachable from
            // an alias target — fall through to the catch-all below.
        }
        // 6. In-memory module (the main bundle, etc.)?
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

/// Build the workerd-parity ESM wrapper for a copied Wasm binary.
///
/// Deno's native `ModuleType::Wasm` exports an instance's exports, which is a
/// different contract. The Worker module-loader contract is the compiled,
/// uninstantiated `WebAssembly.Module` itself, so keep the generated source
/// explicitly shaped as `export default new WebAssembly.Module(...)`.
fn ok_wasm_module(specifier: &ModuleSpecifier, bytes: &[u8]) -> ModuleLoadResponse {
    let encoded = super::base64_encode(bytes);
    let encoded_json = serde_json::to_string(&encoded).expect("base64 is valid JSON text");
    let source = format!(
        "export default new WebAssembly.Module(Uint8Array.from(atob({encoded_json}), byte => byte.charCodeAt(0)));\n"
    );
    ok_js(specifier, &source)
}

/// Decode percent escapes in a URL path, rejecting malformed escapes and
/// non-UTF-8 output. Bundle asset names originate from esbuild's UTF-8 output;
/// rejecting malformed forms is safer than giving them ambiguous path meaning.
fn percent_decode_path(input: &str) -> Result<String, ModuleLoaderError> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(high) = bytes.get(index + 1).and_then(|byte| hex_value(*byte)) else {
                return Err(ModuleLoaderError::generic(format!(
                    "embedded V8 host: malformed percent escape in Wasm import path `{input}`"
                )));
            };
            let Some(low) = bytes.get(index + 2).and_then(|byte| hex_value(*byte)) else {
                return Err(ModuleLoaderError::generic(format!(
                    "embedded V8 host: malformed percent escape in Wasm import path `{input}`"
                )));
            };
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| {
        ModuleLoaderError::generic(format!(
            "embedded V8 host: Wasm import path `{input}` is not valid UTF-8 after percent decoding"
        ))
    })
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Reject encoded path escapes before joining an import onto the asset root.
/// Backslashes are rejected even on Unix because they become path separators
/// on Windows and would otherwise make this loader platform-dependent.
fn is_safe_bundle_asset_relative_path(path: &Path) -> bool {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return false;
    }
    for component in path.components() {
        match component {
            Component::Normal(segment) => {
                if segment.to_string_lossy().contains('\\')
                    || segment.to_string_lossy().contains('\0')
                {
                    return false;
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
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

/// Outcome of deciding whether a `file://` URL is within an alias's
/// disk-read authority. Richer than a bare bool so the loader can keep the
/// plugin-attributed "could not be read" diagnostic for a missing configured
/// target (rather than falling through to the generic unresolved-module
/// error) while still denying a genuine containment escape.
enum AliasDiskAuthority {
    /// Authorised: read from this canonical path (symlinks already resolved).
    /// `plugin` attributes the owning alias for diagnostics.
    Authorized {
        canonical_path: PathBuf,
        plugin: String,
    },
    /// The candidate is a configured alias target that does not exist on disk
    /// (nothing to canonicalise). The loader reads the original path so the
    /// failure keeps the existing plugin-named "could not be read" error.
    MissingTarget { plugin: String },
    /// Not within any alias's authority — the loader denies the read.
    Denied,
}

/// Decide whether a `file://` URL is within an alias's authority, deciding
/// containment on **canonical** paths (symlinks resolved on both sides).
///
/// Authority shape (the canonicalised counterparts of the two historical
/// lexical arms):
///  - **canonical exact-target match** — the candidate resolves to the same
///    real file as a configured alias target; or
///  - **canonical same-parent-directory match** — the candidate resolves into
///    the same real directory as a configured alias target, so a top-level
///    alias entry file can transitively `import './sibling.js'`.
///
/// Trust boundary: an alias whose **configured** target is (or passes through)
/// a symlink is trusted — explicit plugin config expresses intent — and its
/// canonicalised location becomes the authority. What is rejected is a symlink
/// *inside* the authorised directory that resolves *outside* it: canonicalising
/// the candidate collapses the whole chain, so its real parent no longer
/// matches the target's real parent and the read is denied. This is
/// deliberately NOT a `starts_with` prefix check — a prefix match would silently
/// widen same-directory authority to every descendant of the target's folder.
///
/// Why parent-directory (not deeper-subtree) match: the conservative scope
/// handles the common case ("alias points at one entry file, which imports
/// siblings in the same folder") without expanding disk access to arbitrary
/// descendants. If a real-world case needs deeper reach, widen this in a
/// follow-up — but document the trust boundary first.
fn alias_disk_read_authority(
    hooks: &PluginRegistryHooks,
    module_specifier: &ModuleSpecifier,
) -> AliasDiskAuthority {
    let Ok(candidate_path) = module_specifier.to_file_path() else {
        return AliasDiskAuthority::Denied;
    };
    // Resolve the candidate's full symlink chain once. `None` means the file
    // does not exist (a missing target, or a dangling/absent sibling).
    let canonical_candidate = std::fs::canonicalize(&candidate_path).ok();
    // When the candidate file itself is missing, its directory usually still
    // exists; canonicalise that so a missing *sibling* import inside an
    // authorised directory keeps the plugin-attributed "could not be read"
    // error instead of degrading to the generic unresolved-module message.
    // This is still canonical-to-canonical, so it does not widen containment.
    let canonical_candidate_parent = if canonical_candidate.is_none() {
        candidate_path
            .parent()
            .and_then(|parent| std::fs::canonicalize(parent).ok())
    } else {
        None
    };
    let mut missing_target: Option<String> = None;
    for entry in hooks.aliases.values() {
        match std::fs::canonicalize(&entry.target) {
            Ok(canonical_target) => {
                if let Some(canonical_candidate) = canonical_candidate.as_ref() {
                    // Canonical exact-target match takes precedence in attribution.
                    if *canonical_candidate == canonical_target {
                        return AliasDiskAuthority::Authorized {
                            canonical_path: canonical_candidate.clone(),
                            plugin: entry.plugin.clone(),
                        };
                    }
                    // Canonical same-parent-directory match (transitive sibling
                    // imports inside the alias-rooted module folder).
                    if let (Some(candidate_dir), Some(target_dir)) =
                        (canonical_candidate.parent(), canonical_target.parent())
                    {
                        if candidate_dir == target_dir {
                            return AliasDiskAuthority::Authorized {
                                canonical_path: canonical_candidate.clone(),
                                plugin: entry.plugin.clone(),
                            };
                        }
                    }
                } else if let (Some(candidate_dir), Some(target_dir)) = (
                    canonical_candidate_parent.as_deref(),
                    canonical_target.parent(),
                ) {
                    // Candidate file is missing but its (existing) directory is
                    // the alias target's canonical directory: an absent sibling
                    // import. Keep the plugin-attributed read error.
                    if candidate_dir == target_dir {
                        missing_target.get_or_insert_with(|| entry.plugin.clone());
                    }
                }
            }
            Err(_) => {
                // Configured target does not exist. If the bundle imported this
                // exact (still lexical, since there is nothing to canonicalise)
                // target, keep the plugin-named read error instead of denying.
                if candidate_path == entry.target {
                    missing_target.get_or_insert_with(|| entry.plugin.clone());
                }
            }
        }
    }
    match missing_target {
        Some(plugin) => AliasDiskAuthority::MissingTarget { plugin },
        None => AliasDiskAuthority::Denied,
    }
}

/// Read an alias-authorised module from `path` and wrap it as a JavaScript
/// `ModuleSource` keyed on the original `specifier` (so deno_core's module map
/// stays keyed on the imported URL while the bytes come from the canonical
/// path). `plugin` is used only for diagnostics. Rejects TS/TSX targets — the
/// V8 host does not transpile — and surfaces read failures with the plugin
/// attribution.
fn read_alias_target(specifier: &ModuleSpecifier, path: &Path, plugin: &str) -> ModuleLoadResponse {
    // v1 limitation: the V8 host does not transpile TS/TSX. Reject these
    // targets with a clear error rather than letting V8 cough up an opaque
    // syntax error on the user's source.
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if matches!(ext, "ts" | "tsx" | "mts" | "cts") {
            return ModuleLoadResponse::Sync(Err(ModuleLoaderError::generic(format!(
                "embedded V8 host: alias target `{}` (plugin `{plugin}`) \
                 has a TypeScript extension, but the V8 host only accepts \
                 pre-compiled ESM JavaScript. Point the alias at a `.js` \
                 file or have your bundler pre-process the target.",
                path.display()
            ))));
        }
    }
    match std::fs::read_to_string(path) {
        Ok(src) => ok_js(specifier, &src),
        Err(e) => ModuleLoadResponse::Sync(Err(ModuleLoaderError::generic(format!(
            "embedded V8 host: alias-rooted file `{}` (under plugin \
             `{plugin}`'s alias) could not be read: {e}",
            path.display()
        )))),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::BundleModuleLoader;
    use crate::{EmbeddedV8RenderHost, HttpRequestLike, RenderHost};

    // A valid, empty Wasm module: magic, version, and no sections.
    const EMPTY_WASM_MODULE: &[u8] = b"\0asm\x01\0\0\0";

    fn write_wasm_asset(dir: &TempDir, name: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, EMPTY_WASM_MODULE).expect("write Wasm fixture");
        path
    }

    fn bundle_importing_wasm(specifier: &str) -> String {
        format!(
            r#"
            import wasm from "{specifier}";
            export default {{
              fetch() {{
                return new Response(
                  wasm instanceof WebAssembly.Module ? "module" : "not-module",
                  {{ status: 200 }},
                );
              }},
            }};
            "#
        )
    }

    fn host_for_asset_root(asset_root: &Path) -> EmbeddedV8RenderHost {
        let loader = BundleModuleLoader::new().with_bundle_asset_root(asset_root);
        EmbeddedV8RenderHost::with_loader(loader).expect("host boot")
    }

    async fn load_failure(asset_root: &Path, source: &str) -> String {
        let mut host = host_for_asset_root(asset_root);
        host.execute_module("bundle.mjs", source)
            .await
            .expect_err("untrusted Wasm import must fail")
            .to_string()
    }

    #[tokio::test]
    async fn bundle_wasm_import_exports_a_webassembly_module() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_wasm_asset(&dir, "tiny.wasm");
        let mut host = host_for_asset_root(dir.path());

        host.execute_module("bundle.mjs", &bundle_importing_wasm("./tiny.wasm"))
            .await
            .expect("bundle should load its copied Wasm asset");
        let response = host
            .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
            .await
            .expect("dispatch");

        assert_eq!(response.status, 200);
        assert_eq!(response.body_utf8(), Some("module"));
    }

    #[tokio::test]
    async fn bundle_wasm_import_percent_decodes_asset_filename() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_wasm_asset(&dir, "tiny module.wasm");
        let mut host = host_for_asset_root(dir.path());

        host.execute_module("bundle.mjs", &bundle_importing_wasm("./tiny%20module.wasm"))
            .await
            .expect("URL-escaped asset filename should load");
        let response = host
            .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
            .await
            .expect("dispatch");

        assert_eq!(response.body_utf8(), Some("module"));
    }

    #[tokio::test]
    async fn in_memory_wasm_module_takes_precedence_over_bundle_assets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let loader = BundleModuleLoader::new().with_bundle_asset_root(dir.path());
        loader.register_module("file:///zfb/in-memory.wasm", r#"export default "memory";"#);
        let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");
        let bundle = r#"
            import wasm from "./in-memory.wasm";
            export default {
              fetch() {
                return new Response(wasm, { status: 200 });
              },
            };
            "#;

        host.execute_module("bundle.mjs", bundle)
            .await
            .expect("in-memory Wasm module should load before a disk lookup");
        let response = host
            .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
            .await
            .expect("dispatch");

        assert_eq!(response.body_utf8(), Some("memory"));
    }

    #[tokio::test]
    async fn bundle_wasm_import_rejects_encoded_traversal_and_absolute_file_urls() {
        let fixture = tempfile::tempdir().expect("tempdir");
        let asset_root = fixture.path().join("assets");
        std::fs::create_dir(&asset_root).expect("asset root");
        let outside = write_wasm_asset(&fixture, "outside.wasm");

        let traversal_error =
            load_failure(&asset_root, &bundle_importing_wasm("./..%2Foutside.wasm")).await;
        assert!(
            traversal_error.contains("escapes the bundle asset root"),
            "percent-decoded traversal must hit the containment guard: {traversal_error}"
        );

        let absolute_specifier = deno_core::ModuleSpecifier::from_file_path(&outside)
            .expect("absolute file URL")
            .to_string();
        let absolute_error =
            load_failure(&asset_root, &bundle_importing_wasm(&absolute_specifier)).await;
        assert!(
            absolute_error.contains("no in-memory source"),
            "absolute file URL must retain the no-arbitrary-file-read failure: {absolute_error}"
        );

        let hosted_file_url =
            deno_core::ModuleSpecifier::parse("file://asset-host/zfb/outside.wasm")
                .expect("hosted file URL");
        let loader = BundleModuleLoader::new().with_bundle_asset_root(&asset_root);
        assert!(
            loader
                .resolve_bundle_wasm_asset(&hosted_file_url)
                .expect("hosted file URL is not malformed")
                .is_none(),
            "only the exact synthetic file:///zfb/ namespace may load copied Wasm assets"
        );
    }

    #[tokio::test]
    async fn bundle_wasm_import_missing_asset_names_expected_disk_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let expected = dir.path().join("missing.wasm");

        let error = load_failure(dir.path(), &bundle_importing_wasm("./missing.wasm")).await;
        assert!(
            error.contains(&expected.display().to_string()),
            "missing asset error must name the expected disk path `{}`: {error}",
            expected.display()
        );
    }
}
