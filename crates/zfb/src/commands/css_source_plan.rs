//! Values-only construction of the build/dev wind source plan.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use zfb_css::{
    extract_candidates_with_options, ExtractionOptions, PositiveRoot, SourceExclusion, SourceKind,
    SourcePlan,
};

use super::build::root_package_css_excluded_dirs;
use crate::config::{Config, WindSetting};

/// Every discovery channel is supplied as a plain value; construction never reads disk.
#[derive(Clone, Debug, Default)]
pub(crate) struct CssSourcePlanInputs {
    pub project_root: PathBuf,
    pub first_party_root: PathBuf,
    pub configured_output_dir: PathBuf,
    pub pass_output_dir: PathBuf,
    pub default_content_roots: Vec<PathBuf>,
    pub declared_project_roots: Vec<PathBuf>,
    pub package_route_entrypoints: Vec<PathBuf>,
    pub sibling_mirror_roots: Vec<PathBuf>,
    pub declared_package_roots: Vec<PathBuf>,
    pub author_exclusions: Vec<SourceExclusion>,
    pub root_package_claimed: bool,
    pub root_package_excluded_dirs: Vec<PathBuf>,
    pub plugin_virtual_modules: BTreeMap<String, String>,
    pub role_classes: BTreeSet<String>,
    pub manifests: BTreeMap<String, PathBuf>,
    pub safelist: BTreeMap<String, BTreeSet<String>>,
    pub extraction_options: ExtractionOptions,
    pub zfb_written_roots: Vec<PathBuf>,
}

fn absolute(base: &Path, path: &Path) -> PathBuf {
    zfb_types::normalize_path_lexical(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    })
}

fn stable_label(prefix: &str, base: &Path, path: &Path) -> String {
    let base_parts: Vec<_> = base.components().collect();
    let path_parts: Vec<_> = path.components().collect();
    let common = base_parts
        .iter()
        .zip(&path_parts)
        .take_while(|(left, right)| left == right)
        .count();
    let ups = base_parts[common..]
        .iter()
        .filter(|part| matches!(part, std::path::Component::Normal(_)))
        .map(|_| "..");
    let downs = path_parts[common..].iter().filter_map(|part| match part {
        std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
        _ => None,
    });
    format!(
        "{prefix}/{}",
        ups.map(str::to_owned)
            .chain(downs)
            .collect::<Vec<_>>()
            .join("/")
    )
}

fn root(label: String, path: PathBuf, required: bool) -> PositiveRoot {
    PositiveRoot {
        label,
        declaring_dir: path.clone(),
        path: PathBuf::from("."),
        required,
        exclusions: BTreeSet::new(),
        package_root: false,
    }
}

/// Deterministic declaration of the authoritative build/dev scan surface.
pub(crate) fn build_css_source_plan(inputs: &CssSourcePlanInputs) -> SourcePlan {
    let project = absolute(&inputs.first_party_root, &inputs.project_root);
    let first_party = absolute(&project, &inputs.first_party_root);
    let mut plan = SourcePlan {
        extraction_options: inputs.extraction_options.clone(),
        ..SourcePlan::default()
    };
    for path in [&inputs.configured_output_dir, &inputs.pass_output_dir]
        .into_iter()
        .chain(&inputs.zfb_written_roots)
    {
        plan.exclusions.insert(absolute(&project, path));
    }

    plan.author_exclusions = inputs.author_exclusions.clone();

    let mut seen = BTreeSet::new();
    for path in &inputs.declared_package_roots {
        let path = absolute(&project, path);
        if seen.insert(path.clone()) {
            let mut package_root = root(
                stable_label("package-root", &first_party, &path),
                path,
                true,
            );
            package_root.package_root = true;
            plan.roots.push(package_root);
        }
    }
    for path in &inputs.declared_project_roots {
        let path = absolute(&project, path);
        if seen.insert(path.clone()) {
            plan.roots
                .push(root(stable_label("root", &first_party, &path), path, true));
        }
    }
    for path in &inputs.default_content_roots {
        let path = absolute(&project, path);
        if seen.insert(path.clone()) {
            plan.roots.push(root(
                stable_label("default", &first_party, &path),
                path,
                false,
            ));
        }
    }
    let packages: BTreeSet<_> = inputs
        .package_route_entrypoints
        .iter()
        .filter_map(|entry| absolute(&project, entry).parent().map(Path::to_path_buf))
        .collect();
    for path in packages {
        if seen.insert(path.clone()) {
            let label = stable_label("package-route", &first_party, &path);
            plan.package_sources
                .insert(label.clone(), root(label, path, true));
        }
    }
    let mirrors: BTreeSet<_> = inputs
        .sibling_mirror_roots
        .iter()
        .map(|path| absolute(&project, path))
        .collect();
    for path in mirrors {
        if seen.insert(path.clone()) {
            plan.roots.push(root(
                stable_label("mirror", &first_party, &path),
                path,
                true,
            ));
        }
    }
    if inputs.root_package_claimed && seen.insert(first_party.clone()) {
        let mut root_package = root("root-package".into(), first_party.clone(), true);
        root_package.exclusions = inputs
            .root_package_excluded_dirs
            .iter()
            .map(|path| absolute(&first_party, path))
            .collect();
        plan.roots.push(root_package);
    }
    plan.roots.sort();
    for (specifier, source) in &inputs.plugin_virtual_modules {
        let candidates = extract_candidates_with_options(
            source.as_bytes(),
            SourceKind::Tsx,
            &plan.extraction_options,
        )
        .candidates
        .into_iter()
        .map(|candidate| candidate.text)
        .collect();
        plan.generated_sources
            .insert(format!("plugin/{specifier}"), candidates);
    }
    plan.generated_sources.insert(
        "code-highlight/role-classes".into(),
        inputs.role_classes.clone(),
    );
    plan.manifests = inputs.manifests.clone();
    plan.safelist = inputs.safelist.clone();
    plan
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CandidateManifest {
    schema_version: u32,
    spec_version: u32,
    producer: String,
    candidates: Vec<String>,
}

fn read_candidate_manifest(path: &Path, producer: &str) -> Result<BTreeSet<String>> {
    let bytes =
        fs::read(path).with_context(|| format!("manifest {producer} at {}", path.display()))?;
    let parsed: CandidateManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("manifest {producer} at {}", path.display()))?;
    if parsed.schema_version != 1 || parsed.spec_version != 1 || parsed.producer != producer {
        bail!(
            "manifest {producer} at {} has an unsupported version or mismatched producer",
            path.display()
        );
    }
    if parsed
        .candidates
        .iter()
        .any(|candidate| candidate.is_empty())
    {
        bail!(
            "manifest {producer} at {} contains an empty candidate",
            path.display()
        );
    }
    Ok(parsed.candidates.into_iter().collect())
}

fn resolve_manifest_path(
    config_dir: &Path,
    path: &str,
    source_package: Option<&str>,
) -> Result<PathBuf> {
    let base = if let Some(package) = source_package {
        zfb_config_loader::resolve_package_dir(package, config_dir)
            .with_context(|| format!("manifest declaring package {package}"))?
    } else {
        config_dir.to_path_buf()
    };
    if path.starts_with("./") || path.starts_with("../") || Path::new(path).is_absolute() {
        return Ok(absolute(&base, Path::new(path)));
    }
    let url = zfb_config_loader::resolve_node_bare_specifier(path, &base)
        .with_context(|| format!("manifest package path {path}"))?;
    url::Url::parse(&url)?
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("manifest path {path} is not a file URL"))
}

/// A speculative identity for dev's direct watch only. Production source
/// plans always call `resolve_manifest_path`, so an invalid exports map or
/// active condition still fails exactly as the authoritative resolver says.
fn resolve_manifest_watch_path(
    config_dir: &Path,
    path: &str,
    source_package: Option<&str>,
) -> Result<PathBuf> {
    match resolve_manifest_path(config_dir, path, source_package) {
        Ok(resolved) => Ok(resolved),
        Err(error) => {
            let base = if let Some(package) = source_package {
                zfb_config_loader::resolve_package_dir(package, config_dir)?
            } else {
                config_dir.to_path_buf()
            };
            missing_package_subpath(&base, path).ok_or(error)
        }
    }
}

/// Preserve object insertion order because Node picks the first matching
/// export condition in package.json order, not alphabetic key order.
enum OrderedExportValue {
    Null,
    String(String),
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
    Other,
}

impl<'de> Deserialize<'de> for OrderedExportValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct OrderedVisitor;
        impl<'de> Visitor<'de> for OrderedVisitor {
            type Value = OrderedExportValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a JSON value")
            }

            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(OrderedExportValue::Null)
            }

            fn visit_bool<E: serde::de::Error>(
                self,
                _: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(OrderedExportValue::Other)
            }

            fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<Self::Value, E> {
                Ok(OrderedExportValue::Other)
            }

            fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<Self::Value, E> {
                Ok(OrderedExportValue::Other)
            }

            fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<Self::Value, E> {
                Ok(OrderedExportValue::Other)
            }

            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(OrderedExportValue::String(value.to_owned()))
            }

            fn visit_string<E: serde::de::Error>(
                self,
                value: String,
            ) -> std::result::Result<Self::Value, E> {
                Ok(OrderedExportValue::String(value))
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(OrderedExportValue::Array(values))
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(OrderedExportValue::Object(entries))
            }
        }
        deserializer.deserialize_any(OrderedVisitor)
    }
}

fn export_target(value: &OrderedExportValue, wildcard: Option<&str>) -> Option<PathBuf> {
    match value {
        OrderedExportValue::String(target) => {
            let target = if let Some(wildcard) = wildcard {
                target.replace('*', wildcard)
            } else {
                target.clone()
            };
            let path = Path::new(target.strip_prefix("./")?);
            if path.components().all(|component| {
                matches!(component, std::path::Component::Normal(name) if name != "node_modules")
            }) {
                Some(path.to_path_buf())
            } else {
                None
            }
        }
        OrderedExportValue::Object(conditions) => conditions
            .iter()
            .filter(|(condition, _)| matches!(condition.as_str(), "import" | "node" | "default"))
            .find_map(|(_, target)| export_target(target, wildcard)),
        OrderedExportValue::Array(targets) => targets
            .iter()
            .find_map(|target| export_target(target, wildcard)),
        OrderedExportValue::Null | OrderedExportValue::Other => None,
    }
}

/// The file-free subset of oxc_resolver's package exports selection: exact
/// keys first, then the most specific single-star pattern. `export_target`
/// follows the resolver's import/node/default condition set in JSON order.
fn exported_manifest_subpath(exports: &OrderedExportValue, subpath: &Path) -> Option<PathBuf> {
    let key = format!("./{}", subpath.to_string_lossy().replace('\\', "/"));
    let OrderedExportValue::Object(entries) = exports else {
        return None;
    };
    if let Some((_, target)) = entries.iter().find(|(export, _)| export == &key) {
        return export_target(target, None);
    }
    let mut best: Option<(&str, &OrderedExportValue, &str)> = None;
    for (export, target) in entries {
        let Some((prefix, suffix)) = export.split_once('*') else {
            continue;
        };
        if !prefix.starts_with("./")
            || suffix.contains('*')
            || !key.starts_with(prefix)
            || !key.ends_with(suffix)
            || key.len() < export.len()
        {
            continue;
        }
        let wildcard = &key[prefix.len()..key.len() - suffix.len()];
        if best.is_none_or(|(previous, _, _)| {
            let previous_base = previous.find('*').unwrap_or(previous.len());
            prefix.len() > previous_base
                || (prefix.len() == previous_base && export.len() > previous.len())
        }) {
            best = Some((export, target, wildcard));
        }
    }
    best.and_then(|(_, target, wildcard)| export_target(target, Some(wildcard)))
}

fn missing_package_subpath(base: &Path, specifier: &str) -> Option<PathBuf> {
    let mut parts = specifier.split('/');
    let first = parts.next()?;
    let package = if first.starts_with('@') {
        format!("{first}/{}", parts.next()?)
    } else {
        first.to_owned()
    };
    let tail: PathBuf = parts.collect();
    if tail.as_os_str().is_empty()
        || !tail
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    for ancestor in base.ancestors() {
        let package_dir = ancestor.join("node_modules").join(&package);
        let package_json = package_dir.join("package.json");
        let Ok(bytes) = fs::read(package_json) else {
            continue;
        };
        let Ok(metadata) = serde_json::from_slice::<OrderedExportValue>(&bytes) else {
            return None;
        };
        let OrderedExportValue::Object(fields) = metadata else {
            return None;
        };
        let target = match fields.iter().find(|(key, _)| key == "exports") {
            Some((_, exports)) => exported_manifest_subpath(exports, &tail)?,
            None => tail.clone(),
        };
        return Some(absolute(&package_dir, &target));
    }
    None
}

/// Resolve declared manifest identities without reading or validating their
/// contents. The production and dev-watch callers choose their own resolver.
fn resolve_declared_manifest_paths_with(
    project_root: &Path,
    config: &Config,
    resolve: impl Fn(&Path, &str, Option<&str>) -> Result<PathBuf>,
) -> Result<BTreeMap<String, PathBuf>> {
    let mut manifests = BTreeMap::new();
    if let Some(WindSetting::Enabled(wind)) = &config.wind {
        for (producer, declaration) in &wind.manifests {
            let path = resolve(
                wind.declaring_dir(project_root),
                &declaration.path,
                declaration.source_package.as_deref(),
            )
            .with_context(|| format!("manifest {producer} at {}", declaration.path))?;
            manifests.insert(producer.clone(), path);
        }
    }
    Ok(manifests)
}

pub(crate) fn resolve_declared_manifest_watch_paths(
    project_root: &Path,
    config: &Config,
) -> Result<BTreeMap<String, PathBuf>> {
    resolve_declared_manifest_paths_with(project_root, config, resolve_manifest_watch_path)
}

fn resolve_source_declaring_dir(
    config_dir: &Path,
    source_package: Option<&str>,
) -> Result<PathBuf> {
    match source_package {
        Some(package) => zfb_config_loader::resolve_package_dir(package, config_dir)
            .with_context(|| format!("wind.sources declaring package {package}")),
        None => Ok(config_dir.to_path_buf()),
    }
}

/// Resolve a validated `wind.sources.packageRoots` entry against its declaring
/// root. A bare name follows Node's `node_modules` lookup for the package
/// directory itself, so a package without a root export still resolves.
fn resolve_declared_package_root(
    project_root: &Path,
    declaring_dir: &Path,
    root: &str,
) -> Result<PathBuf> {
    let path = if root == "." || root == ".." || root.starts_with("./") || root.starts_with("../") {
        absolute(declaring_dir, Path::new(root))
    } else {
        let mut parts = root.splitn(if root.starts_with('@') { 3 } else { 2 }, '/');
        let name: PathBuf = parts
            .by_ref()
            .take(if root.starts_with('@') { 2 } else { 1 })
            .collect();
        let subpath = parts.next().unwrap_or_default();
        let package = declaring_dir
            .ancestors()
            .map(|ancestor| ancestor.join("node_modules").join(&name))
            .find(|candidate| candidate.is_dir())
            .with_context(|| {
                format!(
                    "package {} is not installed in a node_modules directory above {}",
                    name.display(),
                    declaring_dir.display()
                )
            })?;
        absolute(&package, Path::new(subpath))
    };
    if !path.is_dir() {
        bail!("package root {} is not a directory", path.display());
    }
    // A root containing the project would scan and watch the whole project
    // or workspace; the default roots already cover the project itself.
    let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if project_root.starts_with(&path) || canonical(project_root).starts_with(canonical(&path)) {
        bail!(
            "package root {} is the project root or one of its ancestors; declare a package directory below it",
            path.display()
        );
    }
    Ok(path)
}

/// A project source root must remain strictly below its own declaring project
/// or preset directory, including after following symlinks.
fn resolve_declared_project_root(declaring_dir: &Path, root: &str) -> Result<PathBuf> {
    if root.is_empty()
        || root == "."
        || root.contains('\\')
        || Path::new(root).is_absolute()
        || root.as_bytes().get(1) == Some(&b':')
    {
        bail!("project root must name a relative directory below the declaring root");
    }
    let declaring = fs::canonicalize(declaring_dir).with_context(|| {
        format!(
            "declaring root {} is missing or unreadable",
            declaring_dir.display()
        )
    })?;
    let path = absolute(declaring_dir, Path::new(root));
    let canonical = fs::canonicalize(&path)
        .with_context(|| format!("project root {} is missing or unreadable", path.display()))?;
    if !canonical.is_dir() || !canonical.starts_with(&declaring) || canonical == declaring {
        bail!(
            "project root {} must be a directory below {}",
            path.display(),
            declaring.display()
        );
    }
    Ok(canonical)
}

/// Declared package roots for dev's recursive watch. A declaration that does
/// not resolve yet is skipped; its config or install edit triggers the next pass.
pub(crate) fn declared_package_root_watch_paths(
    project_root: &Path,
    config: &Config,
) -> Vec<PathBuf> {
    let Some(WindSetting::Enabled(wind)) = &config.wind else {
        return Vec::new();
    };
    let project_root = zfb_types::normalize_path_lexical(project_root);
    wind.source_declarations()
        .iter()
        .filter_map(|declaration| {
            resolve_source_declaring_dir(
                wind.declaring_dir(&project_root),
                declaration.source_package.as_deref(),
            )
            .ok()
            .map(|dir| (dir, declaration))
        })
        .flat_map(|(dir, declaration)| {
            declaration
                .sources
                .package_roots
                .iter()
                .filter_map(|root| resolve_declared_package_root(&project_root, &dir, root).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Resolved project roots for the direct CSS walk and dev's recursive watch.
/// The source plan performs authoritative validation; this snapshot is only
/// used to arm watches and collect module import sources ahead of indexing.
pub(crate) fn declared_project_root_paths(project_root: &Path, config: &Config) -> Vec<PathBuf> {
    let Some(WindSetting::Enabled(wind)) = &config.wind else {
        return Vec::new();
    };
    let project_root = zfb_types::normalize_path_lexical(project_root);
    let mut roots = BTreeSet::new();
    for declaration in wind.source_declarations() {
        let Ok(dir) = resolve_source_declaring_dir(
            wind.declaring_dir(&project_root),
            declaration.source_package.as_deref(),
        ) else {
            continue;
        };
        for root in &declaration.sources.roots {
            if let Ok(path) = resolve_declared_project_root(&dir, root) {
                roots.insert(path);
            }
        }
    }
    roots.into_iter().collect()
}

/// Read workspace claims and declared manifests. The caller supplies already computed routes and mirrors.
pub(crate) fn gather_css_source_plan_inputs(
    project_root: &Path,
    pass_output_dir: &Path,
    config: &Config,
    package_route_entrypoints: &[PathBuf],
    sibling_mirror_roots: &[PathBuf],
    plugin_virtual_modules: &[(String, String)],
    zfb_written_roots: &[PathBuf],
) -> Result<CssSourcePlanInputs> {
    let project_root = zfb_types::normalize_path_lexical(project_root);
    let first_party_root = zfb_types::first_party::first_party_root_for(&project_root);
    let root_package_claimed =
        zfb_types::first_party::workspace_explicitly_claims_root_package(&project_root);
    let manifests =
        resolve_declared_manifest_paths_with(&project_root, config, resolve_manifest_path)?;
    let mut safelist = BTreeMap::new();
    let mut extraction_options = ExtractionOptions::default();
    let mut declared_package_roots = Vec::new();
    let mut declared_project_roots = Vec::new();
    let mut author_exclusions = Vec::new();
    if let Some(WindSetting::Enabled(wind)) = &config.wind {
        extraction_options
            .class_helpers
            .extend(wind.sources.class_helpers.iter().cloned());
        for declaration in wind.source_declarations() {
            extraction_options
                .class_helpers
                .extend(declaration.sources.class_helpers.iter().cloned());
            let origin = declaration.origin();
            let declaring_dir = resolve_source_declaring_dir(
                wind.declaring_dir(&project_root),
                declaration.source_package.as_deref(),
            )?;
            for (index, root) in declaration.sources.package_roots.iter().enumerate() {
                declared_package_roots.push(
                    resolve_declared_package_root(&project_root, &declaring_dir, root)
                        .with_context(|| {
                            format!(
                                "wind.sources.packageRoots[{index}] {root:?} declared by {origin}"
                            )
                        })?,
                );
            }
            for (index, root) in declaration.sources.roots.iter().enumerate() {
                declared_project_roots.push(
                    resolve_declared_project_root(&declaring_dir, root).with_context(|| {
                        format!("wind.sources.roots[{index}] {root:?} declared by {origin}")
                    })?,
                );
            }
            author_exclusions.extend(declaration.sources.exclude.iter().map(|pattern| {
                SourceExclusion {
                    origin: origin.clone(),
                    declaring_dir: declaring_dir.clone(),
                    pattern: pattern.clone(),
                }
            }));
        }
        safelist = wind
            .safelist
            .iter()
            .map(|(owner, values)| (owner.clone(), values.iter().cloned().collect()))
            .collect();
        for (producer, path) in &manifests {
            read_candidate_manifest(path, producer)?;
        }
    }
    let role_classes = super::css_support::role_classes_inline_sources(config)
        .into_iter()
        .collect();
    Ok(CssSourcePlanInputs {
        default_content_roots: zfb_css::engine::DEFAULT_CONTENT_ROOTS
            .iter()
            .map(|root| project_root.join(root))
            .collect(),
        configured_output_dir: absolute(&project_root, &config.out_dir),
        pass_output_dir: absolute(&project_root, pass_output_dir),
        root_package_excluded_dirs: if root_package_claimed {
            root_package_css_excluded_dirs(&project_root, &first_party_root)
        } else {
            Vec::new()
        },
        project_root,
        first_party_root,
        package_route_entrypoints: package_route_entrypoints.to_vec(),
        sibling_mirror_roots: sibling_mirror_roots.to_vec(),
        declared_package_roots,
        declared_project_roots,
        author_exclusions,
        root_package_claimed,
        plugin_virtual_modules: plugin_virtual_modules.iter().cloned().collect(),
        role_classes,
        manifests,
        safelist,
        extraction_options,
        zfb_written_roots: zfb_written_roots.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zfb_css::{expand_file_set, CandidateIndex, SourceId};

    fn fixture() -> (tempfile::TempDir, CssSourcePlanInputs) {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let inputs = CssSourcePlanInputs {
            project_root: project.clone(),
            first_party_root: temp.path().to_path_buf(),
            configured_output_dir: project.join("src/custom-output"),
            pass_output_dir: project.join("src/assets-current"),
            default_content_roots: vec![project.join("src")],
            zfb_written_roots: zfb_types::ScratchLayout::default_for(&project).written_roots(),
            ..Default::default()
        };
        (temp, inputs)
    }

    fn write(root: &Path, relative: &str, class: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!("export const x = <div className=\"{class}\" />;"),
        )
        .unwrap();
    }

    fn live(plan: &SourcePlan) -> BTreeSet<String> {
        let files = expand_file_set(plan);
        assert!(files.diagnostics.is_empty(), "{:?}", files.diagnostics);
        let mut index = CandidateIndex::default();
        for file in files.files {
            index.index_file(&file).unwrap();
        }
        for (producer, values) in &plan.generated_sources {
            index.upsert(
                SourceId::new("generated", producer).unwrap(),
                values.iter().cloned(),
            );
        }
        for (owner, values) in &plan.safelist {
            index.replace_safelist(owner, values.iter().cloned());
        }
        index.live_set()
    }

    #[test]
    fn helper_options_change_plan_identity_and_reach_plugin_extraction() {
        let (_temp, mut inputs) = fixture();
        inputs.plugin_virtual_modules.insert(
            "virtual:page".into(),
            "export const Page = () => <div class={ctl(\"text-xl\")} />;".into(),
        );
        let default = build_css_source_plan(&inputs);
        assert!(default.generated_sources["plugin/virtual:page"].contains("text-xl"));
        inputs.extraction_options.class_helpers.insert("ctl".into());
        let configured = build_css_source_plan(&inputs);
        assert_ne!(default, configured);
        assert!(configured.extraction_options.class_helpers.contains("ctl"));
        let config = sources_config(vec![(None, serde_json::json!({"classHelpers": ["ctl"]}))]);
        let gathered = gather(&inputs, &config).unwrap();
        assert!(gathered.extraction_options.class_helpers.contains("ctl"));
        let configured_plan = build_css_source_plan(&gathered);
        let extraction = extract_candidates_with_options(
            gathered.plugin_virtual_modules["virtual:page"].as_bytes(),
            SourceKind::Tsx,
            &configured_plan.extraction_options,
        );
        let candidate = extraction
            .candidates
            .iter()
            .find(|c| c.text == "text-xl")
            .unwrap();
        assert!(candidate
            .occurrences
            .iter()
            .all(|o| o.position_kind == zfb_css::PositionKind::Class));
    }

    #[test]
    fn wind_source_plan_default_root_and_output_exclusions() {
        let (_temp, inputs) = fixture();
        let project = &inputs.project_root;
        write(project, "src/page.tsx", "p-1");
        write(project, "src/custom-output/old.tsx", "p-2");
        write(project, "src/assets-current/old.tsx", "p-3");
        write(project, "src/.zfb-build/old.tsx", "p-4");
        write(project, "src/node_modules/old.tsx", "p-5");
        write(project, "src/page.html", "p-6");
        write(project, "src/dist/old.tsx", "p-9");
        let plan = build_css_source_plan(&inputs);
        let candidates = live(&plan);
        assert!(candidates.contains("p-1"));
        for decoy in ["p-2", "p-3", "p-4", "p-5", "p-6", "p-9"] {
            assert!(!candidates.contains(decoy), "{decoy}");
        }
        let mut without_output_exclusions = plan;
        without_output_exclusions
            .exclusions
            .remove(&inputs.configured_output_dir);
        without_output_exclusions
            .exclusions
            .remove(&inputs.pass_output_dir);
        let leaked = live(&without_output_exclusions);
        assert!(leaked.contains("p-2"));
        assert!(leaked.contains("p-3"));
    }

    #[test]
    fn wind_source_plan_package_route_inside_node_modules_dist() {
        let (_temp, mut inputs) = fixture();
        write(
            &inputs.project_root,
            "node_modules/widgets/dist/route.tsx",
            "m-1",
        );
        inputs.package_route_entrypoints.push(
            inputs
                .project_root
                .join("node_modules/widgets/dist/route.tsx"),
        );
        assert!(live(&build_css_source_plan(&inputs)).contains("m-1"));
    }

    #[test]
    fn wind_source_plan_mirror_and_root_package_do_not_cancel_defaults() {
        let (temp, mut inputs) = fixture();
        let root = temp.path();
        write(&inputs.project_root, "src/page.tsx", "p-1");
        write(root, "root.tsx", "p-2");
        write(root, "sibling/page.tsx", "p-3");
        write(root, "unmirrored/page.tsx", "p-4");
        inputs.root_package_claimed = true;
        inputs.root_package_excluded_dirs = vec![
            inputs.project_root.clone(),
            root.join("sibling"),
            root.join("unmirrored"),
        ];
        inputs.sibling_mirror_roots.push(root.join("sibling"));
        let candidates = live(&build_css_source_plan(&inputs));
        for expected in ["p-1", "p-2", "p-3"] {
            assert!(candidates.contains(expected), "{expected}");
        }
        assert!(!candidates.contains("p-4"));
    }

    #[test]
    fn wind_source_plan_excludes_an_in_project_session_scratch_root() {
        let (_temp, mut inputs) = fixture();
        let project = inputs.project_root.clone();
        let scratch = project.join(".zfb-build/session-a");
        inputs.default_content_roots = vec![project.clone()];
        inputs.zfb_written_roots =
            zfb_types::ScratchLayout::for_scratch_dir(&project, scratch.clone()).written_roots();
        write(&project, "src/page.tsx", "p-1");
        write(&scratch, "bundle.tsx", "p-7");
        let plan = build_css_source_plan(&inputs);
        assert!(plan.exclusions.contains(&scratch));
        let candidates = live(&plan);
        assert!(candidates.contains("p-1"));
        assert!(!candidates.contains("p-7"));
    }

    #[test]
    fn wind_source_plan_excludes_an_out_of_project_session_root_in_a_claimed_workspace() {
        let (temp, mut inputs) = fixture();
        let root = temp.path();
        let scratch = root.join("scratch-x");
        write(root, "root.tsx", "p-2");
        write(&scratch, "bundle.tsx", "p-8");
        inputs.root_package_claimed = true;
        inputs.root_package_excluded_dirs = vec![inputs.project_root.clone()];
        inputs.zfb_written_roots =
            zfb_types::ScratchLayout::for_scratch_dir(&inputs.project_root, scratch.clone())
                .written_roots();
        let plan = build_css_source_plan(&inputs);
        let candidates = live(&plan);
        assert!(candidates.contains("p-2"));
        assert!(!candidates.contains("p-8"));

        let mut without_scratch = plan;
        without_scratch.exclusions.remove(&scratch);
        assert!(
            live(&without_scratch).contains("p-8"),
            "the claimed root package walks the scratch root unless it is excluded"
        );
    }

    #[test]
    fn wind_source_plan_plugin_role_classes_and_safelist() {
        let (_temp, mut inputs) = fixture();
        inputs
            .plugin_virtual_modules
            .insert("virtual:widget".into(), "<div className=\"p-7\" />".into());
        inputs.role_classes.insert("my-keyword-class".into());
        inputs
            .safelist
            .insert("app".into(), BTreeSet::from(["m-8".into()]));
        let plan = build_css_source_plan(&inputs);
        let candidates = live(&plan);
        for expected in ["p-7", "my-keyword-class", "m-8"] {
            assert!(candidates.contains(expected));
        }
        assert_eq!(
            plan.generated_sources["code-highlight/role-classes"],
            BTreeSet::from(["my-keyword-class".into()])
        );
        assert!(!plan.safelist["app"].contains("my-keyword-class"));
    }

    #[test]
    fn wind_source_plan_manifest_is_declared_and_replacement_retracts() {
        let (_temp, mut inputs) = fixture();
        let path = inputs.project_root.join("node_modules/widgets/wind.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"schemaVersion\":1,\"specVersion\":1,\"producer\":\"widgets\",\"candidates\":[\"p-1\"]}").unwrap();
        assert!(build_css_source_plan(&inputs).manifests.is_empty());
        inputs.manifests.insert("widgets".into(), path.clone());
        let plan = build_css_source_plan(&inputs);
        assert!(plan.manifests.contains_key("widgets"));
        let mut index = CandidateIndex::default();
        index.replace_manifest(
            "widgets",
            read_candidate_manifest(&path, "widgets").unwrap(),
        );
        fs::write(&path, "{\"schemaVersion\":1,\"specVersion\":1,\"producer\":\"widgets\",\"candidates\":[\"p-2\"]}").unwrap();
        index.replace_manifest(
            "widgets",
            read_candidate_manifest(&path, "widgets").unwrap(),
        );
        assert_eq!(index.live_set(), BTreeSet::from(["p-2".into()]));
    }

    #[test]
    fn wind_source_plan_order_and_identity_are_stable() {
        let (_temp, mut inputs) = fixture();
        inputs
            .default_content_roots
            .push(inputs.project_root.join("pages"));
        let expected = build_css_source_plan(&inputs);
        inputs.default_content_roots.reverse();
        assert_eq!(build_css_source_plan(&inputs), expected);
        for root in &expected.roots {
            assert!(!root.label.contains(_temp.path().to_str().unwrap()));
        }
    }

    #[test]
    fn wind_source_plan_gather_manifest_and_workspace_claim() {
        let (temp, inputs) = fixture();
        let root = temp.path();
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - '.'\n  - 'project'\n",
        )
        .unwrap();
        let manifest = inputs.project_root.join("wind.json");
        fs::write(&manifest, "{\"schemaVersion\":1,\"specVersion\":1,\"producer\":\"widgets\",\"candidates\":[\"p-1\"]}").unwrap();
        let mut config = Config::default();
        let mut wind = crate::config::WindConfig::default();
        wind.manifests.insert(
            "widgets".into(),
            crate::config::WindManifest {
                path: "./wind.json".into(),
                source_package: None,
            },
        );
        wind.safelist.insert("app".into(), vec!["m-1".into()]);
        config.wind = Some(WindSetting::Enabled(Box::new(wind)));
        let gathered = gather_css_source_plan_inputs(
            &inputs.project_root,
            &inputs.pass_output_dir,
            &config,
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap();
        assert!(gathered.root_package_claimed);
        assert!(gathered
            .root_package_excluded_dirs
            .contains(&inputs.project_root));
        assert_eq!(gathered.manifests["widgets"], manifest);
        assert!(gathered.safelist["app"].contains("m-1"));
        let plan = build_css_source_plan(&gathered);
        assert!(plan.exclusions.contains(&inputs.project_root.join("dist")));
        assert!(plan.exclusions.contains(&inputs.pass_output_dir));
    }

    fn sources_config(declarations: Vec<(Option<&str>, serde_json::Value)>) -> Config {
        let mut wind = crate::config::WindConfig::default();
        for (source_package, sources) in declarations {
            wind.source_declarations
                .push(crate::config::WindSourceDeclaration {
                    source_package: source_package.map(str::to_owned),
                    sources: serde_json::from_value(sources).unwrap(),
                });
        }
        Config {
            wind: Some(WindSetting::Enabled(Box::new(wind))),
            ..Config::default()
        }
    }

    fn gather(inputs: &CssSourcePlanInputs, config: &Config) -> Result<CssSourcePlanInputs> {
        gather_css_source_plan_inputs(
            &inputs.project_root,
            &inputs.pass_output_dir,
            config,
            &[],
            &[],
            &[],
            &inputs.zfb_written_roots,
        )
    }

    fn install_package(project: &Path, name: &str) -> PathBuf {
        let package = project.join("node_modules").join(name);
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("package.json"),
            format!("{{\"name\":\"{name}\",\"main\":\"index.js\"}}"),
        )
        .unwrap();
        fs::write(package.join("index.js"), "export {};").unwrap();
        package
    }

    #[test]
    fn project_roots_keep_provenance_ignores_exclusions_and_declarations_out() {
        let (_temp, inputs) = fixture();
        let project = &inputs.project_root;
        let preset = install_package(project, "@scope/preset");
        write(project, "ui/card.tsx", "p-1");
        write(project, "ui/card.test.ts", "p-2");
        write(project, "ui/ignored.tsx", "p-3");
        write(project, "ui/excluded.tsx", "p-4");
        write(project, "ui/types.d.ts", "p-5");
        write(project, "ui/nested/card.tsx", "p-9");
        write(project, "src/shared.tsx", "p-6");
        write(&preset, "ui/preset.tsx", "p-7");
        write(&preset, "ui/types.d.ts", "p-8");
        fs::write(project.join(".gitignore"), "ui/ignored.tsx\n").unwrap();
        let config = sources_config(vec![
            (
                Some("@scope/preset"),
                serde_json::json!({ "roots": ["./ui"] }),
            ),
            (
                None,
                serde_json::json!({ "roots": ["./ui", "./src", "./ui", "./ui/nested"], "exclude": ["ui/excluded.tsx"] }),
            ),
        ]);
        let gathered = gather(&inputs, &config).unwrap();
        let plan = build_css_source_plan(&gathered);
        assert_eq!(gathered.declared_project_roots.len(), 5);
        let files = expand_file_set(&plan);
        assert!(files.diagnostics.is_empty(), "{:?}", files.diagnostics);
        let ids: Vec<_> = files.files.iter().map(|file| file.id.render()).collect();
        assert!(
            ids.iter()
                .any(|id| id.starts_with("root/") && id.ends_with("/ui:card.tsx")),
            "{ids:?}"
        );
        assert!(
            ids.iter()
                .any(|id| id.starts_with("root/") && id.ends_with("/ui:card.test.ts")),
            "{ids:?}"
        );
        assert!(
            ids.iter()
                .any(|id| id.starts_with("root/") && id.ends_with("/src:shared.tsx")),
            "{ids:?}"
        );
        assert!(ids.iter().any(|id| id.ends_with(":preset.tsx")), "{ids:?}");
        assert!(
            ids.iter().any(|id| id.ends_with("/ui/nested:card.tsx")),
            "{ids:?}"
        );
        assert_eq!(ids.len(), 5, "{ids:?}");
    }

    #[test]
    fn project_roots_reject_ancestors_missing_dirs_and_symlink_escapes() {
        let (temp, inputs) = fixture();
        let project = &inputs.project_root;
        fs::create_dir_all(project.join("ui")).unwrap();
        fs::write(project.join("file.ts"), "").unwrap();
        for root in [".", "..", "../outside", "./missing", "./file.ts"] {
            let config = sources_config(vec![(None, serde_json::json!({ "roots": [root] }))]);
            let error = format!("{:#}", gather(&inputs, &config).unwrap_err());
            assert!(error.contains("wind.sources.roots[0]"), "{root}: {error}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(temp.path(), project.join("outside-link")).unwrap();
            let config = sources_config(vec![(
                None,
                serde_json::json!({ "roots": ["./outside-link"] }),
            )]);
            let error = format!("{:#}", gather(&inputs, &config).unwrap_err());
            assert!(error.contains("wind.sources.roots[0]"), "{error}");
        }
    }

    #[test]
    fn wind_sources_default_plan_is_unchanged() {
        let (_temp, inputs) = fixture();
        let without = gather(&inputs, &Config::default()).unwrap();
        let with_empty = gather(&inputs, &sources_config(Vec::new())).unwrap();
        assert_eq!(
            build_css_source_plan(&without),
            build_css_source_plan(&with_empty)
        );
        assert!(build_css_source_plan(&without).author_exclusions.is_empty());
    }

    #[test]
    fn wind_sources_exclusions_are_scoped_to_their_declaring_root() {
        let (_temp, inputs) = fixture();
        let project = inputs.project_root.clone();
        let preset = install_package(&project, "@scope/preset");
        write(&project, "src/page.tsx", "p-1");
        write(&project, "src/__tests__/page.test.tsx", "p-2");
        write(&project, "src/worker/index.ts", "p-3");
        write(&preset, "src/view.tsx", "p-4");
        write(&preset, "fixtures/demo.tsx", "p-5");
        let config = sources_config(vec![
            (
                Some("@scope/preset"),
                serde_json::json!({ "exclude": ["fixtures/**"], "packageRoots": ["."] }),
            ),
            (
                None,
                serde_json::json!({ "exclude": ["src/**/__tests__/**", "src/worker"] }),
            ),
        ]);
        let plan = build_css_source_plan(&gather(&inputs, &config).unwrap());
        assert_eq!(
            plan.author_exclusions
                .iter()
                .map(|exclusion| (exclusion.origin.as_str(), exclusion.pattern.as_str()))
                .collect::<Vec<_>>(),
            [
                ("preset:@scope/preset", "fixtures/**"),
                ("project", "src/**/__tests__/**"),
                ("project", "src/worker"),
            ]
        );
        let canonical = |path: &Path| fs::canonicalize(path).unwrap();
        assert_eq!(
            canonical(&plan.author_exclusions[0].declaring_dir),
            canonical(&preset)
        );
        assert_eq!(plan.author_exclusions[1].declaring_dir, project);
        let candidates = live(&plan);
        for expected in ["p-1", "p-4"] {
            assert!(candidates.contains(expected), "{expected}");
        }
        for excluded in ["p-2", "p-3", "p-5"] {
            assert!(!candidates.contains(excluded), "{excluded}");
        }
    }

    #[test]
    fn wind_sources_package_roots_traverse_dist_but_not_mandatory_exclusions() {
        let (_temp, mut inputs) = fixture();
        let project = inputs.project_root.clone();
        let ui = install_package(&project, "@scope/ui");
        write(&ui, "dist/route.tsx", "p-1");
        write(&project, "packages/local/dist/view.tsx", "p-2");
        write(&project, "packages/local/out/stale.tsx", "p-3");
        inputs.pass_output_dir = project.join("packages/local/out");
        let config = sources_config(vec![(
            None,
            serde_json::json!({ "packageRoots": ["@scope/ui", "./packages/local", "@scope/ui/dist"] }),
        )]);
        let gathered = gather(&inputs, &config).unwrap();
        assert_eq!(
            gathered.declared_package_roots,
            [ui.clone(), project.join("packages/local"), ui.join("dist")]
        );
        let plan = build_css_source_plan(&gathered);
        let labels: Vec<_> = plan
            .roots
            .iter()
            .filter(|root| root.package_root)
            .map(|root| root.label.as_str())
            .collect();
        assert_eq!(
            labels,
            [
                "package-root/node_modules/@scope/ui",
                "package-root/node_modules/@scope/ui/dist",
                "package-root/packages/local",
            ]
        );
        let files = expand_file_set(&plan);
        assert_eq!(
            files
                .files
                .iter()
                .filter(|file| file.path.ends_with("dist/route.tsx"))
                .count(),
            1,
            "a nested duplicate package root yields its files once"
        );
        let candidates = live(&plan);
        assert!(candidates.contains("p-1"));
        assert!(candidates.contains("p-2"));
        assert!(!candidates.contains("p-3"));
    }

    #[test]
    fn wind_sources_reject_package_roots_containing_the_project() {
        let (_temp, inputs) = fixture();
        fs::create_dir_all(inputs.project_root.join("packages/ui")).unwrap();
        for root in [".", "./", "..", "../project/.."] {
            let config =
                sources_config(vec![(None, serde_json::json!({ "packageRoots": [root] }))]);
            let error = format!("{:#}", gather(&inputs, &config).unwrap_err());
            assert!(error.contains("wind.sources.packageRoots[0]"), "{error}");
            assert!(error.contains("declared by project"), "{error}");
            assert!(error.contains("ancestors"), "{error}");
            assert!(
                declared_package_root_watch_paths(&inputs.project_root, &config).is_empty(),
                "{root} must never become a watch root"
            );
        }
        let config = sources_config(vec![(
            None,
            serde_json::json!({ "packageRoots": ["./packages/ui"] }),
        )]);
        assert_eq!(
            declared_package_root_watch_paths(&inputs.project_root, &config),
            [inputs.project_root.join("packages/ui")]
        );
    }

    #[test]
    fn explicit_config_dir_anchors_config_relative_references() {
        let (temp, inputs) = fixture();
        let canonical = |path: &Path| fs::canonicalize(path).unwrap();
        let config_dir = temp.path().join("config");
        let preset = install_package(&config_dir, "@scope/preset");
        fs::create_dir_all(config_dir.join("packages/ui")).unwrap();
        fs::create_dir_all(preset.join("ui")).unwrap();
        let manifest = |path: &Path, producer: &str| {
            fs::write(
                path,
                format!("{{\"schemaVersion\":1,\"specVersion\":1,\"producer\":\"{producer}\",\"candidates\":[\"p-1\"]}}"),
            )
            .unwrap();
        };
        manifest(&config_dir.join("wind.json"), "local");
        manifest(&preset.join("wind.json"), "preset");
        let mut config = sources_config(vec![
            (
                None,
                serde_json::json!({ "packageRoots": ["./packages/ui"] }),
            ),
            (
                Some("@scope/preset"),
                serde_json::json!({ "packageRoots": ["./ui"] }),
            ),
        ]);
        let Some(WindSetting::Enabled(wind)) = &mut config.wind else {
            unreachable!()
        };
        for (producer, source_package) in [("local", None), ("preset", Some("@scope/preset"))] {
            wind.manifests.insert(
                producer.into(),
                crate::config::WindManifest {
                    path: "./wind.json".into(),
                    source_package: source_package.map(str::to_owned),
                },
            );
        }

        let error = format!("{:#}", gather(&inputs, &config).unwrap_err());
        assert!(error.contains("@scope/preset"), "{error}");

        let Some(WindSetting::Enabled(wind)) = &mut config.wind else {
            unreachable!()
        };
        wind.config_dir = Some(config_dir.clone());
        let gathered = gather(&inputs, &config).unwrap();
        assert_eq!(gathered.manifests["local"], config_dir.join("wind.json"));
        assert_eq!(
            canonical(&gathered.manifests["preset"]),
            canonical(&preset.join("wind.json"))
        );
        assert_eq!(
            gathered
                .declared_package_roots
                .iter()
                .map(|root| canonical(root))
                .collect::<Vec<_>>(),
            [
                canonical(&config_dir.join("packages/ui")),
                canonical(&preset.join("ui"))
            ]
        );
        assert_eq!(gathered.project_root, inputs.project_root);
    }

    #[test]
    fn wind_sources_missing_package_roots_name_their_declaration() {
        let (_temp, inputs) = fixture();
        install_package(&inputs.project_root, "@scope/preset");
        for (source_package, root, needle) in [
            (None, "@scope/absent", "not installed"),
            (None, "./absent", "not a directory"),
            (Some("@scope/preset"), "./absent", "not a directory"),
        ] {
            let config = sources_config(vec![(
                source_package,
                serde_json::json!({ "packageRoots": [root] }),
            )]);
            let error = format!("{:#}", gather(&inputs, &config).unwrap_err());
            assert!(error.contains("wind.sources.packageRoots[0]"), "{error}");
            assert!(error.contains(root), "{error}");
            assert!(error.contains(needle), "{error}");
            let origin =
                source_package.map_or("project".to_owned(), |package| format!("preset:{package}"));
            assert!(error.contains(&origin), "{error}");
        }
    }

    #[test]
    fn missing_bare_manifest_subpath_follows_package_exports_and_conditions() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let package = project.join("node_modules/@fixture/widgets");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("package.json"),
            "{\"name\":\"@fixture/widgets\",\"version\":\"1.0.0\"}",
        )
        .unwrap();
        assert_eq!(
            resolve_manifest_watch_path(&project, "@fixture/widgets/wind.json", None).unwrap(),
            package.join("wind.json")
        );
        fs::write(
            package.join("package.json"),
            "{\"name\":\"@fixture/widgets\",\"exports\":{\"./wind.json\":{\"import\":\"./dist/wind.json\",\"default\":\"./fallback/wind.json\"}}}",
        )
        .unwrap();
        assert_eq!(
            resolve_manifest_watch_path(&project, "@fixture/widgets/wind.json", None).unwrap(),
            package.join("dist/wind.json")
        );
        fs::write(
            package.join("package.json"),
            "{\"name\":\"@fixture/widgets\",\"exports\":{\"./wind.json\":{\"default\":\"./fallback/wind.json\",\"import\":\"./dist/wind.json\"}}}",
        )
        .unwrap();
        assert_eq!(
            missing_package_subpath(&project, "@fixture/widgets/wind.json").unwrap(),
            package.join("fallback/wind.json"),
            "first matching condition in declaration order wins"
        );
        fs::write(
            package.join("package.json"),
            "{\"name\":\"@fixture/widgets\",\"exports\":{\"./*\":\"./fallback/*\",\"./wind.*\":{\"import\":\"./dist/wind.*\"}}}",
        )
        .unwrap();
        assert_eq!(
            missing_package_subpath(&project, "@fixture/widgets/wind.json").unwrap(),
            package.join("dist/wind.json"),
            "the most specific export pattern wins"
        );
    }

    #[test]
    fn production_manifest_resolution_rejects_invalid_active_and_mixed_exports() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let package = project.join("node_modules/@fixture/widgets");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("wind.json"), "{}").unwrap();
        fs::write(
            package.join("package.json"),
            "{\"name\":\"@fixture/widgets\",\"exports\":{\"./wind.json\":{\"import\":\"../outside.json\",\"default\":\"./wind.json\"}}}",
        )
        .unwrap();
        assert!(
            resolve_manifest_path(&project, "@fixture/widgets/wind.json", None).is_err(),
            "invalid active import target must not fall through to default"
        );

        fs::write(
            package.join("package.json"),
            "{\"name\":\"@fixture/widgets\",\"exports\":{\"./wind.json\":\"./wind.json\",\"default\":\"./wind.json\"}}",
        )
        .unwrap();
        assert!(
            resolve_manifest_path(&project, "@fixture/widgets/wind.json", None).is_err(),
            "mixed subpath and condition keys must remain invalid"
        );
    }

    #[test]
    fn wind_source_plan_gather_invalid_declared_manifest_names_producer() {
        let (_temp, inputs) = fixture();
        let manifest = inputs.project_root.join("wind.json");
        fs::write(
            &manifest,
            "{\"schemaVersion\":1,\"specVersion\":1,\"producer\":\"wrong\",\"candidates\":[]}",
        )
        .unwrap();
        let mut config = Config::default();
        let mut wind = crate::config::WindConfig::default();
        wind.manifests.insert(
            "widgets".into(),
            crate::config::WindManifest {
                path: "./wind.json".into(),
                source_package: None,
            },
        );
        config.wind = Some(WindSetting::Enabled(Box::new(wind)));
        let error = gather_css_source_plan_inputs(
            &inputs.project_root,
            &inputs.pass_output_dir,
            &config,
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert!(error.to_string().contains("widgets"));
        assert!(error.to_string().contains("wind.json"));
    }
}
