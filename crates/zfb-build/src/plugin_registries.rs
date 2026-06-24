//! Plugin lifecycle registries populated by the new `setup` hook
//! (Astro-migration epic #253 / sub-issue #255).
//!
//! `setup` runs once per build (or dev-host boot) and lets plugins
//! register three kinds of contributions:
//!
//! - **Aliases** — exact-match import rewrites consulted by the V8
//!   module loader (#260) and the islands esbuild plugin (#261).
//! - **Virtual modules** — bare specifiers whose source text is
//!   produced by a JS-side loader closure owned by the plugin host
//!   subprocess. The Rust side stores an opaque [`VirtualLoaderId`]
//!   that Wave 2 consumers pass back through
//!   [`crate::PluginHost::invoke_virtual_loader`] to fetch the text.
//! - **Injected routes** — dev-only synthetic page routes that the
//!   dev server should evaluate through the page rendering pipeline
//!   when the URL pattern matches.
//!
//! ## Why registries live in `zfb-build`
//!
//! The plugin host is here. Wave 2 (#260 V8 host resolver, #261
//! islands esbuild resolver) imports these types via
//! `use zfb_build::{AliasMap, VirtualModuleRegistry, InjectedRouteList};`
//! and consumes them at bundler / V8-loader wiring time. The
//! `dev-server` only needs [`InjectedRouteList`] (and an opaque
//! [`InjectedRouteSet`] handle in `zfb-server`), which is plumbed
//! through `crates/zfb/src/commands/dev.rs` without making
//! `zfb-server` depend on `zfb-build`.
//!
//! ## Conflict detection
//!
//! The accumulator collects raw registrations and validates on
//! commit. Errors carry both offending plugin names so the diagnostic
//! lands on the user's screen with enough context to fix the typo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Opaque token returned by the JS-side `setup` reply when a plugin
/// calls `addVirtualModule`. Wave 2 consumers pass this token back
/// through [`crate::PluginHost::invoke_virtual_loader`] to fetch the
/// module source text; the token has no meaning outside the host.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VirtualLoaderId(pub String);

impl VirtualLoaderId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One entry in [`VirtualModuleRegistry`].
#[derive(Debug, Clone)]
pub struct VirtualModuleEntry {
    /// Bare specifier the plugin registered (e.g. `"virtual:my-data"`).
    /// Recommended `virtual:` prefix; not enforced.
    pub specifier: String,
    /// Display name of the plugin that registered the loader. Surfaces
    /// in diagnostics when a loader throws.
    pub plugin: String,
    /// Opaque handle into the plugin host's loader map. Wave 2 callers
    /// invoke the loader via [`crate::PluginHost::invoke_virtual_loader`].
    pub loader_id: VirtualLoaderId,
}

/// Map from bare specifier (exact match) → registered loader.
///
/// Lookup is `O(1)` and consumed by Wave 2 (#260 V8 host resolver,
/// #261 islands esbuild resolver) at module-resolution time. The
/// loader closure lives on the JS side; this struct stores only the
/// opaque [`VirtualLoaderId`] and the registering plugin's name.
#[derive(Debug, Clone, Default)]
pub struct VirtualModuleRegistry {
    by_specifier: HashMap<String, VirtualModuleEntry>,
}

impl VirtualModuleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a registration. Caller must have already run
    /// [`PluginSetupAccumulator::commit`] for conflict checks; this is
    /// the unchecked low-level insert used by the accumulator.
    pub(crate) fn insert(&mut self, entry: VirtualModuleEntry) {
        self.by_specifier.insert(entry.specifier.clone(), entry);
    }

    pub fn get(&self, specifier: &str) -> Option<&VirtualModuleEntry> {
        self.by_specifier.get(specifier)
    }

    pub fn contains(&self, specifier: &str) -> bool {
        self.by_specifier.contains_key(specifier)
    }

    pub fn len(&self) -> usize {
        self.by_specifier.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_specifier.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &VirtualModuleEntry)> {
        self.by_specifier.iter()
    }
}

/// One entry in [`AliasMap`]. The `from` import specifier is the key
/// in the outer map; this struct carries the resolved target and
/// owning plugin.
#[derive(Debug, Clone)]
pub struct AliasEntry {
    /// Absolute filesystem path the alias resolves to. The original
    /// `to` argument from `addAlias` is joined against the project
    /// root at registration time so downstream consumers (Wave 2)
    /// receive a ready-to-use path without re-interpreting the
    /// string.
    pub target: PathBuf,
    /// Display name of the registering plugin.
    pub plugin: String,
}

/// Exact-match import alias map.
///
/// **Exact-match-only in v1**: a registration for `"@/foo"` matches
/// `import "@/foo"` but NOT `import "@/foo/bar"`. Prefix-matching is
/// explicitly deferred to v2.
#[derive(Debug, Clone, Default)]
pub struct AliasMap {
    by_from: HashMap<String, AliasEntry>,
}

impl AliasMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert(&mut self, from: String, entry: AliasEntry) {
        self.by_from.insert(from, entry);
    }

    /// Exact-match lookup. Returns `None` for both unregistered
    /// specifiers AND for prefix-style imports that happen to start
    /// with a registered alias.
    pub fn get(&self, from: &str) -> Option<&AliasEntry> {
        self.by_from.get(from)
    }

    pub fn contains(&self, from: &str) -> bool {
        self.by_from.contains_key(from)
    }

    pub fn len(&self) -> usize {
        self.by_from.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_from.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &AliasEntry)> {
        self.by_from.iter()
    }
}

/// One synthetic route contributed by a plugin's
/// `injectRoute(pattern, entrypoint, opts?)` call.
///
/// In `dev` mode these are the dev-router's synthetic routes. In
/// `build` mode (#1193, package-owned routes) they are materialised
/// into the per-build overlay pages root and prerendered through the
/// normal scan → bundle → render pipeline.
#[derive(Debug, Clone)]
pub struct InjectedRoute {
    /// URL pattern using the same `pages/` filename grammar
    /// (`/blog/[slug]`, `/api/dev/x`, etc.). Stored verbatim so the
    /// dev router can apply its existing pattern compiler.
    pub pattern: String,
    /// Absolute filesystem path of the TSX/TS entrypoint. The
    /// original `entrypoint` argument is joined against the project
    /// root at registration time.
    pub entrypoint: PathBuf,
    /// Display name of the registering plugin.
    pub plugin: String,
    /// Optional `prerender` hint from `injectRoute(pattern, entrypoint,
    /// { prerender })` (#1193). `None` (the dev path and any caller that
    /// omits the option) leaves the route's prerender shape to the
    /// build's default (SSG) — the build materialiser only inlines an
    /// explicit `export const prerender = …` when this is `Some`. A
    /// `Some(false)` route is SSR-shaped, which `output: static` rejects
    /// through the existing `resolve_v8_mode` gate.
    pub prerender: Option<bool>,
}

/// Ordered list of injected routes — preserves `Config.plugins`
/// declaration order so a deterministic match order is available to
/// the dev router.
#[derive(Debug, Clone, Default)]
pub struct InjectedRouteList {
    routes: Vec<InjectedRoute>,
}

impl InjectedRouteList {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push(&mut self, route: InjectedRoute) {
        self.routes.push(route);
    }

    pub fn iter(&self) -> impl Iterator<Item = &InjectedRoute> {
        self.routes.iter()
    }

    pub fn as_slice(&self) -> &[InjectedRoute] {
        &self.routes
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

/// Errors raised when conflict detection rejects an accumulated set
/// of `setup` registrations.
#[derive(Debug, thiserror::Error)]
pub enum SetupRegistryError {
    #[error(
        "alias `{from}` registered by plugin `{first_plugin}` -> `{first_target}` \
         conflicts with plugin `{second_plugin}` -> `{second_target}`"
    )]
    AliasConflict {
        from: String,
        first_plugin: String,
        first_target: String,
        second_plugin: String,
        second_target: String,
    },

    #[error(
        "virtual module `{specifier}` registered by plugin `{first_plugin}` \
         conflicts with plugin `{second_plugin}` — only one plugin may own a specifier"
    )]
    VirtualModuleConflict {
        specifier: String,
        first_plugin: String,
        second_plugin: String,
    },

    #[error(
        "injected route `{pattern}` registered by plugin `{first_plugin}` -> `{first_entrypoint}` \
         conflicts with plugin `{second_plugin}` -> `{second_entrypoint}` — same URL pattern \
         (a different plugin claiming the pattern, or the same plugin re-registering it with a \
         different entrypoint)"
    )]
    InjectRouteConflict {
        pattern: String,
        first_plugin: String,
        first_entrypoint: String,
        second_plugin: String,
        second_entrypoint: String,
    },

    /// Retained for a future genuinely-unsupported `injectRoute` shape
    /// during a build. As of #1193 the build path ACCEPTS prerenderable
    /// package routes, so this is no longer constructed by `ingest` — the
    /// old unconditional dev-only rejection is gone. Kept (allowed dead)
    /// rather than removed so a later wave can reintroduce a scoped
    /// rejection without re-plumbing the error surface.
    #[allow(dead_code)]
    #[error(
        "plugin `{plugin}` called `injectRoute(\"{pattern}\", ...)` during a build — \
         this route shape is not supported by the build path"
    )]
    InjectRouteInBuildMode { plugin: String, pattern: String },

    #[error(
        "client entry `{entry_name}` registered by plugin `{first_plugin}` -> `{first_entrypoint}` \
         conflicts with plugin `{second_plugin}` -> `{second_entrypoint}` — same entry name \
         (two plugins registering the same named client entry with different source files)"
    )]
    ClientEntryConflict {
        entry_name: String,
        first_plugin: String,
        first_entrypoint: String,
        second_plugin: String,
        second_entrypoint: String,
    },
}

/// One client-side side-effect entry registered by a plugin's `setup` hook
/// via `addClientEntry(entrypoint)` (#1196).
///
/// The entrypoint is a `*.client.{ts,tsx,js,jsx}` module that is bundled
/// and shipped as `/assets/client/<name>.js` alongside user-authored
/// `*.client.*` files discovered in the project tree. The entry name is
/// derived from the filename stem minus `.client` — the same convention as
/// `discover_client_scripts`. Collisions with user-authored entries of the
/// same name are a hard error.
#[derive(Debug, Clone)]
pub struct ClientEntry {
    /// Derived entry name (e.g. `"my-lib"` for `my-lib.client.ts`).
    pub entry_name: String,
    /// Absolute filesystem path of the source file.
    pub entrypoint: PathBuf,
    /// Display name of the registering plugin.
    pub plugin: String,
}

/// Ordered list of client-side entries registered via `addClientEntry` (#1196).
#[derive(Debug, Clone, Default)]
pub struct ClientEntryList {
    entries: Vec<ClientEntry>,
}

impl ClientEntryList {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push(&mut self, entry: ClientEntry) {
        self.entries.push(entry);
    }

    pub fn iter(&self) -> impl Iterator<Item = &ClientEntry> {
        self.entries.iter()
    }

    pub fn as_slice(&self) -> &[ClientEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Raw, unvalidated registration from a single plugin's `setup`
/// hook. The JS host collects every call into one of these and
/// returns the batch in the `setup` reply so conflict detection can
/// run in Rust against the canonical types.
#[derive(Debug, Clone)]
pub(crate) enum RawSetupRegistration {
    Alias {
        from: String,
        to: String,
    },
    VirtualModule {
        specifier: String,
        loader_id: VirtualLoaderId,
    },
    InjectRoute {
        pattern: String,
        entrypoint: String,
        /// Optional `prerender` hint (#1193). See [`InjectedRoute::prerender`].
        prerender: Option<bool>,
    },
    /// Client-side side-effect entry registered via `addClientEntry` (#1196).
    AddClientEntry {
        /// Path as supplied by the plugin (may be relative to project root).
        entrypoint: String,
    },
}

/// One plugin's raw `setup` output as returned by the JS host. The
/// `plugin` field is the display name of the plugin that produced the
/// registrations.
#[derive(Debug, Clone)]
pub(crate) struct RawPluginSetupOutput {
    pub plugin: String,
    pub registrations: Vec<RawSetupRegistration>,
}

/// Accumulator that walks every plugin's raw `setup` output in
/// declaration order and produces the three validated registries.
///
/// The accumulator owns conflict detection: as each registration is
/// added the existing per-specifier / per-from / per-pattern entry is
/// checked for a clash, and on first conflict the whole commit
/// aborts with the offending pair named.
///
/// As of #1193 `injectRoute` is accepted in both commands, so the
/// accumulator no longer needs the active [`SetupCommand`] — the only
/// command-dependent validation left (the dev-scoped `"/"` rejection)
/// runs JS-side in `plugin-host.mjs`. `new` still takes the command for
/// call-site symmetry with `run_setup`; it is intentionally unused here.
pub(crate) struct PluginSetupAccumulator<'a> {
    project_root: &'a Path,
    alias_origin: HashMap<String, (String, PathBuf)>, // from -> (plugin, target)
    virtual_origin: HashMap<String, String>,          // specifier -> plugin
    route_origin: HashMap<String, (String, PathBuf)>, // pattern -> (plugin, resolved entrypoint)
    client_entry_origin: HashMap<String, (String, PathBuf)>, // entry_name -> (plugin, path)
    aliases: AliasMap,
    virtual_modules: VirtualModuleRegistry,
    injected_routes: InjectedRouteList,
    client_entries: ClientEntryList,
}

/// The active `zfb` command for this run — `setup` ctx exposes it
/// on the JS side and `injectRoute` is rejected during builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupCommand {
    Build,
    Dev,
}

impl SetupCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            SetupCommand::Build => "build",
            SetupCommand::Dev => "dev",
        }
    }
}

impl<'a> PluginSetupAccumulator<'a> {
    // `_command` is retained for call-site symmetry with `run_setup` and
    // future command-scoped validation; conflict detection no longer
    // branches on it (#1193 — the dev-scoped `"/"` rejection runs JS-side).
    pub(crate) fn new(project_root: &'a Path, _command: SetupCommand) -> Self {
        Self {
            project_root,
            alias_origin: HashMap::new(),
            virtual_origin: HashMap::new(),
            route_origin: HashMap::new(),
            client_entry_origin: HashMap::new(),
            aliases: AliasMap::new(),
            virtual_modules: VirtualModuleRegistry::new(),
            injected_routes: InjectedRouteList::new(),
            client_entries: ClientEntryList::new(),
        }
    }

    /// Fold one plugin's raw `setup` output into the accumulator. The
    /// `plugin` field on `output` identifies the registering plugin
    /// in any diagnostic emitted from here.
    pub(crate) fn ingest(
        &mut self,
        output: RawPluginSetupOutput,
    ) -> Result<(), SetupRegistryError> {
        for raw in output.registrations {
            match raw {
                RawSetupRegistration::Alias { from, to } => {
                    let target = resolve_against_root(self.project_root, &to);
                    if let Some((first_plugin, first_target)) = self.alias_origin.get(&from) {
                        if first_target != &target {
                            return Err(SetupRegistryError::AliasConflict {
                                from: from.clone(),
                                first_plugin: first_plugin.clone(),
                                first_target: first_target.display().to_string(),
                                second_plugin: output.plugin.clone(),
                                second_target: target.display().to_string(),
                            });
                        }
                        // Same plugin re-registering the same target
                        // is a no-op rather than an error — keeps the
                        // user's mental model "duplicate registration
                        // is fine as long as it's consistent".
                        continue;
                    }
                    self.alias_origin
                        .insert(from.clone(), (output.plugin.clone(), target.clone()));
                    self.aliases.insert(
                        from,
                        AliasEntry {
                            target,
                            plugin: output.plugin.clone(),
                        },
                    );
                }
                RawSetupRegistration::VirtualModule {
                    specifier,
                    loader_id,
                } => {
                    if let Some(first_plugin) = self.virtual_origin.get(&specifier) {
                        if first_plugin != &output.plugin {
                            return Err(SetupRegistryError::VirtualModuleConflict {
                                specifier: specifier.clone(),
                                first_plugin: first_plugin.clone(),
                                second_plugin: output.plugin.clone(),
                            });
                        }
                        // Same plugin re-registering the same specifier
                        // overwrites the loader_id (last write wins).
                    }
                    self.virtual_origin
                        .insert(specifier.clone(), output.plugin.clone());
                    self.virtual_modules.insert(VirtualModuleEntry {
                        specifier,
                        plugin: output.plugin.clone(),
                        loader_id,
                    });
                }
                RawSetupRegistration::InjectRoute {
                    pattern,
                    entrypoint,
                    prerender,
                } => {
                    // #1193: `injectRoute` is now accepted in BOTH commands.
                    // In `dev` it feeds the dev router's synthetic routes
                    // (unchanged); in `build` it contributes a package-owned
                    // build route that the build materialiser prerenders. The
                    // old unconditional `InjectRouteInBuildMode` rejection is
                    // therefore gone — the variant is retained (see its doc)
                    // for a genuinely-unsupported shape a future wave may
                    // reintroduce, but the build path no longer hard-errors on
                    // a plain prerenderable route.
                    let resolved = resolve_against_root(self.project_root, &entrypoint);
                    if let Some((first_plugin, first_entrypoint)) = self.route_origin.get(&pattern)
                    {
                        if first_plugin != &output.plugin || first_entrypoint != &resolved {
                            // A different plugin claiming the same pattern, OR
                            // the same plugin re-registering the pattern with a
                            // DIFFERENT entrypoint, are both real setup bugs —
                            // surface them rather than silently dropping one.
                            // Mirrors the Alias arm's same-pattern/different-
                            // target conflict. This is the package-vs-package
                            // collision guard (hard error in both modes);
                            // user-`pages/`-wins precedence is enforced later,
                            // pre-scan, by the build materialiser.
                            return Err(SetupRegistryError::InjectRouteConflict {
                                pattern: pattern.clone(),
                                first_plugin: first_plugin.clone(),
                                first_entrypoint: first_entrypoint.display().to_string(),
                                second_plugin: output.plugin.clone(),
                                second_entrypoint: resolved.display().to_string(),
                            });
                        }
                        // Same plugin, same pattern, same resolved entrypoint:
                        // an idempotent no-op (keep-first, mirrors the Alias path).
                        continue;
                    }
                    self.route_origin
                        .insert(pattern.clone(), (output.plugin.clone(), resolved.clone()));
                    self.injected_routes.push(InjectedRoute {
                        pattern,
                        entrypoint: resolved,
                        plugin: output.plugin.clone(),
                        prerender,
                    });
                }
                RawSetupRegistration::AddClientEntry { entrypoint } => {
                    // #1196: register a package-owned client-side side-effect entry.
                    // The entry name is derived from the filename (stem minus `.client`)
                    // using the same convention as `discover_client_scripts`.
                    let resolved = resolve_against_root(self.project_root, &entrypoint);
                    let entry_name = resolved
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| {
                            // Strip the `.client` infix from the stem so
                            // `search-widget.client.ts` → `search-widget`.
                            s.strip_suffix(".client").unwrap_or(s).to_string()
                        })
                        .unwrap_or_else(|| entrypoint.clone());
                    if let Some((first_plugin, first_path)) =
                        self.client_entry_origin.get(&entry_name)
                    {
                        if first_path != &resolved {
                            return Err(SetupRegistryError::ClientEntryConflict {
                                entry_name: entry_name.clone(),
                                first_plugin: first_plugin.clone(),
                                first_entrypoint: first_path.display().to_string(),
                                second_plugin: output.plugin.clone(),
                                second_entrypoint: resolved.display().to_string(),
                            });
                        }
                        // Same plugin, same resolved path — idempotent no-op.
                        continue;
                    }
                    self.client_entry_origin.insert(
                        entry_name.clone(),
                        (output.plugin.clone(), resolved.clone()),
                    );
                    self.client_entries.push(ClientEntry {
                        entry_name,
                        entrypoint: resolved,
                        plugin: output.plugin.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    pub(crate) fn finish(
        self,
    ) -> (
        AliasMap,
        VirtualModuleRegistry,
        InjectedRouteList,
        ClientEntryList,
    ) {
        (
            self.aliases,
            self.virtual_modules,
            self.injected_routes,
            self.client_entries,
        )
    }
}

/// Resolve a user-supplied path relative to the project root. Absolute
/// inputs are passed through unchanged. The path is **not** required
/// to exist on disk yet — Wave 2 consumers may decide whether
/// missing-target is an error at use time.
fn resolve_against_root(project_root: &Path, raw: &str) -> PathBuf {
    let candidate = PathBuf::from(raw);
    if candidate.is_absolute() {
        candidate
    } else {
        project_root.join(candidate)
    }
}

/// Bundle of the registries produced by one `setup` round.
/// Returned by [`crate::PluginHost::run_setup`] so callers can pass
/// the whole thing to downstream consumers without juggling individual
/// handles.
#[derive(Debug, Clone, Default)]
pub struct SetupRegistries {
    pub aliases: AliasMap,
    pub virtual_modules: VirtualModuleRegistry,
    pub injected_routes: InjectedRouteList,
    /// Package-owned client-side side-effect entries registered via
    /// `addClientEntry` (#1196).
    pub client_entries: ClientEntryList,
}

impl SetupRegistries {
    pub fn empty() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_alias(plugin: &str, from: &str, to: &str) -> RawPluginSetupOutput {
        RawPluginSetupOutput {
            plugin: plugin.into(),
            registrations: vec![RawSetupRegistration::Alias {
                from: from.into(),
                to: to.into(),
            }],
        }
    }

    fn raw_vm(plugin: &str, specifier: &str, loader_id: &str) -> RawPluginSetupOutput {
        RawPluginSetupOutput {
            plugin: plugin.into(),
            registrations: vec![RawSetupRegistration::VirtualModule {
                specifier: specifier.into(),
                loader_id: VirtualLoaderId(loader_id.into()),
            }],
        }
    }

    fn raw_route(plugin: &str, pattern: &str, entrypoint: &str) -> RawPluginSetupOutput {
        RawPluginSetupOutput {
            plugin: plugin.into(),
            registrations: vec![RawSetupRegistration::InjectRoute {
                pattern: pattern.into(),
                entrypoint: entrypoint.into(),
                prerender: None,
            }],
        }
    }

    #[test]
    fn alias_resolves_relative_to_project_root() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Build);
        acc.ingest(raw_alias("p", "@/foo", "./src/foo.tsx"))
            .unwrap();
        let (aliases, _, _, _) = acc.finish();
        let entry = aliases.get("@/foo").unwrap();
        assert_eq!(entry.target, PathBuf::from("/proj/src/foo.tsx"));
        assert_eq!(entry.plugin, "p");
    }

    #[test]
    fn alias_is_exact_match_only() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Build);
        acc.ingest(raw_alias("p", "@/foo", "./src/foo.tsx"))
            .unwrap();
        let (aliases, _, _, _) = acc.finish();
        // Exact match works.
        assert!(aliases.contains("@/foo"));
        // Prefix-style imports do NOT match.
        assert!(!aliases.contains("@/foo/bar"));
        assert!(!aliases.contains("@/foobar"));
    }

    #[test]
    fn alias_conflict_names_both_plugins() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Dev);
        acc.ingest(raw_alias("a", "@/x", "./src/a.tsx")).unwrap();
        let err = acc
            .ingest(raw_alias("b", "@/x", "./src/b.tsx"))
            .expect_err("conflicting alias must error");
        match err {
            SetupRegistryError::AliasConflict {
                first_plugin,
                second_plugin,
                from,
                ..
            } => {
                assert_eq!(from, "@/x");
                assert_eq!(first_plugin, "a");
                assert_eq!(second_plugin, "b");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn same_plugin_same_alias_is_idempotent() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Dev);
        acc.ingest(raw_alias("a", "@/x", "./src/a.tsx")).unwrap();
        // Re-registering with the same target from the same plugin is
        // not an error — feels right for a plugin that re-runs its
        // own setup on a config reload.
        acc.ingest(raw_alias("a", "@/x", "./src/a.tsx")).unwrap();
    }

    #[test]
    fn same_plugin_same_route_is_idempotent() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Dev);
        acc.ingest(raw_route("a", "/dev/x", "./scripts/x.ts"))
            .unwrap();
        // Re-registering the same pattern from the same plugin is a
        // no-op (keep-first) — mirrors same_plugin_same_alias_is_idempotent.
        acc.ingest(raw_route("a", "/dev/x", "./scripts/x.ts"))
            .unwrap();
        let (_, _, routes, _) = acc.finish();
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn virtual_module_conflict_names_both_plugins() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Build);
        acc.ingest(raw_vm("a", "virtual:x", "h0")).unwrap();
        let err = acc
            .ingest(raw_vm("b", "virtual:x", "h1"))
            .expect_err("conflicting virtual module must error");
        match err {
            SetupRegistryError::VirtualModuleConflict {
                first_plugin,
                second_plugin,
                specifier,
            } => {
                assert_eq!(specifier, "virtual:x");
                assert_eq!(first_plugin, "a");
                assert_eq!(second_plugin, "b");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn inject_route_in_build_mode_registers() {
        // #1193: `injectRoute` is now ACCEPTED during a build — it
        // contributes a package-owned build route the materialiser
        // prerenders, rather than the pre-#1193 dev-only hard error.
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Build);
        acc.ingest(raw_route("a", "/preset-page", "./scripts/x.ts"))
            .expect("injectRoute during build must be accepted (#1193)");
        let (_, _, routes, _) = acc.finish();
        assert_eq!(routes.len(), 1);
        let r = &routes.as_slice()[0];
        assert_eq!(r.pattern, "/preset-page");
        assert_eq!(r.entrypoint, PathBuf::from("/proj/scripts/x.ts"));
        assert_eq!(r.plugin, "a");
        // No `prerender` option supplied → None → build defaults to SSG.
        assert_eq!(r.prerender, None);
    }

    #[test]
    fn inject_route_carries_prerender_hint() {
        // The optional `{ prerender }` arg threads through to the
        // registered route so the build materialiser can inline an
        // explicit `export const prerender = …`.
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Build);
        acc.ingest(RawPluginSetupOutput {
            plugin: "a".into(),
            registrations: vec![RawSetupRegistration::InjectRoute {
                pattern: "/ssr-page".into(),
                entrypoint: "./scripts/ssr.ts".into(),
                prerender: Some(false),
            }],
        })
        .expect("injectRoute with prerender hint must be accepted");
        let (_, _, routes, _) = acc.finish();
        assert_eq!(routes.as_slice()[0].prerender, Some(false));
    }

    #[test]
    fn inject_route_in_dev_mode_registers() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Dev);
        acc.ingest(raw_route("a", "/dev/x", "./scripts/x.ts"))
            .unwrap();
        let (_, _, routes, _) = acc.finish();
        assert_eq!(routes.len(), 1);
        let r = &routes.as_slice()[0];
        assert_eq!(r.pattern, "/dev/x");
        assert_eq!(r.entrypoint, PathBuf::from("/proj/scripts/x.ts"));
        assert_eq!(r.plugin, "a");
    }

    #[test]
    fn inject_route_conflict_names_both_plugins() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Dev);
        acc.ingest(raw_route("a", "/dev/x", "./scripts/a.ts"))
            .unwrap();
        let err = acc
            .ingest(raw_route("b", "/dev/x", "./scripts/b.ts"))
            .expect_err("conflicting injectRoute must error");
        match err {
            SetupRegistryError::InjectRouteConflict {
                first_plugin,
                second_plugin,
                pattern,
                ..
            } => {
                assert_eq!(pattern, "/dev/x");
                assert_eq!(first_plugin, "a");
                assert_eq!(second_plugin, "b");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn same_plugin_same_route_different_entrypoint_errors() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Dev);
        acc.ingest(raw_route("a", "/dev/x", "./scripts/a.ts"))
            .unwrap();
        // Same plugin, same pattern, but a DIFFERENT entrypoint is a real
        // setup bug — silently keeping the first would hide it.
        let err = acc
            .ingest(raw_route("a", "/dev/x", "./scripts/b.ts"))
            .expect_err(
                "same plugin re-registering a pattern with a different entrypoint must error",
            );
        match err {
            SetupRegistryError::InjectRouteConflict {
                pattern,
                first_plugin,
                first_entrypoint,
                second_plugin,
                second_entrypoint,
            } => {
                assert_eq!(pattern, "/dev/x");
                assert_eq!(first_plugin, "a");
                assert_eq!(second_plugin, "a");
                // resolve_against_root joins verbatim, so the `./` prefix is
                // preserved in the resolved path.
                assert_eq!(first_entrypoint, "/proj/./scripts/a.ts");
                assert_eq!(second_entrypoint, "/proj/./scripts/b.ts");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn declaration_order_is_preserved_for_injected_routes() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Dev);
        acc.ingest(raw_route("a", "/r1", "./a.ts")).unwrap();
        acc.ingest(raw_route("b", "/r2", "./b.ts")).unwrap();
        acc.ingest(raw_route("c", "/r3", "./c.ts")).unwrap();
        let (_, _, routes, _) = acc.finish();
        let patterns: Vec<&str> = routes.iter().map(|r| r.pattern.as_str()).collect();
        assert_eq!(patterns, vec!["/r1", "/r2", "/r3"]);
    }

    // --- addClientEntry tests (#1196) ------------------------------------------

    fn raw_client_entry(plugin: &str, entrypoint: &str) -> RawPluginSetupOutput {
        RawPluginSetupOutput {
            plugin: plugin.into(),
            registrations: vec![RawSetupRegistration::AddClientEntry {
                entrypoint: entrypoint.into(),
            }],
        }
    }

    #[test]
    fn add_client_entry_resolves_relative_path() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Build);
        acc.ingest(raw_client_entry("p", "./src/analytics.client.ts"))
            .unwrap();
        let (_, _, _, entries) = acc.finish();
        assert_eq!(entries.len(), 1);
        let e = &entries.as_slice()[0];
        assert_eq!(e.entry_name, "analytics");
        assert_eq!(e.entrypoint, PathBuf::from("/proj/src/analytics.client.ts"));
        assert_eq!(e.plugin, "p");
    }

    #[test]
    fn add_client_entry_same_plugin_same_path_is_idempotent() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Build);
        acc.ingest(raw_client_entry("p", "./widgets/search.client.ts"))
            .unwrap();
        acc.ingest(raw_client_entry("p", "./widgets/search.client.ts"))
            .unwrap();
        let (_, _, _, entries) = acc.finish();
        assert_eq!(entries.len(), 1, "duplicate same-path should be a no-op");
    }

    #[test]
    fn add_client_entry_conflict_different_entrypoints() {
        let root = PathBuf::from("/proj");
        let mut acc = PluginSetupAccumulator::new(&root, SetupCommand::Build);
        acc.ingest(raw_client_entry("a", "./a/search.client.ts"))
            .unwrap();
        let err = acc
            .ingest(raw_client_entry("b", "./b/search.client.ts"))
            .unwrap_err();
        match err {
            SetupRegistryError::ClientEntryConflict {
                entry_name,
                first_plugin,
                second_plugin,
                ..
            } => {
                assert_eq!(entry_name, "search");
                assert_eq!(first_plugin, "a");
                assert_eq!(second_plugin, "b");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }
}
