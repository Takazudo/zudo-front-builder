//! Shared helpers for the plugin lifecycle (Sub 3 / #108).
//!
//! Both `zfb build` and `zfb dev` need to spawn the long-lived plugin
//! host subprocess if (and only if) the loaded `Config.plugins` list
//! contains entries with a resolved module specifier — the JSON config
//! path leaves `resolved_module` as `None` because bare-specifier Node
//! module resolution is not available there. A `None` entry is treated
//! as "no plugin module" and is silently skipped here.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::config::Config;
use zfb_build::{
    DevRegisterContext, DevRequest, DevResponse, PluginHost, PluginSpec, SetupCommand,
};
use zfb_server::{
    DevMiddlewareDispatcher, DevMiddlewareSet, PluginDispatchError, PluginDispatchOutcome,
    PluginRegistration, PluginRequest, PluginResponse, PluginResponseEncoding,
};

/// Convert `Config.plugins` into the wire shape the plugin host wants.
/// Drops entries without a resolved module specifier **silently** — the
/// `filter_map` + `?` skips them with zero diagnostics. Upstream loaders
/// (`resolve_json_plugin_modules` in config.rs and the TS evaluator) are
/// responsible for hard-erroring on unresolvable plugins (#211), so a
/// `None` resolved_module reaching here is effectively unreachable for
/// config-declared plugins today.
fn build_plugin_specs(config: &Config) -> Vec<PluginSpec> {
    config
        .plugins
        .iter()
        .filter_map(|entry| {
            let module = entry.resolved_module.as_ref()?.clone();
            Some(PluginSpec {
                name: entry.name.clone(),
                module,
                options: entry.options.clone(),
            })
        })
        .collect()
}

/// Spawn the plugin host subprocess if the config declares any
/// resolved plugins. Returns `Ok(None)` for plugin-less projects so
/// callers don't have to special-case the "no plugins" path.
pub async fn maybe_spawn_host(config: &Config) -> Result<Option<PluginHost>> {
    let specs = build_plugin_specs(config);
    if specs.is_empty() {
        return Ok(None);
    }
    let host =
        PluginHost::spawn_with_timeout(specs, None, config.plugin_hook_timeout_secs)
            .await
            .context("plugin lifecycle: failed to spawn the plugin host")?;
    Ok(Some(host))
}

/// Outputs produced by the shared plugin-setup phase:
/// [`run_plugin_setup`] → setup → virtual-module prefetch → alias derivation.
///
/// Both `zfb build` and `zfb dev` consume this struct; they then diverge to
/// build their own mode-specific wrappers (islands config, dev middleware,
/// injected-route set, etc.) from the data it carries.
///
/// Gated on `embed_v8` because [`v8_plugin_hooks`] requires the V8 render
/// host to be compiled in.
#[cfg(feature = "embed_v8")]
pub(crate) struct PluginSetupResult {
    /// V8 plugin-registry hooks derived from `setup_registries` +
    /// `virtual_sources`.  Passed to the embedded V8 host so the runtime
    /// resolver honours plugin-registered aliases and virtual modules (#260).
    pub(crate) v8_plugin_hooks: zfb_render::PluginRegistryHooks,

    /// `(from, to)` alias pairs for the main bundler's esbuild invocation
    /// (#268).  Derived from `setup_registries.aliases`.
    pub(crate) plugin_alias_entries: Vec<(String, String)>,

    /// `(specifier, source)` virtual-module pairs for the main bundler (#268).
    /// Derived from `virtual_sources`.
    pub(crate) plugin_virtual_modules: Vec<(String, String)>,

    /// Raw setup registries — kept alive so the V8 host's borrow of aliases /
    /// virtual-module entries (via hook entries that hold `Arc` handles) stays
    /// valid for the duration of the command.
    ///
    /// For `zfb dev` this also carries `injected_routes` and is used to build
    /// the `InjectedRouteSet`.  For `zfb build` injected routes are rejected
    /// by the accumulator at `setup` time, so the vec is always empty on the
    /// build path.
    pub(crate) setup_registries: zfb_build::SetupRegistries,
}

/// Run the shared plugin lifecycle setup phase:
/// spawn → `setup` → virtual-module prefetch loop → alias/virtual-module
/// derivation → `translate_setup_registries_to_hooks`.
///
/// ## Per-command differences (explicit)
///
/// | Step | `zfb build` | `zfb dev` |
/// |---|---|---|
/// | `SetupCommand` variant | [`SetupCommand::Build`] | [`SetupCommand::Dev`] |
///
/// All other steps are byte-identical.  The caller decides which variant to
/// pass.  Steps that differ further (preBuild out_dir, islands config, dev
/// middleware, injected routes) happen after this function returns.
///
/// Gated on `embed_v8` because the returned [`PluginSetupResult::v8_plugin_hooks`]
/// requires the embedded V8 render host.
#[cfg(feature = "embed_v8")]
pub(crate) async fn run_plugin_setup(
    plugin_host: &Option<PluginHost>,
    project_root: &Path,
    config: &Config,
    // Per-command difference: SetupCommand::Build for zfb build,
    // SetupCommand::Dev for zfb dev. Explicit at every call site.
    setup_command: SetupCommand,
) -> Result<PluginSetupResult> {
    // #255 — run the `setup` hook once.  The returned registries (aliases,
    // virtual modules, injected routes) are consumed downstream by the
    // V8 host resolver (#260) and the esbuild resolver (#261).
    // For `zfb build`, `injectRoute` is rejected by the accumulator with
    // `InjectRouteInBuildMode`; for `zfb dev` it populates injected_routes.
    let setup_registries = if let Some(host) = plugin_host.as_ref() {
        let cfg_json = serde_json::to_value(config)
            .context("plugin lifecycle: serialise config for setup ctx")?;
        host.run_setup(project_root, setup_command, &cfg_json)
            .await
            .map_err(zfb_build::annotate_with_plugin_error)
            .context("setup lifecycle hook")?
    } else {
        zfb_build::SetupRegistries::empty()
    };

    // #261 / #260 — pre-fetch all virtual-module sources once.  They feed
    // BOTH the islands esbuild plugin config (#261) and the V8 host's plugin
    // hooks (#260), so a single async loop here invokes each loader exactly
    // once per build/dev-boot.  `invoke_virtual_loader` is async; the plain
    // strings are then forwarded into the synchronous bundler path.
    let mut virtual_sources: BTreeMap<String, String> = BTreeMap::new();
    if let Some(host) = plugin_host.as_ref() {
        for (specifier, vm_entry) in setup_registries.virtual_modules.iter() {
            match host.invoke_virtual_loader(&vm_entry.loader_id).await {
                Ok(source) => {
                    virtual_sources.insert(specifier.clone(), source);
                }
                Err(e) => {
                    return Err(zfb_build::annotate_with_plugin_error(e)).with_context(|| {
                        format!(
                            "plugin lifecycle: failed to load virtual module \
                             `{specifier}` (plugin: `{plugin}`)",
                            plugin = vm_entry.plugin
                        )
                    });
                }
            }
        }
    }

    // #260 — translate registries + pre-fetched sources into the loader-side
    // PluginRegistryHooks the V8 host consumes.
    let v8_plugin_hooks = crate::v8_host_adapter::translate_setup_registries_to_hooks(
        &setup_registries,
        &virtual_sources,
    );

    // #268 — derive `(from, to)` alias / `(specifier, source)` virtual-module
    // lists for the main bundler.  These are cheap clones of heap data already
    // owned by `setup_registries` / `virtual_sources`.
    let plugin_alias_entries: Vec<(String, String)> = setup_registries
        .aliases
        .iter()
        .map(|(from, entry)| (from.clone(), entry.target.to_string_lossy().into_owned()))
        .collect();
    let plugin_virtual_modules: Vec<(String, String)> = virtual_sources
        .iter()
        .map(|(spec, src)| (spec.clone(), src.clone()))
        .collect();

    Ok(PluginSetupResult {
        v8_plugin_hooks,
        plugin_alias_entries,
        plugin_virtual_modules,
        setup_registries,
    })
}

/// Adapter that bridges a [`PluginHost`] to the dev-server's
/// [`DevMiddlewareDispatcher`] trait so the server doesn't need to
/// know about the build crate's plugin host directly.
struct HostDispatcher {
    host: PluginHost,
}

#[async_trait]
impl DevMiddlewareDispatcher for HostDispatcher {
    async fn dispatch(
        &self,
        handler_id: &str,
        request: PluginRequest,
    ) -> Result<PluginDispatchOutcome, PluginDispatchError> {
        let req = DevRequest {
            method: request.method,
            url: request.url,
            headers: request.headers,
            body: request.body,
        };
        let resp: DevResponse = self
            .host
            .invoke_dev_handler(handler_id, req)
            .await
            .map_err(|err| {
                let pe = zfb_build::extract_plugin_error(&err);
                let plugin = pe
                    .map(|p| p.plugin.clone())
                    .unwrap_or_else(|| "(host)".to_string());
                let message = pe
                    .map(|p| p.message.clone())
                    .unwrap_or_else(|| format!("{err:#}"));
                PluginDispatchError { plugin, message }
            })?;
        if resp.passthrough {
            return Ok(PluginDispatchOutcome::Passthrough);
        }
        let encoding = PluginResponseEncoding::parse(&resp.body_encoding);
        Ok(PluginDispatchOutcome::Response(PluginResponse {
            status: resp.status,
            headers: resp.headers,
            body: resp.body,
            body_encoding: encoding,
        }))
    }
}

/// Register every plugin's `devMiddleware` hook against the supplied
/// host and produce the [`DevMiddlewareSet`] the dev server consumes.
/// Returns `Ok(None)` when no plugin declared the hook.
pub async fn build_dev_middleware_set(
    host: &PluginHost,
    project_root: &std::path::Path,
    config: &Config,
) -> Result<Option<DevMiddlewareSet>> {
    let ctx = DevRegisterContext {
        project_root: project_root.to_path_buf(),
        config: serde_json::to_value(config)
            .context("plugin lifecycle: serialise config for devMiddleware ctx")?,
    };
    let registrations = host
        .register_dev_middlewares(&ctx)
        .await
        .map_err(zfb_build::annotate_with_plugin_error)
        .context("devMiddleware lifecycle hook")?;
    if registrations.is_empty() {
        return Ok(None);
    }
    let registrations: Vec<PluginRegistration> = registrations
        .into_iter()
        .map(|r| PluginRegistration {
            path: r.path,
            handler_id: r.handler_id,
            plugin: r.plugin,
        })
        .collect();
    let dispatcher: Arc<dyn DevMiddlewareDispatcher> = Arc::new(HostDispatcher {
        host: host.clone(),
    });
    Ok(Some(DevMiddlewareSet {
        registrations: Arc::new(registrations),
        dispatcher,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PluginConfig;

    fn cfg_with_plugins(entries: Vec<PluginConfig>) -> Config {
        Config { plugins: entries, ..Config::default() }
    }

    #[test]
    fn unresolved_plugins_are_filtered_out() {
        let c = cfg_with_plugins(vec![
            PluginConfig {
                name: "a".into(),
                options: serde_json::json!({}),
                resolved_module: None,
            },
            PluginConfig {
                name: "b".into(),
                options: serde_json::json!({"x": 1}),
                resolved_module: Some("file:///abs/b.mjs".into()),
            },
        ]);
        let specs = build_plugin_specs(&c);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "b");
        assert_eq!(specs[0].module, "file:///abs/b.mjs");
        assert_eq!(specs[0].options, serde_json::json!({"x": 1}));
    }

    #[tokio::test]
    async fn maybe_spawn_host_is_none_for_pluginless_config() {
        let c = Config::default();
        let host = maybe_spawn_host(&c).await.unwrap();
        assert!(host.is_none());
    }
}
