//! Values-only construction of the build/dev wind source plan.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use zfb_css::{extract_candidates, PositiveRoot, SourceKind, SourcePlan};

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
    pub package_route_entrypoints: Vec<PathBuf>,
    pub sibling_mirror_roots: Vec<PathBuf>,
    pub root_package_claimed: bool,
    pub root_package_excluded_dirs: Vec<PathBuf>,
    pub plugin_virtual_modules: BTreeMap<String, String>,
    pub role_classes: BTreeSet<String>,
    pub manifests: BTreeMap<String, PathBuf>,
    pub safelist: BTreeMap<String, BTreeSet<String>>,
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
    }
}

/// Deterministic declaration of the authoritative build/dev scan surface.
pub(crate) fn build_css_source_plan(inputs: &CssSourcePlanInputs) -> SourcePlan {
    let project = absolute(&inputs.first_party_root, &inputs.project_root);
    let first_party = absolute(&project, &inputs.first_party_root);
    let mut plan = SourcePlan::default();
    for path in [
        &inputs.configured_output_dir,
        &inputs.pass_output_dir,
        &project.join(".zfb-build"),
        &project.join(".zfb"),
    ] {
        plan.exclusions.insert(absolute(&project, path));
    }

    let mut seen = BTreeSet::new();
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
        let candidates = extract_candidates(source.as_bytes(), SourceKind::Tsx)
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
    project_root: &Path,
    path: &str,
    source_package: Option<&str>,
) -> Result<PathBuf> {
    let base = if let Some(package) = source_package {
        zfb_config_loader::resolve_package_dir(package, project_root)
            .with_context(|| format!("manifest declaring package {package}"))?
    } else {
        project_root.to_path_buf()
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

/// Read workspace claims and declared manifests. The caller supplies already computed routes and mirrors.
pub(crate) fn gather_css_source_plan_inputs(
    project_root: &Path,
    pass_output_dir: &Path,
    config: &Config,
    package_route_entrypoints: &[PathBuf],
    sibling_mirror_roots: &[PathBuf],
    plugin_virtual_modules: &[(String, String)],
) -> Result<CssSourcePlanInputs> {
    let project_root = zfb_types::normalize_path_lexical(project_root);
    let first_party_root = zfb_types::first_party::first_party_root_for(&project_root);
    let root_package_claimed =
        zfb_types::first_party::workspace_explicitly_claims_root_package(&project_root);
    let mut manifests = BTreeMap::new();
    let mut safelist = BTreeMap::new();
    if let Some(WindSetting::Enabled(wind)) = &config.wind {
        safelist = wind
            .safelist
            .iter()
            .map(|(owner, values)| (owner.clone(), values.iter().cloned().collect()))
            .collect();
        for (producer, declaration) in &wind.manifests {
            let path = resolve_manifest_path(
                &project_root,
                &declaration.path,
                declaration.source_package.as_deref(),
            )
            .with_context(|| format!("manifest {producer} at {}", declaration.path))?;
            read_candidate_manifest(&path, producer)?;
            manifests.insert(producer.clone(), path);
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
        root_package_claimed,
        plugin_virtual_modules: plugin_virtual_modules.iter().cloned().collect(),
        role_classes,
        manifests,
        safelist,
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
        )
        .unwrap_err();
        assert!(error.to_string().contains("widgets"));
        assert!(error.to_string().contains("wind.json"));
    }
}
