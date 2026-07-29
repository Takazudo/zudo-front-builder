//! Dev-session live-refresh state for plugin virtual modules (issue #2168,
//! epic #2166 "Plugin Watch Hook", Sub 2 "Dev Refresh State").
//!
//! `zfb dev` boots the plugin host once, runs its `setup` hook, and
//! pre-fetches every registered virtual module's source text exactly once
//! (`PluginHost::invoke_virtual_loader`, unforced — see `zfb`'s
//! `commands/plugins.rs::run_plugin_setup`). Before this issue, that
//! prefetch result was frozen into THREE separate copies scattered across
//! `crates/zfb/src/commands/dev.rs`: the main bundler's rebuild inputs, the
//! restarted V8 host's plugin-registry hooks, and the islands/CSS rebuild
//! closures. A loader whose output depends on a file it reads directly
//! (issue #2167's `addVirtualModule(..., { watchFiles })` option) had no way
//! to get fresh output back in front of any of the three once that file
//! changed — updating only one copy would have shipped a split-brain source
//! between the bundle and the V8 host / islands scan.
//!
//! This module is the shared state those three consumers now read from, and
//! the (not yet wired to a live watcher) function that refreshes it:
//!
//! - [`PluginVirtualModuleStore`] — ONE interior-mutable `specifier ->
//!   source` map. Seed it once at boot from the setup prefetch; every
//!   consumer takes a fresh [`PluginVirtualModuleStore::snapshot`] /
//!   [`PluginVirtualModuleStore::snapshot_pairs`] at use time instead of
//!   holding its own frozen copy.
//! - [`PluginWatchOwnership`] — `normalized watched path -> [(specifier,
//!   loader_id)]`, built once from the SAME [`VirtualModuleRegistry`] the
//!   setup hook returned (plugin registrations are frozen after `setup`
//!   runs, so this never needs to be rebuilt mid-session).
//! - [`PluginRefreshState`] bundles both maps with a [`PluginHost`] clone
//!   and exposes [`PluginRefreshState::refresh`], the actual "changed paths
//!   -> re-invoke the owning loaders -> publish" function.
//!
//! [`PluginRefreshState::refresh`] is called from the live watcher loop
//! since issue #2169: the orchestrator's pre-tick hook
//! (`crate::orchestrator::PreTickRefreshHook`, installed by `zfb`'s
//! `commands/dev.rs`) awaits it with the batch's changed paths before
//! dispatching any tick that touches the plugin watch-file set, so the
//! refreshed source ships in the same tick as the watched file's change.
//!
//! ## Atomicity contract
//!
//! [`PluginRefreshState::refresh`] awaits every affected loader's
//! [`PluginHost::invoke_virtual_loader_forced`] into a temporary map FIRST,
//! and calls [`PluginVirtualModuleStore::publish`] only once every one of
//! them has succeeded. A single failing loader in the batch means NONE of
//! the batch's specifiers are published — the store keeps serving its
//! last-good memo for all of them and the failure is logged.
//!
//! A discarded batch is not lost, though: every specifier the failed batch
//! touched (including ones whose OWN loader succeeded — only a sibling
//! failed) is queued in an internal pending-retry set and folded into the
//! NEXT `refresh` call's affected set automatically, regardless of what
//! that call's own `changed_paths` name. Without this, a recovery event
//! naming only the file that failed would retry just that one specifier
//! and silently lose the sibling's already-fetched fresh content forever
//! (codex review, issue #2168).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tracing::warn;

use crate::plugin_registries::{VirtualLoaderId, VirtualModuleRegistry};
use crate::plugin_runner::PluginHost;
use crate::policy::RawImportInvalidation;

/// Shared, interior-mutable `specifier -> source text` store for plugin
/// virtual modules (issue #2168). Cheap to clone — every clone shares the
/// same underlying map, which is exactly what lets unrelated consumers in
/// `zfb`'s `commands/dev.rs` stay in sync without threading a single owner
/// through all of them.
#[derive(Debug, Clone, Default)]
pub struct PluginVirtualModuleStore {
    sources: Arc<RwLock<BTreeMap<String, String>>>,
}

impl PluginVirtualModuleStore {
    /// Seed the store at boot from the setup-phase prefetch — `(specifier,
    /// source)` pairs already fetched via the unforced
    /// `PluginHost::invoke_virtual_loader`.
    pub fn seeded(initial: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            sources: Arc::new(RwLock::new(initial.into_iter().collect())),
        }
    }

    /// Full snapshot, keyed by specifier. Reads the lock once and clones —
    /// callers must never hold the returned lock guard itself across an
    /// `.await` (this method never hands one out).
    pub fn snapshot(&self) -> BTreeMap<String, String> {
        self.sources
            .read()
            .map(|sources| sources.clone())
            .unwrap_or_default()
    }

    /// Snapshot as `(specifier, source)` pairs, sorted by specifier (the
    /// backing map is a `BTreeMap`) — the exact shape
    /// `BundlerInput::plugin_virtual_modules` / `IslandsPluginConfig::virtual_modules`
    /// (both in the `zfb` crate) want.
    pub fn snapshot_pairs(&self) -> Vec<(String, String)> {
        self.snapshot().into_iter().collect()
    }

    /// Look up one specifier's current source text.
    pub fn get(&self, specifier: &str) -> Option<String> {
        self.sources
            .read()
            .ok()
            .and_then(|sources| sources.get(specifier).cloned())
    }

    /// Atomically merge `updates` into the store — a single write-lock
    /// acquisition inserts every key. [`PluginRefreshState::refresh`] only
    /// calls this once every loader in a refresh batch has already
    /// succeeded (see the module doc's atomicity contract); exposed
    /// directly too, for a caller that already has verified authoritative
    /// fresh content (and for tests that want to simulate a completed
    /// refresh without spawning a real plugin host).
    pub fn publish(&self, updates: impl IntoIterator<Item = (String, String)>) {
        if let Ok(mut sources) = self.sources.write() {
            sources.extend(updates);
        }
    }
}

/// `normalized watched path -> [(specifier, loader_id)]` (issue #2168).
///
/// Built once from a [`VirtualModuleRegistry`] snapshot — plugin
/// registrations are frozen once `setup` returns, so there is no later
/// re-registration this map would need to observe. Multiple loaders may
/// watch the same file (two separate `addVirtualModule` calls both
/// declaring the same `watchFiles` entry) and a single loader may watch
/// many files — both fold naturally into the same `Vec` / are reachable via
/// any of their declared paths.
///
/// Keys are alias-expanded with [`RawImportInvalidation::path_aliases`], the
/// SAME lexical/canonical alias expansion `zfb-build`'s other dynamic-watch
/// registries use, so a changed-path lookup succeeds regardless of whether
/// the watcher reports the canonical or the lexical form (macOS FSEvents
/// reports `/private/var/...` where the registered path is `/var/...`, or
/// vice versa).
#[derive(Debug, Clone, Default)]
pub struct PluginWatchOwnership {
    by_path: BTreeMap<PathBuf, Vec<(String, VirtualLoaderId)>>,
}

impl PluginWatchOwnership {
    /// Build the ownership map from every registered virtual module's
    /// `watch_files` (issue #2167's `addVirtualModule(..., { watchFiles })`
    /// option). A loader with no `watch_files` contributes nothing — its
    /// source can only change via a fresh `zfb dev` boot, same as today.
    pub fn from_registry(registry: &VirtualModuleRegistry) -> Self {
        let mut by_path: BTreeMap<PathBuf, Vec<(String, VirtualLoaderId)>> = BTreeMap::new();
        for (specifier, entry) in registry.iter() {
            for watch_file in &entry.watch_files {
                for alias in RawImportInvalidation::path_aliases(watch_file) {
                    by_path
                        .entry(alias)
                        .or_default()
                        .push((specifier.clone(), entry.loader_id.clone()));
                }
            }
        }
        Self { by_path }
    }

    /// Every registered watch-file alias — feeds
    /// `RawImportInvalidation::replace_plugin_watch_files` (issue #2168),
    /// populated once at boot alongside this map.
    pub fn watched_paths(&self) -> BTreeSet<PathBuf> {
        self.by_path.keys().cloned().collect()
    }

    /// `true` when no registered loader declared any `watchFiles`.
    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }

    /// Resolve `changed_paths` to the distinct `(specifier, loader_id)`
    /// pairs a refresh must re-invoke — deduplicated by specifier (an
    /// overlapping declaration or a one-loader/many-files edit can hand the
    /// same specifier back from more than one input path or alias).
    pub fn affected(&self, changed_paths: &[PathBuf]) -> Vec<(String, VirtualLoaderId)> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for path in changed_paths {
            for alias in RawImportInvalidation::path_aliases(path) {
                let Some(owners) = self.by_path.get(&alias) else {
                    continue;
                };
                for (specifier, loader_id) in owners {
                    if seen.insert(specifier.clone()) {
                        out.push((specifier.clone(), loader_id.clone()));
                    }
                }
            }
        }
        out
    }
}

/// Result of one [`PluginRefreshState::refresh`] call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginRefreshOutcome {
    /// Specifiers whose source text was just published to the shared
    /// store. Empty when no watched path was touched, or when the batch
    /// failed (see `failed`).
    pub refreshed: Vec<String>,
    /// `(specifier, error message)` for every loader that failed THIS call.
    /// Non-empty implies `refreshed` is empty — a failing loader anywhere
    /// in the batch means nothing in the batch is published (see the
    /// module doc's atomicity contract).
    pub failed: Vec<(String, String)>,
}

impl PluginRefreshOutcome {
    /// Whether this call touched anything at all — `false` for a batch of
    /// changed paths none of which any loader declared as a `watchFiles`
    /// entry (the common case: most watcher ticks are unrelated to any
    /// plugin loader).
    pub fn is_noop(&self) -> bool {
        self.refreshed.is_empty() && self.failed.is_empty()
    }
}

/// Session-lifetime plugin-refresh state (issue #2168): the shared source
/// store, the watch ownership map, a [`PluginHost`] clone (cheap — see
/// [`PluginHost`]'s own doc comment) to invoke loaders with, and a pending-
/// retry set (see [`Self::refresh`]'s doc for why). Threaded into `zfb`'s
/// `DevRebuildInputs` so it is reachable from the same struct the bundler /
/// V8 host / islands rebuild already read their plugin data from.
#[derive(Clone, Default)]
pub struct PluginRefreshState {
    host: Option<PluginHost>,
    ownership: PluginWatchOwnership,
    store: PluginVirtualModuleStore,
    /// Specifiers a PAST batch fetched-but-discarded because a sibling in
    /// the same batch failed (see [`Self::refresh`]'s atomicity note).
    /// `Arc<Mutex<_>>` so every clone of `PluginRefreshState` shares the
    /// same pending set, same as `store`.
    pending: Arc<std::sync::Mutex<BTreeMap<String, VirtualLoaderId>>>,
}

impl PluginRefreshState {
    pub fn new(
        host: Option<PluginHost>,
        ownership: PluginWatchOwnership,
        store: PluginVirtualModuleStore,
    ) -> Self {
        Self {
            host,
            ownership,
            store,
            pending: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
        }
    }

    /// The shared source store every consumer snapshots at use time.
    pub fn store(&self) -> &PluginVirtualModuleStore {
        &self.store
    }

    /// Every registered watch-file alias — feeds
    /// `RawImportInvalidation::replace_plugin_watch_files` at boot.
    pub fn watched_paths(&self) -> BTreeSet<PathBuf> {
        self.ownership.watched_paths()
    }

    /// Re-invoke every loader owning a path in `changed_paths` (PLUS any
    /// specifier a past call fetched-but-discarded — see below), publishing
    /// atomically on full success. A no-op (returns a default/empty
    /// outcome immediately, no host round-trip) when there is no plugin
    /// host (pluginless project) and nothing is affected or pending.
    ///
    /// ## Why a pending-retry set (codex review, issue #2168)
    ///
    /// The atomicity contract discards the WHOLE batch on any failure —
    /// including specifiers whose own loader just succeeded, if a sibling
    /// in the SAME `changed_paths` batch failed. Without carrying that
    /// success forward, it would be lost for good the moment a recovery
    /// event names only the file that failed: `ownership.affected` for
    /// that narrower path set would never mention the sibling that
    /// succeeded before, so its freshly-fetched (but never published)
    /// content would never resurface until its own watched file happens to
    /// change independently. `pending` remembers every specifier a failed
    /// batch touched (not just the ones that failed) so the NEXT call —
    /// whatever triggered it — retries them too, alongside anything the
    /// caller directly asked for this time.
    ///
    /// Called from the live watcher loop via the orchestrator's pre-tick
    /// hook (issue #2169) with each qualifying tick's changed paths — see
    /// the module doc.
    pub async fn refresh(&self, changed_paths: &[PathBuf]) -> PluginRefreshOutcome {
        let mut affected = self.ownership.affected(changed_paths);
        {
            let pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            if !pending.is_empty() {
                let mut seen: BTreeSet<String> = affected.iter().map(|(s, _)| s.clone()).collect();
                for (specifier, loader_id) in pending.iter() {
                    if seen.insert(specifier.clone()) {
                        affected.push((specifier.clone(), loader_id.clone()));
                    }
                }
            }
        }
        if affected.is_empty() {
            return PluginRefreshOutcome::default();
        }
        let Some(host) = self.host.as_ref() else {
            // No plugin host means no loader could have registered
            // `watchFiles` in the first place — `affected` should already
            // have been empty. Defensive: never invoke a loader with no host.
            return PluginRefreshOutcome::default();
        };

        let mut updates: BTreeMap<String, String> = BTreeMap::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        for (specifier, loader_id) in &affected {
            match host.invoke_virtual_loader_forced(loader_id).await {
                Ok(source) => {
                    updates.insert(specifier.clone(), source);
                }
                Err(err) => {
                    failed.push((specifier.clone(), format!("{err:#}")));
                }
            }
        }

        if !failed.is_empty() {
            for (specifier, message) in &failed {
                warn!(
                    site = "PluginRefreshState::refresh",
                    specifier = %specifier,
                    error = %message,
                    "plugin virtual-module reload failed; retaining last-good source \
                     for the whole batch (queued for retry on the next refresh call)"
                );
            }
            // Remember EVERY specifier this attempt touched — not just the
            // ones that failed — so the next call (however narrow its own
            // `changed_paths`) retries the discarded successes too.
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            for (specifier, loader_id) in &affected {
                pending.insert(specifier.clone(), loader_id.clone());
            }
            return PluginRefreshOutcome {
                refreshed: Vec::new(),
                failed,
            };
        }

        // Full success: every specifier just retried (freshly affected AND
        // carried-over-pending alike) is now published — clear it from
        // pending.
        {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            for (specifier, _) in &affected {
                pending.remove(specifier);
            }
        }
        let refreshed: Vec<String> = updates.keys().cloned().collect();
        self.store.publish(updates);
        PluginRefreshOutcome {
            refreshed,
            failed: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_registries::{SetupCommand, SetupRegistries};
    use crate::plugin_runner::PluginSpec;

    // -----------------------------------------------------------------------
    // PluginVirtualModuleStore — pure logic, no plugin host needed.
    // -----------------------------------------------------------------------

    #[test]
    fn store_seed_snapshot_and_publish_roundtrip() {
        let store = PluginVirtualModuleStore::seeded([
            ("virtual:a".to_string(), "v1".to_string()),
            ("virtual:b".to_string(), "v1".to_string()),
        ]);
        assert_eq!(store.get("virtual:a").as_deref(), Some("v1"));
        assert_eq!(
            store.snapshot_pairs(),
            vec![
                ("virtual:a".to_string(), "v1".to_string()),
                ("virtual:b".to_string(), "v1".to_string()),
            ]
        );

        store.publish([("virtual:a".to_string(), "v2".to_string())]);
        assert_eq!(
            store.get("virtual:a").as_deref(),
            Some("v2"),
            "publish must update the specifier it names"
        );
        assert_eq!(
            store.get("virtual:b").as_deref(),
            Some("v1"),
            "publish must leave every other specifier untouched (merge, not replace)"
        );
        // Both consumption shapes agree — the property the shared store
        // exists to guarantee (no split-brain between a `snapshot_pairs()`
        // consumer and a `get()` consumer).
        assert_eq!(
            store.snapshot().get("virtual:a").map(String::as_str),
            Some("v2")
        );
    }

    #[test]
    fn default_store_is_empty() {
        let store = PluginVirtualModuleStore::default();
        assert!(store.snapshot().is_empty());
        assert_eq!(store.get("virtual:anything"), None);
    }

    // -----------------------------------------------------------------------
    // PluginWatchOwnership — built from a hand-populated registry, no host.
    // -----------------------------------------------------------------------

    fn registry_with(entries: Vec<(&str, &str, Vec<PathBuf>)>) -> VirtualModuleRegistry {
        // `VirtualModuleRegistry::insert` is crate-visible; build via a
        // `SetupRegistries`-shaped accumulator is overkill for a unit test —
        // exercise the registry directly through its public `insert`-adjacent
        // surface by round-tripping through `SetupRegistries` would need a
        // real host. Since `insert` is `pub(crate)` and this test lives in
        // the same crate, call it directly.
        let mut registry = VirtualModuleRegistry::new();
        for (specifier, plugin, watch_files) in entries {
            registry.insert(crate::plugin_registries::VirtualModuleEntry {
                specifier: specifier.to_string(),
                plugin: plugin.to_string(),
                loader_id: VirtualLoaderId(format!("loader:{specifier}")),
                watch_files,
            });
        }
        registry
    }

    #[test]
    fn ownership_map_is_empty_when_no_loader_declares_watch_files() {
        let registry = registry_with(vec![("virtual:a", "p", vec![])]);
        let ownership = PluginWatchOwnership::from_registry(&registry);
        assert!(ownership.is_empty());
        assert!(ownership.watched_paths().is_empty());
        assert!(ownership
            .affected(&[PathBuf::from("/proj/data/whatever.json")])
            .is_empty());
    }

    #[test]
    fn one_loader_many_files_both_paths_resolve_to_the_same_specifier() {
        let a = PathBuf::from("/proj/data/a.json");
        let b = PathBuf::from("/proj/data/b.json");
        let registry = registry_with(vec![("virtual:data", "p", vec![a.clone(), b.clone()])]);
        let ownership = PluginWatchOwnership::from_registry(&registry);
        assert_eq!(ownership.watched_paths().len(), 2);

        let via_a = ownership.affected(std::slice::from_ref(&a));
        assert_eq!(via_a.len(), 1);
        assert_eq!(via_a[0].0, "virtual:data");

        let via_b = ownership.affected(std::slice::from_ref(&b));
        assert_eq!(via_b.len(), 1);
        assert_eq!(via_b[0].0, "virtual:data");

        // Both paths changing in the same tick still yields ONE affected
        // entry — a single loader re-invoked once, not twice.
        let via_both = ownership.affected(&[a, b]);
        assert_eq!(via_both.len(), 1);
        assert_eq!(via_both[0].0, "virtual:data");
    }

    #[test]
    fn overlapping_declarations_two_loaders_one_file_both_affected() {
        let shared = PathBuf::from("/proj/data/shared.json");
        let registry = registry_with(vec![
            ("virtual:a", "plugin-a", vec![shared.clone()]),
            ("virtual:b", "plugin-b", vec![shared.clone()]),
        ]);
        let ownership = PluginWatchOwnership::from_registry(&registry);
        assert_eq!(ownership.watched_paths(), BTreeSet::from([shared.clone()]));

        let affected = ownership.affected(&[shared]);
        let specifiers: BTreeSet<&str> = affected.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(
            specifiers,
            BTreeSet::from(["virtual:a", "virtual:b"]),
            "both loaders watching the same file must both be reported as affected"
        );
    }

    #[test]
    fn affected_ignores_unrelated_changed_paths() {
        let watched = PathBuf::from("/proj/data/watched.json");
        let registry = registry_with(vec![("virtual:a", "p", vec![watched.clone()])]);
        let ownership = PluginWatchOwnership::from_registry(&registry);
        let unrelated = PathBuf::from("/proj/pages/index.tsx");
        assert!(ownership.affected(&[unrelated]).is_empty());
        assert_eq!(ownership.affected(&[watched]).len(), 1);
    }

    // -----------------------------------------------------------------------
    // PluginRefreshState::refresh — real PluginHost + Node subprocess.
    // Mirrors the self-skip convention in `plugin_runner.rs`'s own tests:
    // no `#[ignore]` — Node is assumed present (unlike esbuild/tailwind,
    // which are staged separately and DO need an env-gate).
    // -----------------------------------------------------------------------

    fn host_node_available() -> bool {
        std::process::Command::new("node")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn file_url_for_test(p: &std::path::Path) -> String {
        format!("file://{}", p.to_string_lossy())
    }

    /// Spawn a host running `plugin_src`, run `setup`, and return
    /// `(host, registrations)`.
    async fn spawn_and_setup(
        tmp: &std::path::Path,
        plugin_name: &str,
        plugin_src: &str,
    ) -> (PluginHost, SetupRegistries) {
        let plugin_path = tmp.join(format!("{plugin_name}.mjs"));
        tokio::fs::write(&plugin_path, plugin_src).await.unwrap();
        let host = PluginHost::spawn(
            vec![PluginSpec {
                name: plugin_name.to_string(),
                module: file_url_for_test(&plugin_path),
                options: serde_json::json!({}),
            }],
            None,
        )
        .await
        .expect("host spawns");
        let regs = host
            .run_setup(tmp, SetupCommand::Dev, &serde_json::json!({}))
            .await
            .expect("setup ok");
        (host, regs)
    }

    #[tokio::test]
    async fn refresh_is_noop_without_a_host_or_unrelated_paths() {
        // No host at all (pluginless project).
        let state = PluginRefreshState::default();
        let outcome = state
            .refresh(&[PathBuf::from("/proj/data/anything.json")])
            .await;
        assert!(outcome.is_noop());
        assert!(state.store().snapshot().is_empty());
    }

    #[tokio::test]
    async fn refresh_publishes_fresh_content_read_from_disk() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let watch_path = tmp.path().join("data.json");
        std::fs::write(&watch_path, "one").unwrap();
        let plugin_src = format!(
            r#"
            import {{ readFileSync }} from "node:fs";
            export default {{
              name: "watcher",
              setup({{ addVirtualModule }}) {{
                addVirtualModule("virtual:data", () => {{
                  const text = readFileSync({watch:?}, "utf8");
                  return `export default ${{JSON.stringify(text)}}`;
                }}, {{ watchFiles: [{watch:?}] }});
              }},
            }};
            "#,
            watch = watch_path.to_string_lossy().to_string(),
        );
        let (host, regs) = spawn_and_setup(tmp.path(), "watcher", &plugin_src).await;
        let ownership = PluginWatchOwnership::from_registry(&regs.virtual_modules);
        let seed = regs.virtual_modules.get("virtual:data").map(|_| {
            (
                "virtual:data".to_string(),
                "export default \"stale\"".to_string(),
            )
        });
        let store = PluginVirtualModuleStore::seeded(seed);
        let state = PluginRefreshState::new(Some(host.clone()), ownership, store);

        assert_eq!(
            state.store().get("virtual:data").as_deref(),
            Some("export default \"stale\"")
        );

        std::fs::write(&watch_path, "two").unwrap();
        let outcome = state.refresh(std::slice::from_ref(&watch_path)).await;
        assert_eq!(outcome.refreshed, vec!["virtual:data".to_string()]);
        assert!(outcome.failed.is_empty());
        assert_eq!(
            state.store().get("virtual:data").as_deref(),
            Some("export default \"two\"")
        );

        // An unrelated changed path is a true no-op — no host round trip,
        // store untouched.
        let outcome = state.refresh(&[tmp.path().join("unrelated.tsx")]).await;
        assert!(outcome.is_noop());
        assert_eq!(
            state.store().get("virtual:data").as_deref(),
            Some("export default \"two\"")
        );

        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn refresh_forced_failure_then_successful_retry_recovers() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let watch_path = tmp.path().join("data.json");
        std::fs::write(&watch_path, "one").unwrap();
        let plugin_src = format!(
            r#"
            import {{ readFileSync }} from "node:fs";
            export default {{
              name: "watcher",
              setup({{ addVirtualModule }}) {{
                addVirtualModule("virtual:data", () => {{
                  const text = readFileSync({watch:?}, "utf8");
                  return `export default ${{JSON.stringify(text)}}`;
                }}, {{ watchFiles: [{watch:?}] }});
              }},
            }};
            "#,
            watch = watch_path.to_string_lossy().to_string(),
        );
        let (host, regs) = spawn_and_setup(tmp.path(), "watcher", &plugin_src).await;
        let ownership = PluginWatchOwnership::from_registry(&regs.virtual_modules);
        let store = PluginVirtualModuleStore::seeded([(
            "virtual:data".to_string(),
            "export default \"one\"".to_string(),
        )]);
        let state = PluginRefreshState::new(Some(host.clone()), ownership, store);

        // Delete the watched file — the loader's `readFileSync` throws, so
        // the forced reload must fail without touching the store.
        std::fs::remove_file(&watch_path).unwrap();
        let outcome = state.refresh(std::slice::from_ref(&watch_path)).await;
        assert!(
            outcome.refreshed.is_empty(),
            "a failing loader must publish nothing"
        );
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "virtual:data");
        assert_eq!(
            state.store().get("virtual:data").as_deref(),
            Some("export default \"one\""),
            "the last-good memo must survive a failed forced reload"
        );

        // Recreate the file — the very next event recovers.
        std::fs::write(&watch_path, "recovered").unwrap();
        let outcome = state.refresh(std::slice::from_ref(&watch_path)).await;
        assert_eq!(outcome.refreshed, vec!["virtual:data".to_string()]);
        assert!(outcome.failed.is_empty());
        assert_eq!(
            state.store().get("virtual:data").as_deref(),
            Some("export default \"recovered\"")
        );

        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn refresh_all_or_nothing_one_failing_loader_blocks_the_whole_batch_then_recovers_via_pending_retry(
    ) {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let ok_watch = tmp.path().join("ok.json");
        let bad_watch = tmp.path().join("bad.json");
        std::fs::write(&ok_watch, "ok-one").unwrap();
        std::fs::write(&bad_watch, "bad-one").unwrap();
        let plugin_src = format!(
            r#"
            import {{ readFileSync }} from "node:fs";
            export default {{
              name: "watcher",
              setup({{ addVirtualModule }}) {{
                addVirtualModule("virtual:ok", () => {{
                  const text = readFileSync({ok:?}, "utf8");
                  return `export default ${{JSON.stringify(text)}}`;
                }}, {{ watchFiles: [{ok:?}] }});
                addVirtualModule("virtual:bad", () => {{
                  const text = readFileSync({bad:?}, "utf8");
                  return `export default ${{JSON.stringify(text)}}`;
                }}, {{ watchFiles: [{bad:?}] }});
              }},
            }};
            "#,
            ok = ok_watch.to_string_lossy().to_string(),
            bad = bad_watch.to_string_lossy().to_string(),
        );
        let (host, regs) = spawn_and_setup(tmp.path(), "watcher", &plugin_src).await;
        let ownership = PluginWatchOwnership::from_registry(&regs.virtual_modules);
        let store = PluginVirtualModuleStore::seeded([
            (
                "virtual:ok".to_string(),
                "export default \"ok-one\"".to_string(),
            ),
            (
                "virtual:bad".to_string(),
                "export default \"bad-one\"".to_string(),
            ),
        ]);
        let state = PluginRefreshState::new(Some(host.clone()), ownership, store);

        // Update the ok file, but delete the bad one — one changed-path
        // batch touches BOTH loaders (both watched files changed: one
        // edited, one removed).
        std::fs::write(&ok_watch, "ok-two").unwrap();
        std::fs::remove_file(&bad_watch).unwrap();
        let outcome = state.refresh(&[ok_watch.clone(), bad_watch.clone()]).await;
        assert!(
            outcome.refreshed.is_empty(),
            "virtual:ok must NOT be published even though its own loader succeeded, \
             because virtual:bad's loader in the same batch failed"
        );
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "virtual:bad");
        assert_eq!(
            state.store().get("virtual:ok").as_deref(),
            Some("export default \"ok-one\""),
            "virtual:ok's last-good memo must be retained, not silently advanced"
        );

        // Codex review (issue #2168): fix ONLY the bad file and refresh with
        // a NARROW batch that names just that one path — NOT ok_watch. If
        // virtual:ok's already-fetched "ok-two" content were simply
        // discarded for good, this narrower recovery event would republish
        // virtual:bad alone and leave virtual:ok stale forever. The pending-
        // retry set must carry virtual:ok's failed-batch membership forward
        // so THIS call retries it too, alongside virtual:bad.
        std::fs::write(&bad_watch, "bad-two").unwrap();
        let outcome = state.refresh(std::slice::from_ref(&bad_watch)).await;
        let mut refreshed = outcome.refreshed.clone();
        refreshed.sort();
        assert_eq!(
            refreshed,
            vec!["virtual:bad".to_string(), "virtual:ok".to_string()],
            "a recovery event naming only the fixed file must ALSO republish the \
             sibling specifier the earlier failed batch silently discarded"
        );
        assert!(outcome.failed.is_empty());
        assert_eq!(
            state.store().get("virtual:ok").as_deref(),
            Some("export default \"ok-two\""),
            "virtual:ok's fetched-but-discarded update must not be lost"
        );
        assert_eq!(
            state.store().get("virtual:bad").as_deref(),
            Some("export default \"bad-two\"")
        );

        host.shutdown().await.ok();
    }

    #[tokio::test]
    async fn refresh_overlapping_declarations_publishes_both_loaders() {
        if !host_node_available() {
            eprintln!("skipping: node not on PATH");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let shared_watch = tmp.path().join("shared.json");
        std::fs::write(&shared_watch, "one").unwrap();
        let plugin_src = format!(
            r#"
            import {{ readFileSync }} from "node:fs";
            export default {{
              name: "watcher",
              setup({{ addVirtualModule }}) {{
                addVirtualModule("virtual:a", () => {{
                  const text = readFileSync({shared:?}, "utf8");
                  return `export default "a:" + ${{JSON.stringify(text)}}`;
                }}, {{ watchFiles: [{shared:?}] }});
                addVirtualModule("virtual:b", () => {{
                  const text = readFileSync({shared:?}, "utf8");
                  return `export default "b:" + ${{JSON.stringify(text)}}`;
                }}, {{ watchFiles: [{shared:?}] }});
              }},
            }};
            "#,
            shared = shared_watch.to_string_lossy().to_string(),
        );
        let (host, regs) = spawn_and_setup(tmp.path(), "watcher", &plugin_src).await;
        let ownership = PluginWatchOwnership::from_registry(&regs.virtual_modules);
        let store = PluginVirtualModuleStore::seeded([
            ("virtual:a".to_string(), "stale-a".to_string()),
            ("virtual:b".to_string(), "stale-b".to_string()),
        ]);
        let state = PluginRefreshState::new(Some(host.clone()), ownership, store);

        std::fs::write(&shared_watch, "two").unwrap();
        let outcome = state.refresh(&[shared_watch]).await;
        let mut refreshed = outcome.refreshed.clone();
        refreshed.sort();
        assert_eq!(
            refreshed,
            vec!["virtual:a".to_string(), "virtual:b".to_string()],
            "editing the one shared watch file must refresh BOTH overlapping loaders"
        );
        assert!(outcome.failed.is_empty());
        assert!(state.store().get("virtual:a").unwrap().contains("\"two\""));
        assert!(state.store().get("virtual:b").unwrap().contains("\"two\""));

        host.shutdown().await.ok();
    }
}
