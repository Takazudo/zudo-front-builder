//! Shared helpers for the plugin lifecycle (Sub 3 / #108).
//!
//! Both `zfb build` and `zfb dev` need to spawn the long-lived plugin
//! host subprocess if (and only if) the loaded `Config.plugins` list
//! contains entries with a resolved module specifier — the JSON config
//! path leaves `resolved_module` as `None` because bare-specifier Node
//! module resolution is not available there. A `None` entry is treated
//! as "no plugin module" and is silently skipped here.

use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::config::Config;
use zfb_build::{
    DevRegisterContext, DevRequest, DevResponse, PluginHost, PluginSpec,
};
use zfb_server::{
    DevMiddlewareDispatcher, DevMiddlewareSet, PluginDispatchError, PluginDispatchOutcome,
    PluginRegistration, PluginRequest, PluginResponse, PluginResponseEncoding,
};

/// Convert `Config.plugins` into the wire shape the plugin host wants.
/// Drops entries without a resolved module specifier (the JSON-config
/// path produces those, and there's no path-resolution context here to
/// rescue them). Surfaces a warning so a typo in `zfb.config.json`
/// doesn't fall on the floor silently.
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
    let host = PluginHost::spawn(specs, None)
        .await
        .context("plugin lifecycle: failed to spawn the plugin host")?;
    Ok(Some(host))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PluginConfig;

    fn cfg_with_plugins(entries: Vec<PluginConfig>) -> Config {
        let mut c = Config::default();
        c.plugins = entries;
        c
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
