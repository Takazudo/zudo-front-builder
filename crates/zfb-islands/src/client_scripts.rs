//! Discovery and production adapter for `*.client.{ts,tsx,js,jsx}` entries.
//!
//! ## File convention
//!
//! An author opts a TypeScript/JavaScript file into the client-script pipeline
//! by naming it `<name>.client.<ext>` where `<ext>` is one of `ts`, `tsx`,
//! `js`, or `jsx`. The **entry name** is the file stem minus `.client` —
//! `search-widget.client.ts` → `search-widget`. Duplicate entry names across
//! directories fail the build with a clear error (mirroring
//! `Manifest::collisions`; see `crates/zfb-islands/src/manifest.rs:85-96`).
//!
//! ## Discovery roots
//!
//! Discovery walks `pages/` plus the islands-candidate roots from
//! `GranularityPolicy::islands_roots` (see
//! `crates/zfb-build/src/policy.rs` — defaults to `components/`, `src/`).
//! `layouts/` is intentionally excluded: the policy does not classify it as
//! an islands root, and #976 pins discovery to the policy set so the wave-2
//! dev-path sub-issue stays in lockstep with the dev-trigger classification.
//!
//! - `pages/`
//! - `components/`
//! - `src/`
//!
//! ## Production adapter
//!
//! [`build_production_client_scripts`] mirrors [`build_production_islands_asset`]
//! but produces one [`ProductionClientScriptAsset`] per discovered entry instead
//! of one combined asset. Each entry is bundled independently via
//! [`EsbuildSubprocessBundler::bundle_client_script_file`], content-hashed by
//! `ProductionAssetPipeline`, and served from
//! `/assets/client/<name>-<hash>.js`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::bundler::{BundleChunk, BundleConfig};
use crate::esbuild::EsbuildSubprocessBundler;
use zfb_types::{stable_client_script_relative_path, stable_client_script_url};

// The `.client.*` filename contract (infix, accepted extensions, and the
// `is_client_script_file` / `client_script_entry_name` predicates) is the
// single source of truth in `zfb-types` so the router (`zfb-router`) can skip
// `.client.*` files from route scanning without depending on this crate. We
// re-export them here to keep this module's public API stable.
pub use zfb_types::{
    client_script_entry_name, is_client_script_file, CLIENT_SCRIPT_EXTENSIONS, CLIENT_SCRIPT_INFIX,
};

/// Discovery roots for `.client.*` files (project-root-relative).
///
/// `pages/` plus the `GranularityPolicy::islands_roots` defaults
/// (`components/`, `src/` — `crates/zfb-build/src/policy.rs`). The dev-path
/// wave-2 sub-issue classifies watcher events against these same directories;
/// if discovery and classification diverge, a newly-created `.client.ts` file
/// under a root that discovery misses would never trigger a dev rebuild.
pub const CLIENT_SCRIPT_DISCOVERY_ROOTS: &[&str] = &["pages", "components", "src"];

/// One discovered `*.client.{ts,tsx,js,jsx}` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientScriptEntry {
    /// Logical name derived from the file stem minus `.client`
    /// (e.g. `"search-widget"` for `search-widget.client.ts`).
    pub entry_name: String,
    /// Absolute path to the source file.
    pub source_path: PathBuf,
}

/// One browser-only module-worker entry emitted beside its owning client
/// script.
///
/// `filename` is the locked, path-derived contract name from
/// [`zfb_types::module_worker_filename`]. `source_path` may point into a
/// preprocessing mirror; keeping the two values separate lets the caller
/// preserve the original project-relative filename while esbuild reads the
/// rewritten/mirrored source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientScriptWorkerEntry {
    /// Stable flat output filename, e.g. `worker-src-s-search-d-ts.js`.
    pub filename: String,
    /// JS/TS worker entry handed to esbuild.
    pub source_path: PathBuf,
}

/// A name-collision between two distinct source files that produced the same
/// entry name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientScriptCollision {
    /// The colliding entry name.
    pub name: String,
    /// The source path that did **not** make it into the discovery set
    /// (sorted after `kept_path`).
    pub dropped_path: PathBuf,
    /// The source path that **did** make it into the discovery set
    /// (sorted before `dropped_path`).
    pub kept_path: PathBuf,
}

/// Production bytes-only payload for one client-script entry, ready for
/// plugging into `ProductionAssetPipeline` via
/// `zfb_build::pipeline::AssetEmitterPayload`.
///
/// The shape mirrors [`zfb_islands::bundler::ProductionIslandsAsset`] but
/// carries a single entry rather than the combined islands bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionClientScriptAsset {
    /// Entry name (e.g. `"search-widget"`). Used to derive the stable URL
    /// and relative path.
    pub entry_name: String,
    /// Bundled JS bytes — minified when `BundleConfig::minify` was set.
    pub bytes: Vec<u8>,
    /// Output path relative to `dist_root`, e.g.
    /// `assets/client/search-widget.js`. The pipeline splices the content
    /// hash before the extension.
    pub relative_path: PathBuf,
    /// Stable public URL the renderer will embed in HTML, e.g.
    /// `/assets/client/search-widget.js`. The pipeline rewrites every
    /// boundary-anchored match in HTML to the hashed equivalent.
    pub stable_url: String,
    /// Stable, verbatim module-worker files shipped flat beside the hashed
    /// client-script entry. These filenames are never renamed or hashed;
    /// cache busting lives in the rewritten `?v=` URL query.
    pub companions: Vec<BundleChunk>,
}

/// Walk `project_root` for `*.client.{ts,tsx,js,jsx}` files under the
/// conventional discovery roots.
///
/// Returns a sorted (by `source_path`) list of discovered entries and a
/// (possibly empty) list of collisions. A collision occurs when two distinct
/// source files produce the same `entry_name`; callers that want a hard build
/// failure should check `collisions` and surface an error.
///
/// Files whose parent directories are not under any of the recognised roots are
/// silently ignored. Non-existent roots are also silently ignored (a project
/// without a `layouts/` directory is not an error).
pub fn discover_client_scripts(
    project_root: &Path,
) -> Result<(Vec<ClientScriptEntry>, Vec<ClientScriptCollision>)> {
    // Collect all candidate paths sorted deterministically.
    let mut candidates: Vec<PathBuf> = Vec::new();
    for root_name in CLIENT_SCRIPT_DISCOVERY_ROOTS {
        let root_dir = project_root.join(root_name);
        if !root_dir.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&root_dir)
            .into_iter()
            .filter_map(|r| r.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.into_path();
            if is_client_script_file(&path) {
                candidates.push(path);
            }
        }
    }
    // Sort for deterministic ordering: earlier-sorted source paths win
    // collision resolution (same rule as Manifest::collisions).
    candidates.sort();

    // Deduplicate by entry name, recording collisions.
    let mut entries_map: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut collisions: Vec<ClientScriptCollision> = Vec::new();

    for path in candidates {
        let entry_name = match client_script_entry_name(&path) {
            Some(n) => n,
            None => continue, // shouldn't happen given is_client_script_file check
        };
        match entries_map.get(&entry_name) {
            None => {
                entries_map.insert(entry_name, path);
            }
            Some(kept) => {
                collisions.push(ClientScriptCollision {
                    name: entry_name,
                    dropped_path: path,
                    kept_path: kept.clone(),
                });
            }
        }
    }

    // Build sorted Vec (BTreeMap iteration is already sorted by key).
    let entries: Vec<ClientScriptEntry> = entries_map
        .into_iter()
        .map(|(entry_name, source_path)| ClientScriptEntry {
            entry_name,
            source_path,
        })
        .collect();

    Ok((entries, collisions))
}

/// Run the esbuild bundler over each discovered client-script entry and return
/// production asset payloads.
///
/// - Empty input (`entries` is empty) returns `Ok(vec![])` without invoking
///   the bundler — no assets are produced and no `<script>` tags will be
///   injected for client scripts.
/// - Each entry is bundled independently. A failure on any entry surfaces as an
///   `Err` and aborts the whole pass (callers should treat this as a fatal
///   build error, the same way islands bundler failures are treated).
///
/// The returned [`ProductionClientScriptAsset`] items carry **stable**
/// relative paths and URLs (no hash). `ProductionAssetPipeline` performs the
/// hashing and HTML rewrite step.
pub fn build_production_client_scripts(
    bundler: &EsbuildSubprocessBundler,
    entries: &[ClientScriptEntry],
    config: &BundleConfig,
) -> Result<Vec<ProductionClientScriptAsset>> {
    build_production_client_scripts_with_workers(bundler, entries, &BTreeMap::new(), config)
}

/// Bundle production client scripts together with their discovered module
/// workers.
///
/// The map is keyed by [`ClientScriptEntry::entry_name`]. Every worker is
/// bundled as an independent, self-contained ESM entry and returned under its
/// contract filename. Unknown or missing subprocess outputs are rejected by
/// the esbuild read-back boundary.
pub fn build_production_client_scripts_with_workers(
    bundler: &EsbuildSubprocessBundler,
    entries: &[ClientScriptEntry],
    workers_by_entry: &BTreeMap<String, Vec<ClientScriptWorkerEntry>>,
    config: &BundleConfig,
) -> Result<Vec<ProductionClientScriptAsset>> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    let mut assets: Vec<ProductionClientScriptAsset> = Vec::with_capacity(entries.len());
    for entry in entries {
        let workers = workers_by_entry
            .get(&entry.entry_name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let output = bundler
            .bundle_client_script_file_with_workers(
                &entry.entry_name,
                &entry.source_path,
                workers,
                config,
            )
            .with_context(|| {
                format!(
                    "client-script bundler failed for entry `{}` ({})",
                    entry.entry_name,
                    entry.source_path.display()
                )
            })?;

        let relative_path = stable_client_script_relative_path(&entry.entry_name);
        let stable_url = stable_client_script_url(&entry.entry_name);

        assets.push(ProductionClientScriptAsset {
            entry_name: entry.entry_name.clone(),
            bytes: output.js.into_bytes(),
            relative_path,
            stable_url,
            companions: output.companions,
        });
    }

    Ok(assets)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The `is_client_script_file` / `client_script_entry_name` filename
    // predicates now live in `zfb-types`; their unit tests live there too.
    // The discovery tests below still exercise them through the re-export.

    // -----------------------------------------------------------------------
    // discover_client_scripts (filesystem-based)
    // -----------------------------------------------------------------------

    #[test]
    fn discover_finds_client_scripts_in_conventional_roots() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Create files under pages/, components/, src/ (and a nested dir).
        for rel in [
            "pages/search-widget.client.ts",
            "components/analytics.client.tsx",
            "src/my-lib.client.js",
            "src/widgets/fancy.client.jsx",
        ] {
            let file = root.join(rel);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "// client script").unwrap();
        }
        // Non-client file should not be found.
        let non_client = root.join("pages/index.tsx");
        std::fs::write(&non_client, "// not a client script").unwrap();

        let (entries, collisions) = discover_client_scripts(root).unwrap();
        assert!(
            collisions.is_empty(),
            "no collisions expected: {collisions:?}"
        );
        let names: Vec<&str> = entries.iter().map(|e| e.entry_name.as_str()).collect();
        assert!(names.contains(&"search-widget"), "names={names:?}");
        assert!(names.contains(&"analytics"), "names={names:?}");
        assert!(names.contains(&"my-lib"), "names={names:?}");
        assert!(names.contains(&"fancy"), "names={names:?}");
        assert!(
            !names.contains(&"index"),
            "index.tsx should not appear: {names:?}"
        );
    }

    #[test]
    fn discover_does_not_find_files_outside_conventional_roots() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Outside the conventional roots entirely.
        let file = root.join("lib/util.client.ts");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "// not in roots").unwrap();
        // layouts/ is watched by the dev server but is NOT an islands root
        // (GranularityPolicy::islands_roots) — #976 pins discovery to the
        // policy set, so .client.* files under layouts/ are ignored.
        let layouts_file = root.join("layouts/header-toggle.client.jsx");
        std::fs::create_dir_all(layouts_file.parent().unwrap()).unwrap();
        std::fs::write(&layouts_file, "// not in roots either").unwrap();

        let (entries, _) = discover_client_scripts(root).unwrap();
        assert!(
            entries.is_empty(),
            "should find nothing outside conventional roots: {entries:?}"
        );
    }

    #[test]
    fn discover_detects_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Two files with the same entry name in different roots.
        for rel in [
            "pages/search-widget.client.ts",
            "components/search-widget.client.ts",
        ] {
            let file = root.join(rel);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "// client script").unwrap();
        }

        let (entries, collisions) = discover_client_scripts(root).unwrap();
        assert_eq!(entries.len(), 1, "only one entry wins: {entries:?}");
        assert_eq!(
            entries[0].entry_name, "search-widget",
            "entry name: {:?}",
            entries[0]
        );
        assert_eq!(collisions.len(), 1, "exactly one collision: {collisions:?}");
        assert_eq!(collisions[0].name, "search-widget");
    }

    #[test]
    fn discover_returns_empty_for_project_with_no_client_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Create the root dirs but no .client.* files.
        std::fs::create_dir_all(root.join("pages")).unwrap();
        std::fs::write(root.join("pages/index.tsx"), "<h1>Hello</h1>").unwrap();

        let (entries, collisions) = discover_client_scripts(root).unwrap();
        assert!(entries.is_empty());
        assert!(collisions.is_empty());
    }

    // -----------------------------------------------------------------------
    // build_production_client_scripts (mock bundler)
    // -----------------------------------------------------------------------

    #[test]
    fn build_production_client_scripts_returns_empty_for_no_entries() {
        use crate::esbuild::{EsbuildSubprocessBundler, EsbuildSubprocessConfig};
        let cfg = EsbuildSubprocessConfig::default().with_mock_output("// mock");
        let bundler = EsbuildSubprocessBundler::new(cfg);
        let bundle_cfg = BundleConfig::default().with_outdir(std::path::PathBuf::from("/tmp"));
        let result = build_production_client_scripts(&bundler, &[], &bundle_cfg).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn build_production_client_scripts_bundles_each_entry() {
        use crate::esbuild::{EsbuildSubprocessBundler, EsbuildSubprocessConfig};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Stage two .client.ts files.
        for name in ["search-widget", "analytics"] {
            let path = root.join(format!("{name}.client.ts"));
            std::fs::write(&path, "// client script").unwrap();
        }

        let cfg = EsbuildSubprocessConfig::default().with_mock_output("// bundled");
        let bundler = EsbuildSubprocessBundler::new(cfg);
        let bundle_cfg = BundleConfig::default().with_outdir(root.to_path_buf());

        let entries = vec![
            ClientScriptEntry {
                entry_name: "search-widget".into(),
                source_path: root.join("search-widget.client.ts"),
            },
            ClientScriptEntry {
                entry_name: "analytics".into(),
                source_path: root.join("analytics.client.ts"),
            },
        ];

        let assets = build_production_client_scripts(&bundler, &entries, &bundle_cfg).unwrap();
        assert_eq!(assets.len(), 2);

        let sw = assets
            .iter()
            .find(|a| a.entry_name == "search-widget")
            .unwrap();
        assert_eq!(sw.stable_url, "/assets/client/search-widget.js");
        assert_eq!(
            sw.relative_path,
            PathBuf::from("assets/client/search-widget.js")
        );
        assert!(!sw.bytes.is_empty());

        let an = assets.iter().find(|a| a.entry_name == "analytics").unwrap();
        assert_eq!(an.stable_url, "/assets/client/analytics.js");
    }

    #[test]
    fn build_production_client_scripts_carries_worker_companions() {
        use crate::esbuild::{EsbuildSubprocessBundler, EsbuildSubprocessConfig};

        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("widget.client.ts");
        let worker = dir.path().join("worker.ts");
        std::fs::write(&entry, "console.log('entry');\n").unwrap();
        std::fs::write(&worker, "self.postMessage('worker');\n").unwrap();
        let bundler = EsbuildSubprocessBundler::new(
            EsbuildSubprocessConfig::default().with_mock_output("// bundled"),
        );
        let entries = vec![ClientScriptEntry {
            entry_name: "widget".into(),
            source_path: entry,
        }];
        let workers = BTreeMap::from([(
            "widget".into(),
            vec![ClientScriptWorkerEntry {
                filename: "worker-worker-d-ts.js".into(),
                source_path: worker,
            }],
        )]);

        let assets = build_production_client_scripts_with_workers(
            &bundler,
            &entries,
            &workers,
            &BundleConfig::production(),
        )
        .unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].companions.len(), 1);
        assert_eq!(assets[0].companions[0].filename, "worker-worker-d-ts.js");
        assert!(String::from_utf8_lossy(&assets[0].companions[0].bytes).contains("postMessage"));
    }

    #[test]
    fn build_production_client_scripts_stable_urls_are_distinct_from_islands() {
        // Acceptance: client-script stable URLs must not collide with the
        // islands or CSS stable URLs under the boundary_replace rewrite rules.
        use zfb_types::{STABLE_CSS_URL, STABLE_ISLANDS_URL};
        let url = zfb_types::stable_client_script_url("search-widget");
        assert_ne!(url, STABLE_CSS_URL);
        assert_ne!(url, STABLE_ISLANDS_URL);
        // No prefix collision either.
        assert!(!STABLE_CSS_URL.starts_with(&url));
        assert!(!STABLE_ISLANDS_URL.starts_with(&url));
        assert!(!url.starts_with(STABLE_CSS_URL));
        assert!(!url.starts_with(STABLE_ISLANDS_URL));
    }
}
