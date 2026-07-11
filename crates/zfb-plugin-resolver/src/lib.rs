//! # zfb-plugin-resolver
//!
//! Shared exact-match resolver inputs for the two esbuild call sites in
//! the workspace:
//!
//! - the main page/layout bundler in `zfb-build::bundler`
//! - the islands bundler in `zfb-islands::esbuild`
//!
//! Both crates used to forward plugin-registered aliases as
//! `--alias:<from>=<to>` CLI flags. esbuild's `--alias` is
//! **prefix-with-slash** — registering `@/foo` would silently match
//! `@/foo/bar` as well, which contradicts the documented exact-match
//! contract honored by the embedded V8 host
//! (`zfb-render::BundleModuleLoader::resolve_alias`).
//!
//! This helper produces the inputs needed to express the same aliases
//! as **exact-match** entries in esbuild's `--tsconfig` path-mapping
//! pipeline: a `compilerOptions.paths` entry without the wildcard
//! suffix is a literal exact-match in TypeScript / esbuild.
//!
//! ## What it does
//!
//! Given a list of plugin aliases and a list of plugin virtual modules,
//! plus a working dir for temp-file allocation, the helper:
//!
//! 1. Writes each virtual module's source to a temp `.mjs` file under
//!    the working dir (held alive in `ResolverInputs::_temp_files`).
//! 2. Builds a `Vec<(String, String)>` of `(specifier, absolute path)`
//!    pairs, with paths normalized to POSIX forward-slash form so the
//!    same JSON is emitted on Windows / WSL / Linux.
//! 3. Provides a `merge_into_tsconfig_paths` helper that folds those
//!    pairs into the caller's existing `tsconfig_paths` map without
//!    overwriting user-supplied entries.
//!
//! Callers then write the merged map into their synthetic tsconfig and
//! pass `--tsconfig=<that path>` to esbuild. No `--alias` flags are
//! emitted for plugin entries.
//!
//! ## Why one shared crate
//!
//! `zfb-build` depends on `zfb-render` and `zfb-content`; `zfb-islands`
//! does not depend on `zfb-build` (and `zfb-build` re-uses
//! `zfb-islands` through the orchestrator's bundling fan-out). Placing
//! this helper in either crate would create a cycle or pull in the
//! whole orchestrator graph. A tiny leaf crate sidesteps both.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tempfile::NamedTempFile;
use zfb_types::path_to_posix_string;

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            std::path::Component::RootDir => out.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if out.file_name().is_some() {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            std::path::Component::Normal(segment) => out.push(segment),
        }
    }
    out
}

/// Output of [`build_resolver_inputs`].
///
/// `paths_entries` is the list of `(specifier, absolute-target-path)`
/// pairs the caller will merge into its synthetic
/// `compilerOptions.paths` map. The target path is already in POSIX
/// forward-slash form.
///
/// `_temp_files` holds the on-disk lifetime of any virtual-module
/// `.mjs` files the helper wrote. The caller MUST keep this value
/// alive (e.g. by binding it to a local) until esbuild has finished
/// reading the files; their `Drop` impl deletes them from disk.
pub struct ResolverInputs {
    /// `(specifier, absolute-path)` pairs to merge into
    /// `compilerOptions.paths`. Paths are POSIX forward-slash.
    pub paths_entries: Vec<(String, String)>,

    /// `(specifier, absolute-POSIX-path)` pairs for **plugin virtual
    /// modules only**, to ALSO be emitted as esbuild
    /// `--alias:<specifier>=<path>` CLI flags via
    /// [`Self::virtual_module_alias_flags`] — a strict subset of
    /// `paths_entries`: plugin aliases like `@/foo` are NOT included, and
    /// neither is any virtual specifier the user's own
    /// `compilerOptions.paths` already claims (user-wins, #1267 — see
    /// `build_resolver_inputs`).
    ///
    /// ## Why a second mechanism on top of `compilerOptions.paths` (#1263)
    ///
    /// esbuild does **not** apply a tsconfig's `compilerOptions.paths` to
    /// any source file whose (real) directory is inside `node_modules`.
    /// In esbuild 0.25.x's resolver, `tsConfigForDir` returns `nil` the
    /// moment `dirInfo.isInsideNodeModules` is true — *before* it ever
    /// consults the `--tsconfig` / `--tsconfig-raw` override. So a preset
    /// route entrypoint installed under `node_modules`
    /// (e.g. `node_modules/@takazudo/zudo-doc/dist/routes/_chrome.js`)
    /// that does `import x from "virtual:foo"` fails to resolve through
    /// the synthetic tsconfig alone, with `Could not resolve "virtual:foo"`.
    /// This is NOT fixable by switching `--tsconfig=<file>` to
    /// `--tsconfig-raw`: both set the same gated override.
    ///
    /// esbuild's `--alias` (`PackageAliases`) is applied earlier in
    /// resolution and is **not** `node_modules`-gated, so it reaches the
    /// node_modules entrypoint. We deliberately use it ONLY for virtual
    /// modules — never for `@/`-style aliases — because:
    ///
    /// - A virtual module's target is always a single materialized
    ///   `.mjs` *file*. esbuild's alias is prefix-with-slash, so
    ///   `virtual:foo/bar` remaps to `<tmp>.mjs/bar`, which cannot
    ///   resolve (a file has no children) → it fails, *preserving the
    ///   exact-match contract* (#269). esbuild even reports the failure
    ///   as `Could not resolve "<tmp>.mjs/bar" (originally
    ///   "virtual:foo/bar")`, keeping the original specifier visible.
    /// - A plugin alias like `@/foo` may point at a *directory*, where
    ///   prefix-with-slash WOULD silently resolve `@/foo/bar` to a real
    ///   file — exactly the regression #269 moved aliases off `--alias`
    ///   to avoid. Those stay `compilerOptions.paths`-only.
    pub virtual_module_alias_args: Vec<(String, String)>,

    /// Held-alive temp-file handles for plugin virtual modules.
    ///
    /// Public so callers can extend the lifetime explicitly; the field
    /// itself is otherwise opaque — the caller does not need to read
    /// it. Underscore prefix discourages incidental use.
    pub _temp_files: Vec<NamedTempFile>,
}

impl ResolverInputs {
    /// esbuild `--alias:<specifier>=<absolute-path>` CLI flags for every
    /// plugin virtual module (see [`Self::virtual_module_alias_args`]).
    ///
    /// Both esbuild call sites (`zfb-islands` and `zfb-build`) emit these
    /// IN ADDITION to the synthetic `--tsconfig` so a `virtual:*` import
    /// resolves even from a route entrypoint whose realpath is under
    /// `node_modules` (#1263). Returned as `Vec<String>` so the islands
    /// path can wrap each in `OsString` and the build path can pass it
    /// straight to `Command::arg`.
    ///
    /// esbuild's CLI splits `--alias:<k>=<v>` on the FIRST `=`
    /// (`strings.IndexByte`), so a specifier containing a colon
    /// (`virtual:foo`) is preserved intact as the key. Temp-file targets
    /// never contain `=`.
    pub fn virtual_module_alias_flags(&self) -> Vec<String> {
        self.virtual_module_alias_args
            .iter()
            .map(|(specifier, target)| format!("--alias:{specifier}={target}"))
            .collect()
    }
}

/// Build the per-call resolver inputs from plugin-registered aliases
/// and virtual modules.
///
/// - `aliases`: `(from, to)` pairs where `to` is an absolute filesystem
///   path the alias resolves to.
/// - `virtual_modules`: `(specifier, source)` pairs where `source` is
///   the ESM source text to materialize to disk.
/// - `working_dir`: where to allocate virtual-module temp files. Falls
///   back to the system temp dir when the path is not a directory
///   (matches the islands path's pre-existing fallback for the
///   unit-test environment).
/// - `user_tsconfig_paths`: the user's own `compilerOptions.paths` map
///   (keyed by pattern). Drives the user-wins collision policy (#1267):
///   a virtual specifier the user already claims (exact key, or a
///   wildcard it falls under) is dropped from BOTH the `--alias` arg set
///   AND `paths_entries`, so the user's own mapping governs. Dropping
///   only the `--alias` is insufficient for a wildcard collision: a
///   plugin EXACT key (`virtual:foo/bar`) out-specifics a user
///   `virtual:foo/*` pattern in TS/esbuild path matching, so the plugin
///   `paths_entry` must be suppressed too.
///
/// On success the returned `ResolverInputs::paths_entries` contains one
/// entry per alias plus one entry per virtual module the user does NOT
/// own, all keyed on the bare specifier, all targeting a single absolute
/// POSIX path. `virtual_module_alias_args` carries the same
/// user-unclaimed virtual-module entries.
pub fn build_resolver_inputs(
    aliases: &[(String, String)],
    virtual_modules: &[(String, String)],
    working_dir: &Path,
    user_tsconfig_paths: &BTreeMap<String, Vec<String>>,
) -> Result<ResolverInputs> {
    let mut paths_entries: Vec<(String, String)> = Vec::new();
    let mut virtual_module_alias_args: Vec<(String, String)> = Vec::new();
    let mut temp_files: Vec<NamedTempFile> = Vec::new();

    // Plugin aliases first. The target string already came from the
    // plugin registry as an absolute filesystem path (resolved against
    // the project root by `PluginSetupAccumulator::resolve_against_root`).
    // Normalize to POSIX so the JSON output is platform-stable.
    for (from, to) in aliases {
        paths_entries.push((from.clone(), path_to_posix_string(Path::new(to))));
    }

    // Plugin virtual modules. Each source is written to a `.mjs` temp
    // file *inside* `working_dir` so esbuild's upward node_modules walk
    // resolves to the right tree. If `working_dir` is not a real dir
    // (unit-test environment, missing path), fall back to the system
    // temp dir — the islands path does the same.
    for (specifier, source) in virtual_modules {
        let tmp = if working_dir.is_dir() {
            tempfile::Builder::new()
                .prefix(".zfb-virtual-")
                .suffix(".mjs")
                .tempfile_in(working_dir)
                .with_context(|| {
                    format!(
                        "zfb-plugin-resolver: failed to allocate virtual-module \
                         temp file for `{specifier}` inside `{}`",
                        working_dir.display()
                    )
                })?
        } else {
            tempfile::Builder::new()
                .prefix("zfb-virtual-")
                .suffix(".mjs")
                .tempfile()
                .with_context(|| {
                    format!(
                        "zfb-plugin-resolver: failed to allocate virtual-module \
                         temp file for `{specifier}`"
                    )
                })?
        };
        std::fs::write(tmp.path(), source.as_bytes()).with_context(|| {
            format!(
                "zfb-plugin-resolver: failed to write virtual-module source \
                 for specifier `{specifier}` to `{}`",
                tmp.path().display()
            )
        })?;
        let posix_target = path_to_posix_string(tmp.path());
        if user_claims_specifier(user_tsconfig_paths, specifier) {
            // The user's own `compilerOptions.paths` already claims this
            // specifier (exact key, or a wildcard it falls under). Suppress
            // BOTH the synthetic-tsconfig `paths_entry` AND the esbuild
            // `--alias` so the user's mapping governs (user-wins, #1267).
            //
            // Dropping the alias alone is NOT enough for a WILDCARD
            // collision: a plugin EXACT key like `virtual:foo/bar`
            // out-specifics the user's `virtual:foo/*` pattern in TS/esbuild
            // path matching and would win anyway, so the plugin
            // `paths_entry` must be dropped too. For an exact-key collision
            // `merge_into_tsconfig_paths` would already keep the user entry,
            // but suppressing here is equivalent and keeps the policy uniform.
            //
            // ACCEPTED TRADEOFF (#1271 spec): `--alias` and tsconfig `paths`
            // are mutually exclusive for one specifier (esbuild applies
            // `--alias` first), so we cannot have the user's mapping win AND
            // keep the plugin alias's node_modules reach. A user-claimed
            // `virtual:*` imported from a node_modules-resident entrypoint
            // therefore no longer resolves (tsconfig `paths` are gated off
            // under node_modules, #1263) — deliberate: the user who maps a
            // `virtual:` specifier opts into that. The temp file is still
            // materialized and retained in `temp_files`; it is simply left
            // unreferenced so resolution falls through to the user's mapping.
        } else {
            paths_entries.push((specifier.clone(), posix_target.clone()));
            // The same materialized target ALSO surfaces as an esbuild
            // `--alias` flag so the import resolves from a node_modules-
            // resident entrypoint, where `compilerOptions.paths` is gated
            // off (#1263).
            virtual_module_alias_args.push((specifier.clone(), posix_target));
        }
        temp_files.push(tmp);
    }

    Ok(ResolverInputs {
        paths_entries,
        virtual_module_alias_args,
        _temp_files: temp_files,
    })
}

/// True when the user's `compilerOptions.paths` already claims
/// `specifier` — either as an exact key, or via a wildcard pattern whose
/// fixed prefix/suffix `specifier` falls under. This is the user-wins
/// gate for plugin `virtual:*` `--alias` flags (#1267): a specifier the
/// user owns must NOT get a plugin `--alias` (esbuild applies `--alias`
/// before tsconfig `paths`, so it would override the user's mapping).
pub fn user_claims_specifier(
    user_tsconfig_paths: &BTreeMap<String, Vec<String>>,
    specifier: &str,
) -> bool {
    user_tsconfig_paths
        .keys()
        .any(|pattern| tsconfig_pattern_matches(pattern, specifier))
}

/// Match a single `compilerOptions.paths` key against a concrete
/// specifier using TypeScript's path-mapping semantics: a pattern has at
/// most one `*`, and `prefix*suffix` matches `s` when `s` starts with
/// `prefix`, ends with `suffix`, and is long enough to fill the `*`
/// (so `virtual:foo/*` matches `virtual:foo/bar` but NOT the shorter
/// exact `virtual:foo`). A pattern with no `*` matches only by exact
/// equality.
fn tsconfig_pattern_matches(pattern: &str, specifier: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == specifier,
        Some((prefix, suffix)) => {
            specifier.len() >= prefix.len() + suffix.len()
                && specifier.starts_with(prefix)
                && specifier.ends_with(suffix)
        }
    }
}

/// Merge `entries` (typically `ResolverInputs::paths_entries`) into a
/// caller-owned `paths` map shaped like `BTreeMap<String, Vec<String>>`
/// — the shape TypeScript / esbuild expect under
/// `compilerOptions.paths`.
///
/// **Collision policy: user wins.** If the map already contains an
/// entry for a given specifier (because the caller pre-populated it
/// from `BundlerInput::tsconfig_paths`), the existing value is left
/// untouched. The explicit `tsconfig_paths` field is the user's
/// hand-written contract; plugin-registered aliases are additive
/// suggestions on top. Plugin-vs-plugin conflicts are already
/// detected upstream by `PluginSetupAccumulator` (`AliasConflict` /
/// `VirtualModuleConflict`), so by the time entries reach this helper
/// they are guaranteed to be self-consistent.
pub fn merge_into_tsconfig_paths(
    paths: &mut std::collections::BTreeMap<String, Vec<String>>,
    entries: &[(String, String)],
) {
    for (specifier, target) in entries {
        paths
            .entry(specifier.clone())
            .or_insert_with(|| vec![target.clone()]);
    }
}

// ---------------------------------------------------------------------------
// Shared tsconfig compilerOptions.paths / extends-chain helper
// ---------------------------------------------------------------------------
//
// Two call sites in the workspace need to read a project's tsconfig.json and
// walk its `extends` chain to collect `compilerOptions.paths` + `baseUrl`:
//
//   - `crates/zfb/src/commands/build.rs` (via `read_tsconfig_paths_into_map`)
//   - `crates/zfb-islands/src/scanner.rs` (via `parse_tsconfig_paths`)
//
// Previously the scanner lacked JSONC comment stripping, so a tsconfig with
// `//` comments or trailing commas silently broke alias resolution there.
// This shared helper adds JSONC stripping to both sites and uses a
// file-based `extends` resolver (`resolve_tsconfig_extends_file`) that
// supports any tsconfig filename (e.g. `tsconfig.base.json`), not just
// the scanner's old directory-only resolver that required the target to
// be named `tsconfig.json`.

/// Parsed `compilerOptions.paths` and/or resolved `baseUrl` from a
/// `tsconfig.json` file, walked through any `extends` chain.
///
/// `targets` inside each [`TsPathAlias`] are the **raw** strings from
/// `compilerOptions.paths` — relative to `base_dir`, not pre-absolutised.
/// Callers that need absolute paths (e.g. `build.rs`) must join each
/// target onto `base_dir` themselves (using
/// [`resolve_tsconfig_path_target`] or equivalent logic).
///
/// The scanner in `zfb-islands` uses `base_dir.join(&substituted)` at
/// resolution time, so it naturally gets the same result without any
/// extra conversion step.
#[derive(Debug, Clone)]
pub struct TsConfigPaths {
    /// Absolute directory used as the anchor for relative
    /// `compilerOptions.paths` targets. This is the effective resolved
    /// `compilerOptions.baseUrl` after the extends merge. When the effective
    /// config has no `baseUrl`, it defaults to the directory of the config
    /// that declared the winning `paths` table.
    pub base_dir: PathBuf,
    /// Explicit (or inherited through `extends`) `compilerOptions.baseUrl`.
    ///
    /// This remains `None` when `base_dir` is only the implicit directory
    /// anchor for a `paths` table. Keeping the distinction lets module-graph
    /// consumers apply TypeScript's bare-specifier baseUrl fallback without
    /// accidentally treating every unmatched `paths` specifier as a local
    /// project file.
    pub base_url: Option<PathBuf>,
    /// Parsed alias entries, in `compilerOptions.paths` source order.
    pub aliases: Vec<TsPathAlias>,
}

/// One entry from `compilerOptions.paths`.
#[derive(Debug, Clone)]
pub struct TsPathAlias {
    /// Pattern as authored — e.g. `"@/*"` or `"~"`. The TypeScript
    /// spec allows at most one `*` per pattern (the wildcard form).
    pub pattern: String,
    /// Substitution targets, in priority order. Each target is the
    /// raw string from `tsconfig.json`; callers join it onto
    /// `TsConfigPaths::base_dir` to get the absolute probe path.
    pub targets: Vec<String>,
}

/// Read `<tsconfig_dir>/tsconfig.json` and walk its `extends` chain,
/// returning the merged `compilerOptions.paths` + `baseUrl` if any
/// paths are found.
///
/// Returns `None` when:
/// - the file cannot be read,
/// - the content cannot be parsed (even after JSONC stripping),
/// - neither a usable `paths` table nor `baseUrl` exists in the chain.
///
/// ## JSONC support
///
/// `tsconfig.json` files conventionally allow `//` line comments,
/// `/* … */` block comments, and trailing commas — TypeScript itself
/// accepts them, as does esbuild's built-in tsconfig reader.  This
/// function strips those artefacts before handing the text to
/// `serde_json` so annotated tsconfigs work without a full JSONC
/// parser.
///
/// ## extends chain
///
/// Chains are followed up to depth 8 to guard against cycles. Path and bare
/// npm-package extends specifiers are resolved, including extends arrays.
/// Any tsconfig filename is supported (e.g. `tsconfig.base.json`) — unlike
/// the old scanner helper which only accepted files named `tsconfig.json`.
///
/// ## Merge semantics
///
/// Extends-array parents merge left-to-right and the leaf merges last. The
/// winning effective `baseUrl` anchors the winning `paths` table even when
/// those fields came from different configs; this matches pinned esbuild.
/// Without an effective `baseUrl`, paths anchor at the config that declared
/// the winning table.
pub fn read_tsconfig_paths(tsconfig_dir: &Path) -> Option<TsConfigPaths> {
    read_tsconfig_paths_file(&tsconfig_dir.join("tsconfig.json"))
}

/// Read path-mapping resolver inputs from one explicit TypeScript-style
/// config file.
///
/// This is the file-oriented sibling of [`read_tsconfig_paths`]. It accepts
/// either `tsconfig.json`, `jsconfig.json`, or another config filename used as
/// a leaf. It follows path/package `extends` values (including arrays) with
/// TypeScript's later-parent-then-leaf precedence. Callers that already
/// selected the closest config for a source file use this entry point so a
/// nested `jsconfig.json` is not accidentally replaced by a more distant
/// `tsconfig.json`.
pub fn read_tsconfig_paths_file(config_file: &Path) -> Option<TsConfigPaths> {
    #[derive(Clone)]
    struct EffectivePaths {
        declaration_dir: PathBuf,
        aliases: Vec<TsPathAlias>,
    }

    #[derive(Clone, Default)]
    struct EffectiveConfig {
        base_url: Option<PathBuf>,
        paths: Option<EffectivePaths>,
    }

    fn walk(file: &Path, depth: usize) -> Option<EffectiveConfig> {
        if depth > 8 {
            return None;
        }
        let dir = file.parent()?;
        let raw = std::fs::read_to_string(file).ok()?;
        let cleaned = strip_tsconfig_jsonc(&raw);
        let value: serde_json::Value = serde_json::from_str(&cleaned).ok()?;

        let extends_values: Vec<&str> = match value.get("extends") {
            Some(serde_json::Value::String(extends)) => vec![extends],
            Some(serde_json::Value::Array(extends)) => extends
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect(),
            _ => Vec::new(),
        };
        let mut effective = EffectiveConfig::default();
        for extends in extends_values {
            let Some(parent_file) = resolve_tsconfig_extends_file(dir, extends) else {
                continue;
            };
            let Some(parent) = walk(&parent_file, depth + 1) else {
                continue;
            };
            if let Some(paths) = parent.paths {
                effective.paths = Some(paths);
            }
            if let Some(base_url) = parent.base_url {
                effective.base_url = Some(base_url);
            }
        }

        let compiler_options = value.get("compilerOptions");
        let local_base_url = compiler_options
            .and_then(|c| c.get("baseUrl"))
            .and_then(|b| b.as_str())
            .map(|raw_url| {
                // Strip a leading `./` so `dir.join("./src")` does not embed
                // a `.` component in the resulting path string.
                let clean = raw_url.strip_prefix("./").unwrap_or(raw_url);
                if clean.is_empty() || clean == "." {
                    normalize_lexical_path(dir)
                } else {
                    normalize_lexical_path(&dir.join(clean))
                }
            });
        let local_aliases = compiler_options
            .and_then(|c| c.get("paths"))
            .and_then(|p| p.as_object())
            .map(|map| {
                map.iter()
                    .filter_map(|(pattern, targets_value)| {
                        let targets = targets_value.as_array()?;
                        let collected: Vec<String> = targets
                            .iter()
                            .filter_map(|t| t.as_str().map(|s| s.to_string()))
                            .collect();
                        if collected.is_empty() {
                            None
                        } else {
                            Some(TsPathAlias {
                                pattern: pattern.clone(),
                                targets: collected,
                            })
                        }
                    })
                    .collect::<Vec<_>>()
            });
        if let Some(base_url) = local_base_url.as_ref() {
            effective.base_url = Some(base_url.clone());
        }
        if let Some(aliases) = local_aliases {
            effective.paths = Some(EffectivePaths {
                declaration_dir: normalize_lexical_path(dir),
                aliases,
            });
        }
        Some(effective)
    }

    let effective = walk(config_file, 0)?;
    let base_url = effective.base_url;
    let (base_dir, aliases) = match effective.paths {
        Some(paths) => (
            base_url
                .clone()
                .unwrap_or_else(|| paths.declaration_dir.clone()),
            paths.aliases,
        ),
        None => (base_url.clone()?, Vec::new()),
    };

    Some(TsConfigPaths {
        base_dir,
        base_url,
        aliases,
    })
}

/// Read `<project_root>/tsconfig.json` and return its
/// `compilerOptions.paths` as an **absolutised** `BTreeMap` suitable for
/// forwarding directly into esbuild's synthetic tsconfig writer.
///
/// This is the `build.rs` call-site adapter: it calls [`read_tsconfig_paths`]
/// and then resolves every relative target against the resolved `base_dir`
/// using [`resolve_tsconfig_path_target`].  Already-absolute targets are
/// returned unchanged.  Missing file / parse errors / absent paths all
/// return an empty map.
pub fn read_tsconfig_paths_into_map(project_root: &Path) -> BTreeMap<String, Vec<String>> {
    let Some(parsed) = read_tsconfig_paths(project_root) else {
        return BTreeMap::new();
    };

    tsconfig_paths_into_map(&parsed)
}

/// Read one explicitly selected TypeScript-style config file and return its
/// `compilerOptions.paths` as an absolutised map.
///
/// See [`read_tsconfig_paths_file`] for why the file-oriented form matters to
/// preprocessing shadows with nested `tsconfig.json` / `jsconfig.json`
/// scopes.
pub fn read_tsconfig_paths_file_into_map(config_file: &Path) -> BTreeMap<String, Vec<String>> {
    let Some(parsed) = read_tsconfig_paths_file(config_file) else {
        return BTreeMap::new();
    };

    tsconfig_paths_into_map(&parsed)
}

fn tsconfig_paths_into_map(parsed: &TsConfigPaths) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for alias in &parsed.aliases {
        let entries: Vec<String> = alias
            .targets
            .iter()
            .map(|s| resolve_tsconfig_path_target(&parsed.base_dir, s))
            .collect();
        if !entries.is_empty() {
            out.insert(alias.pattern.clone(), entries);
        }
    }
    out
}

/// Resolve a tsconfig `extends` value to the absolute path of the extended
/// config **file**, suitable for passing directly to `fs::read_to_string`.
///
/// Supported shapes:
/// - Relative path starting with `./`, `../`, or `/`: resolved against
///   `extending_dir`. If the target is a directory, `tsconfig.json` is
///   appended. If the target is a file with any name, it is used directly.
///   The extends value may omit the `.json` extension.
/// - Bare npm-package specifier: resolved through the closest ancestor
///   `node_modules`. Scoped packages and package subpaths are supported; a
///   bare package root uses its `package.json#tsconfig` field when present,
///   then falls back to `tsconfig.json`.
///
/// Unlike the old `resolve_extends_target` in `scanner.rs`, this function
/// supports any tsconfig filename (e.g. `tsconfig.base.json`), not just
/// files named `tsconfig.json`.
struct TsConfigResolutionDependency {
    path: PathBuf,
    affects_fingerprint: bool,
}

fn resolve_tsconfig_extends_with_dependencies(
    extending_dir: &Path,
    extends: &str,
) -> Option<(PathBuf, Vec<TsConfigResolutionDependency>)> {
    fn existing_config(raw: PathBuf) -> Option<(PathBuf, Vec<TsConfigResolutionDependency>)> {
        if raw.is_dir() {
            let candidate = raw.join("tsconfig.json");
            return candidate.is_file().then_some((candidate, Vec::new()));
        }
        if raw.is_file() {
            return Some((raw, Vec::new()));
        }
        // The extends value may omit the `.json` extension — TypeScript
        // accepts `"./tsconfig.base"` to mean `"./tsconfig.base.json"`.
        let mut appended = raw.clone().into_os_string();
        appended.push(".json");
        let with_ext = PathBuf::from(appended);
        with_ext.is_file().then(|| {
            (
                with_ext,
                vec![
                    // An exact file or directory at `raw` outranks the
                    // extension-appended fallback. Keep both the raw path and
                    // its directory-config probe observable so a
                    // create-then-populate sequence cannot leave a stale
                    // worker graph.
                    TsConfigResolutionDependency {
                        path: raw.clone(),
                        affects_fingerprint: false,
                    },
                    TsConfigResolutionDependency {
                        path: raw.join("tsconfig.json"),
                        affects_fingerprint: false,
                    },
                ],
            )
        })
    }

    let is_path =
        Path::new(extends).is_absolute() || extends.starts_with("./") || extends.starts_with("../");
    if is_path {
        return existing_config(extending_dir.join(extends));
    }

    let parts: Vec<&str> = extends.split('/').collect();
    if parts.is_empty()
        || parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return None;
    }
    let package_parts = if parts[0].starts_with('@') { 2 } else { 1 };
    if parts.len() < package_parts {
        return None;
    }
    let package_name = parts[..package_parts].join("/");
    let subpath = &parts[package_parts..];
    let mut search_dir = Some(extending_dir);
    while let Some(dir) = search_dir {
        let package_root = dir.join("node_modules").join(&package_name);
        if package_root.is_dir() {
            if !subpath.is_empty() {
                return existing_config(
                    subpath
                        .iter()
                        .fold(package_root, |path, part| path.join(part)),
                );
            }
            let package_json = package_root.join("package.json");
            // The candidate path is a resolver input even while absent:
            // creating/deleting it can switch a bare package extends target
            // between `package.json#tsconfig` and the default tsconfig.json.
            let package_metadata = TsConfigResolutionDependency {
                path: package_json.clone(),
                affects_fingerprint: true,
            };
            if let Ok(raw) = std::fs::read_to_string(&package_json) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(target) = value.get("tsconfig").and_then(|value| value.as_str()) {
                        if let Some((config, mut resolution_dependencies)) =
                            existing_config(package_root.join(target))
                        {
                            resolution_dependencies.insert(0, package_metadata);
                            return Some((config, resolution_dependencies));
                        }
                    }
                }
            }
            return existing_config(package_root.join("tsconfig.json")).map(
                |(config, mut resolution_dependencies)| {
                    resolution_dependencies.insert(0, package_metadata);
                    (config, resolution_dependencies)
                },
            );
        }
        search_dir = dir.parent();
    }
    None
}

/// Resolve one TypeScript-style `extends` target to its config file.
///
/// See [`collect_tsconfig_dependency_chain`] for the strict dependency walk
/// that additionally records package metadata used during this resolution.
pub fn resolve_tsconfig_extends_file(extending_dir: &Path, extends: &str) -> Option<PathBuf> {
    resolve_tsconfig_extends_with_dependencies(extending_dir, extends).map(|(config, _)| config)
}

/// One effective TypeScript config input or package-resolution metadata path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TsConfigDependencyInput {
    /// Candidate whose bytes or missing state affect config resolution.
    pub path: PathBuf,
    /// Resolution metadata may legitimately be absent. Effective config
    /// files themselves remain required and fail closed when unreadable.
    pub missing_allowed: bool,
    /// Effective configs and package metadata affect the fingerprint.
    /// Precedence probes are watch-only and must not make relocated cache
    /// identities depend on absent paths.
    pub affects_fingerprint: bool,
}

/// Rich form of [`collect_tsconfig_dependency_chain`] that distinguishes
/// required effective configs from optional package-resolution metadata.
///
/// The walk accepts the same `extends` forms as [`read_tsconfig_paths_file`]:
/// strings, arrays, relative/absolute files, and package specifiers. Unlike
/// the paths-only reader, this helper is intentionally strict. A worker cache
/// key must not look valid after omitting an unreadable, malformed, cyclic, or
/// unresolved parent that esbuild itself will consume (or reject) later.
/// Paths remain lexical rather than canonical so callers retain their stable
/// shadow-materialisation and invalidation identities.
pub fn collect_tsconfig_dependency_inputs(
    config_file: &Path,
) -> Result<Vec<TsConfigDependencyInput>> {
    fn walk(
        file: &Path,
        depth: usize,
        visiting: &mut BTreeSet<PathBuf>,
        emitted_configs: &mut BTreeSet<PathBuf>,
        emitted_inputs: &mut BTreeSet<PathBuf>,
        chain: &mut Vec<TsConfigDependencyInput>,
    ) -> Result<()> {
        if depth > 8 {
            bail!(
                "zfb-plugin-resolver: TypeScript config extends chain exceeded 8 levels at {}",
                file.display()
            );
        }
        let file = normalize_lexical_path(file);
        if emitted_configs.contains(&file) {
            return Ok(());
        }
        if !visiting.insert(file.clone()) {
            bail!(
                "zfb-plugin-resolver: cyclic TypeScript config extends chain at {}",
                file.display()
            );
        }

        let raw = std::fs::read_to_string(&file).with_context(|| {
            format!(
                "zfb-plugin-resolver: failed to read TypeScript config {}",
                file.display()
            )
        })?;
        let cleaned = strip_tsconfig_jsonc(&raw);
        let value: serde_json::Value = serde_json::from_str(&cleaned).with_context(|| {
            format!(
                "zfb-plugin-resolver: failed to parse TypeScript config {}",
                file.display()
            )
        })?;
        let extends_values: Vec<&str> = match value.get("extends") {
            Some(serde_json::Value::String(extends)) => vec![extends],
            Some(serde_json::Value::Array(extends)) => {
                let mut values = Vec::with_capacity(extends.len());
                for (index, extends) in extends.iter().enumerate() {
                    let Some(extends) = extends.as_str() else {
                        bail!(
                            "zfb-plugin-resolver: TypeScript config {} has non-string extends[{index}]",
                            file.display()
                        );
                    };
                    values.push(extends);
                }
                values
            }
            Some(_) => {
                bail!(
                    "zfb-plugin-resolver: TypeScript config {} has an extends value that is neither a string nor an array of strings",
                    file.display()
                )
            }
            None => Vec::new(),
        };
        let dir = file.parent().unwrap_or_else(|| Path::new("."));
        for extends in extends_values {
            let (parent, resolution_dependencies) =
                resolve_tsconfig_extends_with_dependencies(dir, extends).with_context(|| {
                    format!(
                        "zfb-plugin-resolver: TypeScript config {} extends unresolved target {extends:?}",
                        file.display()
                    )
                })?;
            for dependency in resolution_dependencies {
                let dependency_path = normalize_lexical_path(&dependency.path);
                if !emitted_configs.contains(&dependency_path)
                    && emitted_inputs.insert(dependency_path.clone())
                {
                    chain.push(TsConfigDependencyInput {
                        path: dependency_path,
                        missing_allowed: true,
                        affects_fingerprint: dependency.affects_fingerprint,
                    });
                }
            }
            walk(
                &parent,
                depth + 1,
                visiting,
                emitted_configs,
                emitted_inputs,
                chain,
            )?;
        }

        visiting.remove(&file);
        if emitted_configs.insert(file.clone()) {
            if let Some(existing) = chain.iter_mut().find(|input| input.path == file) {
                existing.missing_allowed = false;
                existing.affects_fingerprint = true;
            } else {
                emitted_inputs.insert(file.clone());
                chain.push(TsConfigDependencyInput {
                    path: file,
                    missing_allowed: false,
                    affects_fingerprint: true,
                });
            }
        }
        Ok(())
    }

    let mut visiting = BTreeSet::new();
    let mut emitted_configs = BTreeSet::new();
    let mut emitted_inputs = BTreeSet::new();
    let mut chain = Vec::new();
    walk(
        config_file,
        0,
        &mut visiting,
        &mut emitted_configs,
        &mut emitted_inputs,
        &mut chain,
    )?;
    Ok(chain)
}

/// Return every effective config, resolution-metadata candidate, and
/// watch-only precedence probe in deterministic discovery order.
///
/// This path-only compatibility view includes absent package.json and
/// extensionless-resolution candidates. Callers that hash contents should use
/// [`collect_tsconfig_dependency_inputs`] and honor `affects_fingerprint`.
pub fn collect_tsconfig_dependency_chain(config_file: &Path) -> Result<Vec<PathBuf>> {
    Ok(collect_tsconfig_dependency_inputs(config_file)?
        .into_iter()
        .map(|input| input.path)
        .collect())
}

/// Resolve one tsconfig `paths` target string to an absolute path against
/// `base_dir` (the resolved `compilerOptions.baseUrl`, or the directory of
/// the tsconfig that declared `paths` when no `baseUrl` is set).
///
/// Preserves any trailing `/*` glob suffix that esbuild reads as a wildcard.
/// Already-absolute targets are returned unchanged.
pub fn resolve_tsconfig_path_target(base_dir: &Path, target: &str) -> String {
    // Already-absolute targets bypass baseUrl in TypeScript and esbuild.
    if Path::new(target).is_absolute() {
        return target.to_string();
    }
    // Split off a trailing `/*` wildcard suffix before path-joining.
    let (prefix, suffix) = match target.rsplit_once("/*") {
        Some((p, "")) => (p, "/*"),
        _ => (target, ""),
    };
    // Strip a leading `./` (or bare `.`) so `base_dir.join(".")` does not
    // produce a trailing `.` component (e.g. `/root/src/.`) in the output.
    let clean_prefix = prefix
        .strip_prefix("./")
        .unwrap_or(prefix)
        .trim_end_matches('/');
    let abs = if clean_prefix.is_empty() || clean_prefix == "." {
        base_dir.to_path_buf()
    } else {
        base_dir.join(clean_prefix)
    };
    let abs = normalize_lexical_path(&abs);
    let mut out = abs.to_string_lossy().into_owned();
    out.push_str(suffix);
    out
}

/// Strip `//` line comments, `/* … */` block comments, and trailing
/// commas from a JSONC source so it parses with the strict `serde_json`
/// reader.
///
/// Both passes are char-based (multi-byte UTF-8 content — e.g. CJK path
/// targets or comments — passes through byte-identical) and
/// string-aware: escape sequences inside strings are honoured so a `//`
/// or a trailing-comma-lookalike inside a string value is never treated
/// as syntax.
pub fn strip_tsconfig_jsonc(input: &str) -> String {
    // Pass 1: strip comments.
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            // Line comment — keep the terminating newline so line
            // structure (and any line-sensitive diagnostics) survive.
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            // Block comment.
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }

    // Pass 2: strip trailing commas — `,` immediately preceding `}` or
    // `]` (whitespace allowed in between), skipping string contents.
    let chars: Vec<char> = out.chars().collect();
    let mut stripped = String::with_capacity(out.len());
    let mut in_string = false;
    let mut escape = false;
    let mut j = 0;
    while j < chars.len() {
        let c = chars[j];
        if in_string {
            stripped.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            j += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            stripped.push(c);
            j += 1;
            continue;
        }
        if c == ',' {
            let mut k = j + 1;
            while k < chars.len() && chars[k].is_whitespace() {
                k += 1;
            }
            if k < chars.len() && (chars[k] == '}' || chars[k] == ']') {
                j += 1;
                continue;
            }
        }
        stripped.push(c);
        j += 1;
    }
    stripped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    /// `build_resolver_inputs` with zero aliases + zero virtual modules
    /// returns an empty `paths_entries` and no temp files.
    #[test]
    fn empty_inputs_produce_empty_outputs() {
        let dir = TempDir::new().unwrap();
        let r = build_resolver_inputs(&[], &[], dir.path(), &BTreeMap::new()).unwrap();
        assert!(r.paths_entries.is_empty());
        assert!(r.virtual_module_alias_args.is_empty());
        assert!(r.virtual_module_alias_flags().is_empty());
        assert!(r._temp_files.is_empty());
    }

    /// An alias `(from, to)` produces exactly one `paths` entry keyed
    /// on the bare specifier and pointing to the target path. No
    /// wildcard suffix — this is the exact-match contract.
    #[test]
    fn alias_produces_exact_match_paths_entry() {
        let dir = TempDir::new().unwrap();
        let aliases = vec![("@/foo".to_string(), "/abs/src/foo.tsx".to_string())];
        let r = build_resolver_inputs(&aliases, &[], dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(r.paths_entries.len(), 1);
        assert_eq!(r.paths_entries[0].0, "@/foo");
        assert_eq!(r.paths_entries[0].1, "/abs/src/foo.tsx");
        // No wildcard — bare specifier only.
        assert!(!r.paths_entries[0].0.contains('*'));
        assert!(!r.paths_entries[0].1.contains('*'));
        // Plugin aliases must NOT become `--alias` flags — only virtual
        // modules do (#1263). `@/`-style aliases can target a directory,
        // where esbuild's prefix-with-slash `--alias` would regress the
        // exact-match contract (#269).
        assert!(
            r.virtual_module_alias_args.is_empty(),
            "alias entries must not be emitted as --alias flags"
        );
        assert!(r.virtual_module_alias_flags().is_empty());
    }

    /// Virtual modules materialize to a `.mjs` temp file under
    /// `working_dir` and the entry's target points at that file. The
    /// `NamedTempFile` lives in `_temp_files` so the disk handle is
    /// held alive until the caller drops `ResolverInputs`.
    #[test]
    fn virtual_module_materializes_to_temp_mjs_under_working_dir() {
        let dir = TempDir::new().unwrap();
        let vms = vec![(
            "virtual:meta".to_string(),
            "export default { ok: true };".to_string(),
        )];
        let r = build_resolver_inputs(&[], &vms, dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(r.paths_entries.len(), 1);
        assert_eq!(r.paths_entries[0].0, "virtual:meta");
        assert_eq!(r._temp_files.len(), 1);
        let tmp_path = r._temp_files[0].path().to_path_buf();
        // The temp file is under working_dir.
        assert!(
            tmp_path.starts_with(dir.path()),
            "temp file `{}` must be under working_dir `{}`",
            tmp_path.display(),
            dir.path().display()
        );
        // Suffix is `.mjs`.
        assert_eq!(tmp_path.extension().and_then(|s| s.to_str()), Some("mjs"));
        // The path entry's target matches the temp file path (POSIX).
        assert_eq!(r.paths_entries[0].1, path_to_posix_string(&tmp_path));
        // The source was written to disk.
        let contents = std::fs::read_to_string(&tmp_path).unwrap();
        assert_eq!(contents, "export default { ok: true };");
        // The virtual module ALSO surfaces as an `--alias` arg pointing at
        // the same temp file (#1263), so it resolves from a node_modules
        // entrypoint where `compilerOptions.paths` is gated off.
        assert_eq!(r.virtual_module_alias_args.len(), 1);
        assert_eq!(r.virtual_module_alias_args[0].0, "virtual:meta");
        assert_eq!(
            r.virtual_module_alias_args[0].1,
            path_to_posix_string(&tmp_path)
        );
        let expected_flag = format!("--alias:virtual:meta={}", path_to_posix_string(&tmp_path));
        assert_eq!(
            r.virtual_module_alias_flags(),
            vec![expected_flag],
            "virtual module must surface as a single --alias flag"
        );
    }

    /// `virtual_module_alias_flags` formats each pair as
    /// `--alias:<specifier>=<target>`. The specifier keeps its embedded
    /// colon (`virtual:meta`) — esbuild's CLI splits on the FIRST `=`, so
    /// the colon stays part of the key. The target is the alias value.
    #[test]
    fn virtual_module_alias_flags_format_is_alias_colon_key_equals_value() {
        let dir = TempDir::new().unwrap();
        let vms = vec![("virtual:meta".to_string(), "export default 1;".to_string())];
        let r = build_resolver_inputs(&[], &vms, dir.path(), &BTreeMap::new()).unwrap();
        let flags = r.virtual_module_alias_flags();
        assert_eq!(flags.len(), 1);
        let flag = &flags[0];
        assert!(
            flag.starts_with("--alias:virtual:meta="),
            "flag must keep the colon in the specifier key: {flag}"
        );
        // Exactly one `=` separates the `--alias:<key>` from the value, and
        // the value is the materialized temp path.
        let (head, value) = flag.split_once('=').unwrap();
        assert_eq!(head, "--alias:virtual:meta");
        assert_eq!(value, r.virtual_module_alias_args[0].1);
    }

    /// When `working_dir` does not exist, the helper still succeeds by
    /// falling back to the system temp dir. The path is no longer
    /// guaranteed to be under `working_dir`, but the file is still
    /// written and the entry references it.
    #[test]
    fn virtual_module_falls_back_to_system_tmpdir_when_working_dir_missing() {
        let bogus = std::path::PathBuf::from("/this/path/should/not/exist/zfb-test");
        let vms = vec![("virtual:x".to_string(), "export const x = 1;".to_string())];
        let r = build_resolver_inputs(&[], &vms, &bogus, &BTreeMap::new()).unwrap();
        assert_eq!(r._temp_files.len(), 1);
        let tmp_path = r._temp_files[0].path();
        assert!(tmp_path.exists(), "temp file must be on disk");
        let contents = std::fs::read_to_string(tmp_path).unwrap();
        assert_eq!(contents, "export const x = 1;");
    }

    /// `merge_into_tsconfig_paths` adds new entries when the key is
    /// absent. Plugin entries are wrapped as `vec![target]` — the
    /// `Vec<String>` shape esbuild expects for `compilerOptions.paths`
    /// values.
    #[test]
    fn merge_adds_new_keys() {
        let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let entries = vec![
            ("@/foo".to_string(), "/abs/foo.tsx".to_string()),
            ("virtual:meta".to_string(), "/tmp/v.mjs".to_string()),
        ];
        merge_into_tsconfig_paths(&mut paths, &entries);
        assert_eq!(paths.len(), 2);
        assert_eq!(
            paths.get("@/foo").unwrap(),
            &vec!["/abs/foo.tsx".to_string()]
        );
        assert_eq!(
            paths.get("virtual:meta").unwrap(),
            &vec!["/tmp/v.mjs".to_string()]
        );
    }

    /// **Collision policy test.** Pre-existing user entries (from
    /// `BundlerInput::tsconfig_paths`) MUST be preserved when a plugin
    /// later registers the same specifier — the explicit
    /// `tsconfig_paths` field is the user's hand-written contract.
    #[test]
    fn merge_preserves_user_entries_on_collision() {
        let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
        paths.insert(
            "@/foo".to_string(),
            vec!["src/user-version.tsx".to_string()],
        );
        let entries = vec![("@/foo".to_string(), "/abs/plugin-version.tsx".to_string())];
        merge_into_tsconfig_paths(&mut paths, &entries);
        // User entry is untouched.
        assert_eq!(
            paths.get("@/foo").unwrap(),
            &vec!["src/user-version.tsx".to_string()],
            "user-supplied tsconfig_paths entries must win over plugin entries"
        );
    }

    /// Pre-existing user entries with DIFFERENT keys coexist with
    /// plugin entries. Both are honored.
    #[test]
    fn merge_honors_both_user_and_plugin_entries() {
        let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
        paths.insert(
            "@/components/*".to_string(),
            vec!["src/components/*".to_string()],
        );
        let entries = vec![("@/data".to_string(), "/abs/data.ts".to_string())];
        merge_into_tsconfig_paths(&mut paths, &entries);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains_key("@/components/*"));
        assert!(paths.contains_key("@/data"));
    }

    /// `to_posix` is a no-op on Unix-style paths. On Windows it converts
    /// backslashes to forward slashes; on Unix it preserves them (backslash
    /// is a valid filename byte on Unix, not a separator).
    #[test]
    fn to_posix_replaces_backslashes() {
        assert_eq!(
            path_to_posix_string(Path::new("/abs/foo.tsx")),
            "/abs/foo.tsx"
        );
        // Platform-gated: on Windows the backslashes are converted; on
        // Unix the path string is returned verbatim (preserving the
        // backslash as a valid filename byte).
        let result = path_to_posix_string(Path::new(r"C:\abs\foo.tsx"));
        if cfg!(windows) {
            assert_eq!(result, "C:/abs/foo.tsx", "Windows: backslashes converted");
        } else {
            assert_eq!(result, r"C:\abs\foo.tsx", "Unix: backslashes preserved");
        }
    }

    /// Combined: aliases + virtual modules together produce one entry
    /// per registration in deterministic order (aliases first, then
    /// virtual modules), and the temp-file count matches the
    /// virtual-module count.
    #[test]
    fn combined_aliases_and_virtual_modules_round_trip() {
        let dir = TempDir::new().unwrap();
        let aliases = vec![
            ("@/foo".to_string(), "/abs/foo.tsx".to_string()),
            ("@/bar".to_string(), "/abs/bar.tsx".to_string()),
        ];
        let vms = vec![(
            "virtual:meta".to_string(),
            "export const meta = 1;".to_string(),
        )];
        let r = build_resolver_inputs(&aliases, &vms, dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(r.paths_entries.len(), 3);
        assert_eq!(r.paths_entries[0].0, "@/foo");
        assert_eq!(r.paths_entries[1].0, "@/bar");
        assert_eq!(r.paths_entries[2].0, "virtual:meta");
        assert_eq!(r._temp_files.len(), 1);
        // Only the virtual module becomes an `--alias` flag; the two
        // `@/`-aliases do NOT (#1263 — they stay tsconfig-paths-only).
        assert_eq!(r.virtual_module_alias_args.len(), 1);
        assert_eq!(r.virtual_module_alias_args[0].0, "virtual:meta");
        assert_eq!(r.virtual_module_alias_flags().len(), 1);
    }

    // -----------------------------------------------------------------------
    // user-wins `--alias` filtering (#1267) — exact + subpath collisions
    // -----------------------------------------------------------------------

    /// **No user collision** → the virtual module's `--alias` is STILL
    /// emitted. This is the #1263 / #1268-Symptom-1 path: a `virtual:*`
    /// import from a node_modules-resident entrypoint resolves via the
    /// alias, which is NOT node_modules-gated. A user `paths` map that
    /// claims an UNRELATED key (`@/components/*`) must not suppress it.
    #[test]
    fn no_user_collision_keeps_virtual_alias() {
        let dir = TempDir::new().unwrap();
        let vms = vec![("virtual:foo".to_string(), "export default 1;".to_string())];
        let mut user = BTreeMap::new();
        user.insert(
            "@/components/*".to_string(),
            vec!["src/components/*".to_string()],
        );
        let r = build_resolver_inputs(&[], &vms, dir.path(), &user).unwrap();
        // paths_entries is untouched by filtering — the synthetic-tsconfig
        // entry for the virtual module is always present.
        assert_eq!(r.paths_entries.len(), 1);
        assert_eq!(r.paths_entries[0].0, "virtual:foo");
        // And the alias survives because the user did not claim it.
        assert_eq!(r.virtual_module_alias_args.len(), 1);
        assert_eq!(r.virtual_module_alias_args[0].0, "virtual:foo");
        assert_eq!(
            r.virtual_module_alias_flags().len(),
            1,
            "non-colliding virtual module must keep its --alias (#1268 Symptom 1)"
        );
    }

    /// **Exact-key collision.** The user's `compilerOptions.paths` already
    /// maps `virtual:foo`. esbuild applies `--alias` BEFORE tsconfig
    /// `paths`, so a plugin `--alias:virtual:foo=...` would override the
    /// user's mapping — inverting the user-wins policy. The alias is
    /// filtered out; the user's synthetic-tsconfig entry governs. The
    /// synthetic `paths_entries` entry for the plugin VM still exists (it
    /// just loses on merge to the user's pre-seeded key).
    #[test]
    fn exact_key_collision_drops_virtual_alias() {
        let dir = TempDir::new().unwrap();
        let vms = vec![(
            "virtual:foo".to_string(),
            "export default 'plugin';".to_string(),
        )];
        let mut user = BTreeMap::new();
        user.insert(
            "virtual:foo".to_string(),
            vec!["src/user-foo.ts".to_string()],
        );
        let r = build_resolver_inputs(&[], &vms, dir.path(), &user).unwrap();
        // A user-claimed specifier is dropped from BOTH paths_entries and
        // the --alias arg set → the user's own mapping governs (#1267).
        assert!(
            r.paths_entries.is_empty(),
            "user-claimed exact specifier must NOT get a plugin paths_entry"
        );
        assert!(
            r.virtual_module_alias_args.is_empty(),
            "user-claimed exact specifier must NOT get a plugin --alias"
        );
        assert!(r.virtual_module_alias_flags().is_empty());
    }

    /// **Subpath / wildcard collision.** The user maps `virtual:foo/*`. A
    /// plugin VM `virtual:foo/bar` falls UNDER that wildcard, so its
    /// `--alias` is suppressed (user wildcard governs). A non-colliding
    /// VM `virtual:baz` still gets its alias — prefix-aware matching, not
    /// a blanket suppression. The bare `virtual:foo` (no slash) would NOT
    /// fall under `virtual:foo/*`, exercising the length/prefix guard.
    #[test]
    fn subpath_collision_drops_only_covered_virtual_alias() {
        let dir = TempDir::new().unwrap();
        let vms = vec![
            (
                "virtual:foo/bar".to_string(),
                "export default 'covered';".to_string(),
            ),
            (
                "virtual:baz".to_string(),
                "export default 'free';".to_string(),
            ),
        ];
        let mut user = BTreeMap::new();
        user.insert("virtual:foo/*".to_string(), vec!["src/foo/*".to_string()]);
        let r = build_resolver_inputs(&[], &vms, dir.path(), &user).unwrap();
        // The wildcard-covered `virtual:foo/bar` is dropped from BOTH
        // paths_entries and the alias set (a plugin EXACT key would
        // out-specific the user's `virtual:foo/*` in path matching and win
        // anyway); only the free `virtual:baz` survives in each.
        assert_eq!(r.paths_entries.len(), 1);
        assert_eq!(r.paths_entries[0].0, "virtual:baz");
        assert!(
            !r.paths_entries.iter().any(|(s, _)| s == "virtual:foo/bar"),
            "a specifier under the user's wildcard must NOT get a plugin paths_entry"
        );
        assert_eq!(r.virtual_module_alias_args.len(), 1);
        assert_eq!(
            r.virtual_module_alias_args[0].0, "virtual:baz",
            "only the non-colliding virtual module keeps its --alias"
        );
        assert!(
            !r.virtual_module_alias_args
                .iter()
                .any(|(spec, _)| spec == "virtual:foo/bar"),
            "a specifier under the user's wildcard must NOT get a plugin --alias"
        );
    }

    /// Unit coverage for the TS path-pattern matcher backing the filter:
    /// exact keys match only themselves; a `prefix/*` wildcard matches
    /// anything under the prefix but NOT the shorter bare prefix; a
    /// mid-string `*` honors both prefix and suffix.
    #[test]
    fn tsconfig_pattern_matches_semantics() {
        // Exact (no `*`).
        assert!(tsconfig_pattern_matches("virtual:foo", "virtual:foo"));
        assert!(!tsconfig_pattern_matches("virtual:foo", "virtual:foobar"));
        assert!(!tsconfig_pattern_matches("virtual:foo", "virtual:foo/bar"));
        // Trailing-slash wildcard.
        assert!(tsconfig_pattern_matches("virtual:foo/*", "virtual:foo/bar"));
        assert!(tsconfig_pattern_matches("virtual:foo/*", "virtual:foo/a/b"));
        assert!(!tsconfig_pattern_matches("virtual:foo/*", "virtual:foo"));
        assert!(!tsconfig_pattern_matches("virtual:foo/*", "virtual:baz"));
        // Mid-string `*` respects prefix AND suffix.
        assert!(tsconfig_pattern_matches("a*c", "abc"));
        assert!(tsconfig_pattern_matches("a*c", "ac"));
        assert!(!tsconfig_pattern_matches("a*c", "ab"));
    }

    // -----------------------------------------------------------------------
    // read_tsconfig_paths_into_map — unit tests (moved from build.rs, #669)
    // -----------------------------------------------------------------------
    //
    // These tests cover the full build-side call site: reads tsconfig.json,
    // walks the extends chain, resolves baseUrl, and absolutises all targets.

    /// Multiple direct path keys all survive: regression guard that
    /// `read_tsconfig_paths_into_map` returns every key present in the paths
    /// object, not just the first or a hard-coded subset.
    #[test]
    fn read_tsconfig_paths_multi_key_direct() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(
            root.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "paths": {
                  "@/*": ["src/*"],
                  "@data/*": ["data/*"],
                  "~/*": ["lib/*"]
                }
              }
            }"#,
        )
        .unwrap();

        let result = read_tsconfig_paths_into_map(root);
        assert!(result.contains_key("@/*"), "missing @/*; got {result:?}");
        assert!(
            result.contains_key("@data/*"),
            "missing @data/*; got {result:?}"
        );
        assert!(result.contains_key("~/*"), "missing ~/*; got {result:?}");
        assert_eq!(result.len(), 3, "unexpected extra keys: {result:?}");

        // Targets should be absolute, anchored at the project root.
        let at_star = &result["@/*"];
        let expected_target = format!("{}/src/*", root.display());
        assert_eq!(
            at_star,
            &vec![expected_target.clone()],
            "@/* target must be absolute"
        );
    }

    /// `extends` chain is followed: the leaf tsconfig has no `paths` of its
    /// own, but extends a `tsconfig.base.json` (same directory) that does.
    /// Inherited entries must appear in the returned map.
    #[test]
    fn read_tsconfig_paths_extends_chain_merges_parent_paths() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Base config declares the alias map.
        fs::write(
            root.join("tsconfig.base.json"),
            r#"{
              "compilerOptions": {
                "paths": {
                  "@/*": ["src/*"],
                  "@ui/*": ["ui/*"]
                }
              }
            }"#,
        )
        .unwrap();
        // Leaf config has no `paths` of its own — only an extends reference.
        fs::write(
            root.join("tsconfig.json"),
            r#"{
              "extends": "./tsconfig.base.json"
            }"#,
        )
        .unwrap();

        let result = read_tsconfig_paths_into_map(root);
        assert!(
            result.contains_key("@/*"),
            "inherited @/* must be present; got {result:?}"
        );
        assert!(
            result.contains_key("@ui/*"),
            "inherited @ui/* must be present; got {result:?}"
        );
        assert_eq!(result.len(), 2, "unexpected extra keys: {result:?}");

        // Targets must be absolute, anchored at the root (where tsconfig.base.json
        // lives — that's the config that declared paths, so it's the anchor).
        let at_star = &result["@/*"];
        let expected = format!("{}/src/*", root.display());
        assert_eq!(at_star, &vec![expected], "@/* target must be absolute");
    }

    #[test]
    fn read_tsconfig_paths_extends_array_prefers_later_parent_paths() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("first.json"),
            r#"{"compilerOptions":{"paths":{"@pick/*":["first/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("second.json"),
            r#"{"compilerOptions":{"paths":{"@pick/*":["second/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            r#"{"extends":["./first.json","./second.json"]}"#,
        )
        .unwrap();

        let result = read_tsconfig_paths_into_map(root);
        assert_eq!(
            result["@pick/*"],
            vec![root.join("second/*").to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn config_dependency_chain_tracks_array_and_package_extends_in_order() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let package = root.join("node_modules/@scope/worker-config");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@scope/worker-config","tsconfig":"base.json"}"#,
        )
        .unwrap();
        std::fs::write(
            package.join("base.json"),
            r#"{"compilerOptions":{"useDefineForClassFields":false}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("shared.json"),
            r#"{"compilerOptions":{"jsx":"automatic"}}"#,
        )
        .unwrap();
        let leaf = root.join("src/jsconfig.json");
        std::fs::write(
            &leaf,
            r#"{"extends":["@scope/worker-config","../shared.json"]}"#,
        )
        .unwrap();

        assert_eq!(
            collect_tsconfig_dependency_chain(&leaf).unwrap(),
            vec![
                package.join("package.json"),
                package.join("base.json"),
                root.join("shared.json"),
                leaf,
            ]
        );
    }

    #[test]
    fn config_dependency_chain_rejects_an_unresolved_parent() {
        let dir = TempDir::new().unwrap();
        let leaf = dir.path().join("tsconfig.json");
        std::fs::write(&leaf, r#"{"extends":"./missing.json"}"#).unwrap();

        let error = collect_tsconfig_dependency_chain(&leaf).unwrap_err();
        assert!(
            error.to_string().contains("extends unresolved target"),
            "{error:#}"
        );
    }

    #[test]
    fn extensionless_extends_reports_exact_and_directory_precedence_probes() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let raw = root.join("src/base");
        let fallback = root.join("src/base.json");
        std::fs::write(&fallback, r#"{"compilerOptions":{"jsx":"react"}}"#).unwrap();
        let leaf = root.join("src/jsconfig.json");
        std::fs::write(&leaf, r#"{"extends":"./base"}"#).unwrap();

        let inputs = collect_tsconfig_dependency_inputs(&leaf).unwrap();
        assert_eq!(
            inputs
                .iter()
                .map(|input| input.path.clone())
                .collect::<Vec<_>>(),
            vec![raw.clone(), raw.join("tsconfig.json"), fallback, leaf,]
        );
        assert!(!inputs[0].affects_fingerprint);
        assert!(!inputs[1].affects_fingerprint);
        assert!(inputs[2].affects_fingerprint);
        assert!(inputs[3].affects_fingerprint);
    }

    #[test]
    fn config_dependency_chain_rejects_malformed_extends_arrays() {
        let dir = TempDir::new().unwrap();
        let leaf = dir.path().join("tsconfig.json");
        std::fs::write(&leaf, r#"{"extends":["./base.json",42]}"#).unwrap();

        let error = collect_tsconfig_dependency_chain(&leaf).unwrap_err();
        assert!(
            error.to_string().contains("non-string extends[1]"),
            "{error:#}"
        );
    }

    #[test]
    fn package_metadata_candidate_survives_delete_fallback_and_recreate() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let package = root.join("node_modules/worker-config");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            package.join("a.json"),
            r#"{"compilerOptions":{"jsx":"react"}}"#,
        )
        .unwrap();
        std::fs::write(
            package.join("b.json"),
            r#"{"compilerOptions":{"jsx":"preserve"}}"#,
        )
        .unwrap();
        std::fs::write(package.join("tsconfig.json"), r#"{"compilerOptions":{}}"#).unwrap();
        let package_json = package.join("package.json");
        std::fs::write(&package_json, r#"{"tsconfig":"a.json"}"#).unwrap();
        let leaf = root.join("src/jsconfig.json");
        std::fs::write(&leaf, r#"{"extends":"worker-config"}"#).unwrap();

        assert_eq!(
            collect_tsconfig_dependency_chain(&leaf).unwrap(),
            vec![package_json.clone(), package.join("a.json"), leaf.clone()]
        );

        std::fs::remove_file(&package_json).unwrap();
        assert_eq!(
            collect_tsconfig_dependency_chain(&leaf).unwrap(),
            vec![
                package_json.clone(),
                package.join("tsconfig.json"),
                leaf.clone()
            ],
            "the absent package.json candidate must remain watchable while fallback is active"
        );

        std::fs::write(&package_json, r#"{"tsconfig":"b.json"}"#).unwrap();
        assert_eq!(
            collect_tsconfig_dependency_chain(&leaf).unwrap(),
            vec![package_json, package.join("b.json"), leaf]
        );
    }

    #[test]
    fn read_tsconfig_paths_extends_array_merges_later_base_url_with_earlier_paths() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("later-base")).unwrap();
        std::fs::write(
            root.join("paths.json"),
            r#"{"compilerOptions":{"paths":{"@merged/*":["feature/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("base-url.json"),
            r#"{"compilerOptions":{"baseUrl":"./later-base"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            r#"{"extends":["./paths.json","./base-url.json"]}"#,
        )
        .unwrap();

        let result = read_tsconfig_paths_into_map(root);
        assert_eq!(
            result["@merged/*"],
            vec![root
                .join("later-base/feature/*")
                .to_string_lossy()
                .into_owned()]
        );
    }

    #[test]
    fn read_tsconfig_paths_extends_array_merges_earlier_base_url_with_later_paths() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("early-base")).unwrap();
        std::fs::write(
            root.join("base-url.json"),
            r#"{"compilerOptions":{"baseUrl":"./early-base"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("paths.json"),
            r#"{"compilerOptions":{"paths":{"@merged/*":["feature/*"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            r#"{"extends":["./base-url.json","./paths.json"]}"#,
        )
        .unwrap();

        let result = read_tsconfig_paths_into_map(root);
        assert_eq!(
            result["@merged/*"],
            vec![root
                .join("early-base/feature/*")
                .to_string_lossy()
                .into_owned()],
            "pinned esbuild retains an earlier parent's baseUrl for a later parent's paths"
        );
    }

    #[test]
    fn read_tsconfig_paths_resolves_bare_package_extends_for_user_wins() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let package = root.join("node_modules/@scope/tsconfig-web");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@scope/tsconfig-web","tsconfig":"web.json"}"#,
        )
        .unwrap();
        std::fs::write(
            package.join("web.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"virtual:owned":["owned.ts"]}}}"#,
        )
        .unwrap();
        std::fs::write(package.join("owned.ts"), "export default 'user';\n").unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            r#"{"extends":"@scope/tsconfig-web"}"#,
        )
        .unwrap();

        let user = read_tsconfig_paths_into_map(root);
        assert_eq!(
            user["virtual:owned"],
            vec![package.join("owned.ts").to_string_lossy().into_owned()]
        );
        let resolver = build_resolver_inputs(
            &[],
            &[(
                "virtual:owned".to_string(),
                "export default 'plugin';".to_string(),
            )],
            root,
            &user,
        )
        .unwrap();
        assert!(resolver.paths_entries.is_empty());
        assert!(resolver.virtual_module_alias_flags().is_empty());
    }

    /// `compilerOptions.baseUrl` shifts the substitution anchor.
    /// `"baseUrl": "./src", "paths": { "@/*": ["./*"] }` must produce
    /// `<root>/src/*`, NOT `<root>/*`.
    #[test]
    fn read_tsconfig_paths_base_url_non_default_absolutises_correctly() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        fs::write(
            root.join("tsconfig.json"),
            r#"{
              "compilerOptions": {
                "baseUrl": "./src",
                "paths": {
                  "@/*": ["./*"]
                }
              }
            }"#,
        )
        .unwrap();

        let result = read_tsconfig_paths_into_map(root);
        assert!(result.contains_key("@/*"), "missing @/*; got {result:?}");

        let targets = &result["@/*"];
        let expected = format!("{}/src/*", root.display());
        assert_eq!(
            targets,
            &vec![expected.clone()],
            "target must be anchored at baseUrl (./src), not project root; \
             expected {expected:?}, got {targets:?}"
        );
    }

    #[test]
    fn read_tsconfig_paths_preserves_base_url_without_paths_table() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":"./src"}}"#,
        )
        .unwrap();

        let parsed = read_tsconfig_paths(root).expect("baseUrl-only config is resolver input");
        assert!(parsed.aliases.is_empty());
        assert_eq!(parsed.base_dir, root.join("src"));
        assert_eq!(parsed.base_url, Some(root.join("src")));
        assert!(read_tsconfig_paths_into_map(root).is_empty());
    }

    /// A leaf paths table and an inherited baseUrl are fields of the same
    /// effective config. Pinned esbuild anchors those paths at the inherited
    /// baseUrl, just like it does when the two fields come from different
    /// parents in an extends array.
    #[test]
    fn read_tsconfig_paths_leaf_paths_use_inherited_parent_base_url() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Parent lives in a subdir and declares a `baseUrl` pointing into its
        // own `lib/` — a directory distinct from both the leaf dir and the
        // leaf's intended target, so a mis-anchor is observable.
        let parent_dir = root.join("config");
        fs::create_dir_all(&parent_dir).unwrap();
        fs::write(
            parent_dir.join("tsconfig.base.json"),
            r#"{
              "compilerOptions": {
                "baseUrl": "./lib"
              }
            }"#,
        )
        .unwrap();
        // Leaf declares `paths` only (no `baseUrl`) and extends the parent.
        fs::write(
            root.join("tsconfig.json"),
            r#"{
              "extends": "./config/tsconfig.base.json",
              "compilerOptions": {
                "paths": {
                  "@/*": ["src/*"]
                }
              }
            }"#,
        )
        .unwrap();

        let result = read_tsconfig_paths_into_map(root);
        assert!(result.contains_key("@/*"), "missing @/*; got {result:?}");

        let targets = &result["@/*"];
        let expected = format!("{}/config/lib/src/*", root.display());
        assert_eq!(
            targets,
            &vec![expected.clone()],
            "leaf `paths` must use the inherited effective baseUrl; \
             expected {expected:?}, got {targets:?}"
        );
    }

    /// When no `tsconfig.json` is present the function must return an empty
    /// map without panicking (resilience contract).
    #[test]
    fn read_tsconfig_paths_missing_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let result = read_tsconfig_paths_into_map(dir.path());
        assert!(result.is_empty(), "expected empty map; got {result:?}");
    }

    /// JSONC (comment + trailing comma) preprocessing must survive so that
    /// annotated tsconfigs still parse correctly.
    #[test]
    fn read_tsconfig_paths_jsonc_preprocessing_survives() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        fs::write(
            root.join("tsconfig.json"),
            r#"{
              // project aliases
              "compilerOptions": {
                "paths": {
                  "@/*": ["src/*"], // inline comment
                  "~/*": ["lib/*"],  /* block comment */
                }
              }
            }"#,
        )
        .unwrap();

        let result = read_tsconfig_paths_into_map(root);
        assert!(
            result.contains_key("@/*"),
            "JSONC-annotated tsconfig must parse; got {result:?}"
        );
        assert!(
            result.contains_key("~/*"),
            "second key must survive; got {result:?}"
        );
    }

    /// Regression: multi-byte UTF-8 (CJK) content must pass through the
    /// JSONC stripper byte-identical. The old byte-as-char loop re-encoded
    /// every byte as Latin-1, mojibake-ing CJK path targets and comments.
    #[test]
    fn strip_tsconfig_jsonc_preserves_multibyte_utf8() {
        let input = "{\n  // 日本語のコメント\n  \"compilerOptions\": {\n    \"paths\": { \"@/*\": [\"ソース/*\"] }\n  }\n}";
        let cleaned = strip_tsconfig_jsonc(input);
        assert!(
            cleaned.contains("ソース/*"),
            "CJK string content must survive untouched; got {cleaned}"
        );
        assert!(
            !cleaned.contains("日本語"),
            "comment must be stripped; got {cleaned}"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(&cleaned).is_ok(),
            "stripped output must parse; got {cleaned}"
        );
    }

    /// Regression: a comma INSIDE a string value followed by whitespace
    /// and a brace must not be eaten by the trailing-comma pass.
    #[test]
    fn strip_tsconfig_jsonc_trailing_comma_pass_is_string_aware() {
        let input = r#"{ "a": "x, }", "b": "y" }"#;
        let cleaned = strip_tsconfig_jsonc(input);
        let v: serde_json::Value = serde_json::from_str(&cleaned).expect("must still parse");
        assert_eq!(v["a"], "x, }", "comma inside string must survive");
    }

    /// Regression for the `Path::with_extension` bug: when `extends` is
    /// `"./tsconfig.base"` (no `.json` suffix), we must resolve it to
    /// `tsconfig.base.json`, NOT `tsconfig.json`.
    ///
    /// `Path::with_extension("json")` *replaces* the existing extension, so
    /// `tsconfig.base` → `tsconfig.json`.  The fix appends `.json` to the
    /// raw path string instead.
    #[test]
    fn extends_without_json_suffix_resolves_to_dotted_name() {
        use std::fs;
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Base config is named `tsconfig.base.json` — a common pattern.
        fs::write(
            root.join("tsconfig.base.json"),
            r#"{
              "compilerOptions": {
                "paths": {
                  "@/*": ["src/*"]
                }
              }
            }"#,
        )
        .unwrap();
        // Leaf omits the `.json` suffix in extends — TypeScript accepts this.
        fs::write(
            root.join("tsconfig.json"),
            r#"{
              "extends": "./tsconfig.base"
            }"#,
        )
        .unwrap();

        let result = read_tsconfig_paths_into_map(root);
        assert!(
            result.contains_key("@/*"),
            "extends without .json suffix must resolve to tsconfig.base.json; got {result:?}"
        );
    }
}
