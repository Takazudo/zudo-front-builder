//! Identity of esbuild's resolved graph, complementary to executable bytes.
//!
//! This reads the resolver's records; it never predicts a resolution winner.
//! Paths are normalized only in metadata, never in authored or emitted code.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use zfb_types::normalize_path_lexical;

#[derive(Deserialize)]
struct Metafile {
    inputs: BTreeMap<String, Input>,
}

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    imports: Vec<Import>,
    format: Option<String>,
}

#[derive(Deserialize)]
struct Import {
    path: String,
    kind: String,
    #[serde(default)]
    external: bool,
    original: Option<String>,
}

/// Hash graph identities alongside (not instead of) the executable closure.
/// `synthetic_entry` is true only for the compiler-owned registration wrapper;
/// real authored entries (including workers) always contribute source bytes.
/// `source_overrides` records exact pre-path-rewrite source provenance for
/// materialized files. Only actual metafile inputs consume these records;
/// unrelated overrides do not affect identity. No source text is normalized.
/// `virtual_paths` contains `(specifier, materialized absolute path)` pairs.
/// Package-less external inputs retain their lexical relation to `cwd`; moving
/// those inputs relative to the project deliberately changes their identity.
pub(crate) fn resolved_graph_digest(
    metafile: &Path,
    cwd: &Path,
    entry: &Path,
    virtual_paths: &[(String, String)],
    synthetic_entry: bool,
    source_overrides: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<[u8; 32]> {
    let meta: Metafile = serde_json::from_slice(&std::fs::read(metafile)?)
        .context("invalid esbuild graph identity metafile")?;
    let logical_cwd = normalize_path_lexical(cwd);
    // esbuild makes metafile paths relative to the process's physical cwd,
    // including when --preserve-symlinks keeps logical input spellings.
    let cwd = std::fs::canonicalize(&logical_cwd).unwrap_or_else(|_| logical_cwd.clone());
    let rebase = |path: PathBuf| match path.strip_prefix(&logical_cwd) {
        Ok(relative) => cwd.join(relative),
        Err(_) => path,
    };
    let absolute = |path: &Path| normalize_path_lexical(&logical_cwd.join(path));
    let entry = absolute(entry);
    let virtual_paths: BTreeMap<PathBuf, &str> = virtual_paths
        .iter()
        .map(|(specifier, path)| (absolute(Path::new(path)), specifier.as_str()))
        .collect();
    let source_overrides: BTreeMap<PathBuf, &Vec<u8>> = source_overrides
        .iter()
        .map(|(path, source)| (absolute(path), source))
        .collect();
    let paths: BTreeMap<&str, PathBuf> = meta
        .inputs
        .keys()
        .map(|key| (key.as_str(), normalize_path_lexical(&cwd.join(key))))
        .collect();
    let entry_key = paths
        .iter()
        .find(|(key, path)| !non_file_input(key) && **path == entry)
        .map(|(key, _)| *key);
    let entry_key = match entry_key {
        Some(key) => key,
        None => {
            let canonical_entry = file_alias(&entry);
            let mut candidates = paths.iter().filter(|(key, path)| {
                !non_file_input(key)
                    && (rebase((*path).clone()) == rebase(entry.clone())
                        || file_alias(path) == canonical_entry)
            });
            let key = candidates
                .next()
                .map(|(key, _)| *key)
                .context("esbuild graph identity metafile does not contain its entry")?;
            if candidates.next().is_some() {
                bail!("esbuild graph identity has ambiguous entry aliases");
            }
            key
        }
    };
    let mut identities = BTreeMap::new();
    for (key, path) in &paths {
        let identity = if *key == entry_key {
            "entry:".to_owned()
        } else if let Some(specifier) = if non_file_input(key) {
            None
        } else {
            lookup_owned(&virtual_paths, path)?
        } {
            format!("virtual:{specifier}")
        } else {
            input_identity(&rebase(path.clone()), &cwd)?
        };
        identities.insert(*key, identity);
    }
    // Canonical traversal distinguishes separate instances of the same package
    // without encoding installation layout. Import order comes from esbuild.
    let mut order = Vec::new();
    let mut visited = BTreeSet::new();
    let mut pending = vec![entry_key];
    while let Some(key) = pending.pop() {
        if !visited.insert(key) {
            continue;
        }
        let input = meta
            .inputs
            .get(key)
            .with_context(|| format!("esbuild graph is missing input {key:?}"))?;
        order.push(key);
        for import in input.imports.iter().rev().filter(|import| !import.external) {
            pending.push(import.path.as_str());
        }
    }
    // No graph input may silently disappear from the identity. Real esbuild
    // keeps tree-shaken inputs connected through its import records.
    if order.len() != meta.inputs.len() {
        bail!("esbuild graph identity contains inputs disconnected from its entry");
    }
    let node_ids: BTreeMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(index, key)| (*key, index))
        .collect();
    let mut records = Vec::new();
    for key in order {
        let input = &meta.inputs[key];
        let mut imports = Vec::new();
        for import in &input.imports {
            let target = if import.external {
                format!("external:{}", import.path)
            } else {
                format!("node:{}", node_ids[import.path.as_str()])
            };
            // Internal originals may contain generated absolute shadow paths.
            // Their resolved node identity and linkage capture the binding.
            let original = if import.external {
                import.original.as_deref()
            } else {
                None
            };
            imports.push((&import.kind, import.external, target, original));
        }
        // Minification is not a semantics-preserving fingerprint for an
        // unminified client: Function.prototype.toString can observe source
        // differences erased by the probe. Preserve raw resolved input bytes.
        // Only the explicitly owned registration wrapper is omitted: it
        // contains staging paths and is covered by the registration compiler,
        // executable probe and canonical graph edges. Worker entries are real
        // authored inputs and must never take that exception.
        let source_digest =
            if (synthetic_entry && key == entry_key) || key.starts_with("(disabled):") {
                None
            } else {
                let path = &paths[key];
                let source_override = if non_file_input(key) {
                    None
                } else {
                    lookup_owned(&source_overrides, path)?
                };
                let bytes = match source_override {
                    Some(source) => (*source).clone(),
                    None => resolved_input_bytes(&cwd, key)?,
                };
                Some(<[u8; 32]>::from(Sha256::digest(bytes)))
            };
        records.push((&identities[key], &input.format, imports, source_digest));
    }
    let mut hash = Sha256::new();
    hash.update(b"zfb-esbuild-resolved-graph-v2\0");
    hash.update(serde_json::to_vec(&records)?);
    Ok(hash.finalize().into())
}

// Alias matching is only for owned file provenance. Graph keys and edges stay
// distinct, including separate package instances under --preserve-symlinks.
fn file_alias(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn lookup_owned<'a, T: PartialEq>(
    records: &'a BTreeMap<PathBuf, T>,
    path: &Path,
) -> Result<Option<&'a T>> {
    if records.is_empty() {
        return Ok(None);
    }
    if let Some(value) = records.get(path) {
        return Ok(Some(value));
    }
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return Ok(None);
    };
    let mut found = None;
    for (candidate, value) in records {
        if std::fs::canonicalize(candidate).ok().as_ref() == Some(&canonical) {
            if found.is_some_and(|previous| previous != value) {
                bail!(
                    "ambiguous esbuild graph source provenance for {}",
                    path.display()
                );
            }
            found = Some(value);
        }
    }
    Ok(found)
}

fn non_file_input(key: &str) -> bool {
    key.starts_with("(disabled):") || (key.starts_with("<data:") && key.ends_with('>'))
}

/// Interpret esbuild's input record, without performing import resolution.
fn resolved_input_bytes(cwd: &Path, key: &str) -> Result<Vec<u8>> {
    // An inline data URL is itself the exact authored source representation;
    // retaining its spelling is conservative and requires no URL decoder.
    if key.starts_with("<data:") && key.ends_with('>') {
        return Ok(key.as_bytes().to_vec());
    }
    let path = cwd.join(key);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && key.ends_with("?url") => {
            // The locked resource contract already recognizes this exact
            // esbuild suffix. A real filename containing ?url wins above.
            let resource = cwd.join(key.strip_suffix("?url").expect("checked suffix"));
            std::fs::read(&resource)
                .with_context(|| format!("read esbuild resolved resource {}", resource.display()))
        }
        Err(error) => Err(error)
            .with_context(|| format!("read esbuild resolved graph input {}", path.display())),
    }
}

fn input_identity(path: &Path, cwd: &Path) -> Result<String> {
    let in_project = path.strip_prefix(cwd).ok();
    let dependency = path
        .components()
        .any(|part| part.as_os_str() == "node_modules");
    if in_project.is_none() || dependency {
        for ancestor in path.parent().into_iter().flat_map(Path::ancestors) {
            let package = ancestor.join("package.json");
            let bytes = match std::fs::read(&package) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).with_context(|| format!("read {}", package.display()))
                }
            };
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse {}", package.display()))?;
            if let Some(name) = value.get("name").and_then(serde_json::Value::as_str) {
                let version = value
                    .get("version")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                return Ok(format!(
                    "package:{name}@{version}/{}",
                    slash(path.strip_prefix(ancestor)?)
                ));
            }
        }
    }
    Ok(format!("project:{}", relative(path, cwd)))
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn relative(path: &Path, base: &Path) -> String {
    let path_parts: Vec<Component<'_>> = path.components().collect();
    let base_parts: Vec<Component<'_>> = base.components().collect();
    let shared = path_parts
        .iter()
        .zip(&base_parts)
        .take_while(|(a, b)| a == b)
        .count();
    if shared == 0 {
        return slash(path);
    }
    let mut result = PathBuf::new();
    for _ in shared..base_parts.len() {
        result.push("..");
    }
    for component in &path_parts[shared..] {
        result.push(component.as_os_str());
    }
    slash(&result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn digest(root: &Path, external: &str) -> [u8; 32] {
        std::fs::create_dir_all(root).unwrap();
        let entry = root.join("random-entry.tsx");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/widget.ts"), "export const value = 1;").unwrap();
        let metadata = json!({"inputs": {
            "random-entry.tsx": {"imports": [
                {"path":"src/widget.ts", "kind":"import-statement", "original":root.join("src/widget.ts")}
            ]},
            "src/widget.ts": {"format":"esm", "imports": [
                {"path":external,"kind":"dynamic-import","external":true}
            ]}
        }});
        let metafile = root.join("meta.json");
        std::fs::write(&metafile, serde_json::to_vec(&metadata).unwrap()).unwrap();
        resolved_graph_digest(&metafile, root, &entry, &[], true, &BTreeMap::new()).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn physical_metafile_base_and_owned_path_aliases_agree() {
        let temp = tempfile::tempdir().unwrap();
        let canonical_temp = temp.path().canonicalize().unwrap();
        let physical = canonical_temp.join("private/var");
        std::fs::create_dir_all(&physical).unwrap();
        let logical = canonical_temp.join("var");
        std::os::unix::fs::symlink(&physical, &logical).unwrap();
        let entry = logical.join("entry.js");
        let source = logical.join("virtual.js");
        std::fs::write(&entry, "compiler wrapper").unwrap();
        std::fs::write(&source, "rewritten source").unwrap();
        let metafile = physical.join("meta.json");
        let overrides = BTreeMap::from([(source.clone(), b"authored source".to_vec())]);
        let virtuals = [("virtual:demo".into(), source.to_string_lossy().into_owned())];
        let mut digests = Vec::new();
        // Captured esbuild 0.25.12 spellings: physical cwd, with and without
        // --preserve-symlinks, for var -> private/var at a different depth.
        for (entry_key, source_key) in [
            ("entry.js", "virtual.js"),
            ("../../var/entry.js", "../../var/virtual.js"),
        ] {
            let meta = json!({"inputs": {
                (entry_key): {"imports": [{"path":source_key,"kind":"import-statement"}]},
                (source_key): {"format":"esm"}
            }});
            std::fs::write(&metafile, serde_json::to_vec(&meta).unwrap()).unwrap();
            for cwd in [&physical, &logical] {
                digests.push(
                    resolved_graph_digest(&metafile, cwd, &entry, &virtuals, true, &overrides)
                        .unwrap(),
                );
            }
        }
        assert!(digests.iter().all(|digest| *digest == digests[0]));
    }

    #[cfg(unix)]
    #[test]
    fn exact_owned_paths_win_and_ambiguous_alias_provenance_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let canonical_temp = temp.path().canonicalize().unwrap();
        let root = canonical_temp.as_path();
        let real = root.join("real.js");
        let alias = root.join("alias.js");
        let other = root.join("other.js");
        std::fs::write(&real, "materialized").unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        std::os::unix::fs::symlink(&real, &other).unwrap();
        let mut records = BTreeMap::from([(real.clone(), b"authored".to_vec())]);
        assert_eq!(lookup_owned(&records, &alias).unwrap(), records.get(&real));
        records.insert(alias.clone(), b"different".to_vec());
        assert_eq!(lookup_owned(&records, &real).unwrap(), records.get(&real));
        assert_eq!(lookup_owned(&records, &alias).unwrap(), records.get(&alias));
        assert!(lookup_owned(&records, &other)
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
        let metafile = root.join("meta.json");
        std::fs::write(&metafile, br#"{"inputs":{"entry.js":{"imports":[{"path":"real.js","kind":"import-statement"}]},"real.js":{}}}"#).unwrap();
        let digest = |records: &BTreeMap<PathBuf, Vec<u8>>| {
            resolved_graph_digest(&metafile, root, &root.join("entry.js"), &[], true, records)
                .unwrap()
        };
        let before = digest(&records);
        records.insert(alias, b"unreachable change".to_vec());
        assert_eq!(before, digest(&records));
        records.insert(real, b"authored change".to_vec());
        assert_ne!(before, digest(&records));
    }

    #[cfg(unix)]
    #[test]
    fn exact_entry_alias_keeps_distinct_graph_nodes_and_synthetic_entries_need_no_file() {
        let temp = tempfile::tempdir().unwrap();
        let canonical_temp = temp.path().canonicalize().unwrap();
        let physical = canonical_temp.join("private/var");
        std::fs::create_dir_all(&physical).unwrap();
        let logical = canonical_temp.join("var");
        std::os::unix::fs::symlink(&physical, &logical).unwrap();
        let metafile = physical.join("meta.json");
        let entry = physical.join("entry.js");
        std::fs::write(&entry, "authored self import").unwrap();
        // Real --preserve-symlinks graph: the physical entry imports its
        // logical alias, which in turn imports itself as a separate node.
        let meta = json!({"inputs": {
            "entry.js":{"imports":[{"path":"../../var/entry.js","kind":"import-statement"}]},
            "../../var/entry.js":{"imports":[{"path":"../../var/entry.js","kind":"import-statement"}]}
        }});
        std::fs::write(&metafile, serde_json::to_vec(&meta).unwrap()).unwrap();
        resolved_graph_digest(&metafile, &logical, &entry, &[], true, &BTreeMap::new()).unwrap();
        let third_alias = canonical_temp.join("third.js");
        std::os::unix::fs::symlink(&entry, &third_alias).unwrap();
        assert!(resolved_graph_digest(
            &metafile,
            &logical,
            &third_alias,
            &[],
            true,
            &BTreeMap::new()
        )
        .unwrap_err()
        .to_string()
        .contains("ambiguous entry"));
        std::fs::remove_file(&entry).unwrap();
        std::fs::write(&metafile, br#"{"inputs":{"entry.js":{}}}"#).unwrap();
        resolved_graph_digest(
            &metafile,
            &logical,
            &logical.join("entry.js"),
            &[],
            true,
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(resolved_graph_digest(
            &metafile,
            &logical,
            &logical.join("entry.js"),
            &[],
            false,
            &BTreeMap::new()
        )
        .is_err());
        assert!(resolved_graph_digest(
            &metafile,
            &logical,
            &logical.join("missing.js"),
            &[],
            true,
            &BTreeMap::new()
        )
        .unwrap_err()
        .to_string()
        .contains("does not contain its entry"));
    }

    #[test]
    fn namespaces_resource_suffix_and_missing_provenance_remain_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let canonical_temp = temp.path().canonicalize().unwrap();
        let cwd = canonical_temp.as_path();
        let meta = cwd.join("meta.json");
        let data = "<data:text/javascript,export default 1>";
        let metadata = json!({"inputs": {
            "entry.js":{"imports":[
                {"path":data,"kind":"import-statement"},
                {"path":"(disabled):fs","kind":"import-statement"},
                {"path":"asset.bin?url","kind":"import-statement"}
            ]},
            (data):{}, "(disabled):fs":{}, "asset.bin?url":{}
        }});
        std::fs::write(&meta, serde_json::to_vec(&metadata).unwrap()).unwrap();
        let digest = || {
            resolved_graph_digest(
                &meta,
                cwd,
                &cwd.join("entry.js"),
                &[],
                true,
                &BTreeMap::new(),
            )
        };
        assert!(
            digest().is_err(),
            "missing input provenance must not be omitted"
        );
        std::fs::write(cwd.join("asset.bin"), "resource").unwrap();
        let _fallback = digest().unwrap();
        #[cfg(unix)]
        {
            std::fs::write(cwd.join("asset.bin?url"), "literal filename").unwrap();
            let literal = digest().unwrap();
            assert_ne!(_fallback, literal);
            std::fs::write(cwd.join("asset.bin"), "irrelevant while literal exists").unwrap();
            assert_eq!(literal, digest().unwrap());
        }
        assert_eq!(resolved_input_bytes(cwd, data).unwrap(), data.as_bytes());
        std::fs::write(&meta, br#"{"inputs":{"entry.js":{},"orphan.js":{}}}"#).unwrap();
        assert!(digest().unwrap_err().to_string().contains("disconnected"));
        std::fs::write(&meta, br#"{"inputs":{"entry.js":{"imports":[{"path":"absent.js","kind":"import-statement"}]}}}"#).unwrap();
        assert!(digest().unwrap_err().to_string().contains("missing input"));
    }

    #[cfg(unix)]
    #[test]
    fn linked_instances_and_package_less_external_locations_remain_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let canonical_temp = temp.path().canonicalize().unwrap();
        let cwd = canonical_temp.join("project");
        let package = canonical_temp.join("package");
        std::fs::create_dir_all(cwd.join("node_modules")).unwrap();
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"demo","version":"1"}"#,
        )
        .unwrap();
        std::fs::write(package.join("index.js"), "export const value = 1").unwrap();
        for name in ["one", "two"] {
            std::os::unix::fs::symlink(&package, cwd.join("node_modules").join(name)).unwrap();
        }
        let metafile = cwd.join("meta.json");
        let graph = |targets: &[&str]| {
            let mut inputs = serde_json::Map::new();
            inputs.insert("entry.js".into(), json!({"imports":targets.iter().map(|path| json!({"path":path,"kind":"import-statement"})).collect::<Vec<_>>()}));
            for path in targets {
                inputs.insert((*path).into(), json!({}));
            }
            std::fs::write(
                &metafile,
                serde_json::to_vec(&json!({"inputs":inputs})).unwrap(),
            )
            .unwrap();
            resolved_graph_digest(
                &metafile,
                &cwd,
                &cwd.join("entry.js"),
                &[],
                true,
                &BTreeMap::new(),
            )
            .unwrap()
        };
        let one = "node_modules/one/index.js";
        let two = "node_modules/two/index.js";
        assert_ne!(graph(&[one, one]), graph(&[one, two]));
        for name in ["outside-a.js", "outside-b.js"] {
            std::fs::write(canonical_temp.join(name), "export const value = 1").unwrap();
        }
        assert_ne!(graph(&["../outside-a.js"]), graph(&["../outside-b.js"]));
    }

    #[test]
    fn graph_identity_ignores_cwd_depth_but_preserves_external_imports() {
        let temp = tempfile::tempdir().unwrap();
        let first = digest(&temp.path().join("a"), "https://example.test/one.js");
        assert_eq!(
            first,
            digest(
                &temp.path().join("deep/nested/b"),
                "https://example.test/one.js"
            )
        );
        assert_ne!(
            first,
            digest(&temp.path().join("a"), "https://example.test/two.js")
        );
    }

    #[test]
    fn package_identity_matches_linked_and_installed_locations() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("app");
        let installed = cwd.join("node_modules/demo");
        let linked = temp.path().join("packages/demo");
        for package in [&installed, &linked] {
            std::fs::create_dir_all(package).unwrap();
            std::fs::write(package.join("package.json"), r#"{"name":"demo"}"#).unwrap();
        }
        assert_eq!(
            input_identity(&installed.join("src/index.js"), &cwd).unwrap(),
            input_identity(&linked.join("src/index.js"), &cwd).unwrap()
        );
    }

    #[test]
    fn selected_dynamic_module_identity_changes_even_with_identical_code() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let mut digests = Vec::new();
        for name in ["a.js", "b.js"] {
            std::fs::write(cwd.join(name), "export const url = import.meta.url;").unwrap();
            let meta = json!({"inputs": {
                "entry.tsx":{"imports":[{"path":name,"kind":"dynamic-import"}]},
                (name):{"format":"esm","imports":[]}
            }});
            let file = cwd.join("meta.json");
            std::fs::write(&file, serde_json::to_vec(&meta).unwrap()).unwrap();
            digests.push(
                resolved_graph_digest(
                    &file,
                    cwd,
                    &cwd.join("entry.tsx"),
                    &[],
                    true,
                    &BTreeMap::new(),
                )
                .unwrap(),
            );
        }
        assert_ne!(digests[0], digests[1]);
    }

    #[test]
    fn duplicate_package_instances_preserve_graph_topology() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let mut digests = Vec::new();
        for layout in ["one", "deep/two"] {
            let first = format!("{layout}/node_modules/demo/index.js");
            let second = format!("{layout}/nested/node_modules/demo/index.js");
            for path in [&first, &second] {
                let package = cwd.join(path).parent().unwrap().to_path_buf();
                std::fs::create_dir_all(&package).unwrap();
                std::fs::write(cwd.join(path), "export const value = 1;").unwrap();
                std::fs::write(
                    package.join("package.json"),
                    r#"{"name":"demo","version":"1.0.0"}"#,
                )
                .unwrap();
            }
            for same_instance in [false, true] {
                let imports = if same_instance {
                    vec![&first, &first]
                } else {
                    vec![&first, &second]
                };
                let mut inputs = serde_json::Map::new();
                inputs.insert("entry.tsx".into(), json!({"imports":imports.iter().map(|path| json!({"path":path,"kind":"import-statement"})).collect::<Vec<_>>()}));
                inputs.insert(first.clone(), json!({"format":"esm"}));
                if !same_instance {
                    inputs.insert(second.clone(), json!({"format":"esm"}));
                }
                let file = cwd.join("meta.json");
                std::fs::write(
                    &file,
                    serde_json::to_vec(&json!({"inputs":inputs})).unwrap(),
                )
                .unwrap();
                digests.push(
                    resolved_graph_digest(
                        &file,
                        cwd,
                        &cwd.join("entry.tsx"),
                        &[],
                        true,
                        &BTreeMap::new(),
                    )
                    .unwrap(),
                );
            }
        }
        assert_ne!(digests[0], digests[1]);
        assert_eq!(digests[0], digests[2]);
        assert_eq!(digests[1], digests[3]);
    }

    #[test]
    fn exact_source_provenance_ignores_materialized_rewrites_but_tracks_original_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let entry = cwd.join("entry.tsx");
        let source = cwd.join("rewritten.js");
        let metafile = cwd.join("meta.json");
        std::fs::write(&metafile, br#"{"inputs":{"entry.tsx":{"imports":[{"path":"rewritten.js","kind":"import-statement"}]},"rewritten.js":{"imports":[]}}}"#).unwrap();
        let mut originals =
            BTreeMap::from([(source.clone(), b"export const value = 1 + 2;".to_vec())]);
        std::fs::write(&source, "export const value = '/tmp/shadow-one';").unwrap();
        let before = resolved_graph_digest(&metafile, cwd, &entry, &[], true, &originals).unwrap();
        std::fs::write(&source, "export const value = '/tmp/shadow-two';").unwrap();
        originals.insert(cwd.join("unreachable.js"), b"irrelevant".to_vec());
        assert_eq!(
            before,
            resolved_graph_digest(&metafile, cwd, &entry, &[], true, &originals).unwrap()
        );
        originals.insert(source, b"export const value = 3;".to_vec());
        assert_ne!(
            before,
            resolved_graph_digest(&metafile, cwd, &entry, &[], true, &originals).unwrap()
        );
    }

    #[test]
    fn actual_esbuild_data_url_namespace_retains_inline_source_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let entry = cwd.join("entry.tsx");
        let metafile = cwd.join("meta.json");
        let mut digests = Vec::new();
        for key in [
            "<data:text/javascript,export default 1>",
            "<data:text/javascript,export default 2>",
        ] {
            let metadata = json!({"inputs": {
                "entry.tsx":{"imports":[{"path":key,"kind":"import-statement"}]},
                (key):{"format":"esm","imports":[]}
            }});
            std::fs::write(&metafile, serde_json::to_vec(&metadata).unwrap()).unwrap();
            digests.push(
                resolved_graph_digest(&metafile, cwd, &entry, &[], true, &BTreeMap::new()).unwrap(),
            );
        }
        assert_ne!(digests[0], digests[1]);
    }

    #[test]
    fn real_entry_bytes_are_hashed_even_with_synthetic_looking_filename() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let entry = cwd.join(".zfb-esbuild-entry-authored.tsx");
        let metafile = cwd.join("meta.json");
        std::fs::write(
            &metafile,
            br#"{"inputs":{".zfb-esbuild-entry-authored.tsx":{"imports":[]}}}"#,
        )
        .unwrap();
        std::fs::write(
            &entry,
            "function worker(){return 1 + 2} postMessage(worker.toString());",
        )
        .unwrap();
        let before =
            resolved_graph_digest(&metafile, cwd, &entry, &[], false, &BTreeMap::new()).unwrap();
        std::fs::write(
            &entry,
            "function worker(){return 3} postMessage(worker.toString());",
        )
        .unwrap();
        let after =
            resolved_graph_digest(&metafile, cwd, &entry, &[], false, &BTreeMap::new()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn virtual_identity_does_not_include_random_materialized_name() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let mut digests = Vec::new();
        for name in [".zfb-virtual-AAA.mjs", ".zfb-virtual-BBB.mjs"] {
            std::fs::write(cwd.join(name), "export default 1;").unwrap();
            let meta = json!({"inputs": {
                "entry.tsx":{"imports":[{"path":name,"kind":"import-statement"}]},
                (name):{"format":"esm","imports":[]}
            }});
            let file = cwd.join("meta.json");
            std::fs::write(&file, serde_json::to_vec(&meta).unwrap()).unwrap();
            digests.push(
                resolved_graph_digest(
                    &file,
                    cwd,
                    &cwd.join("entry.tsx"),
                    &[(
                        "virtual:demo".into(),
                        cwd.join(name).to_string_lossy().into_owned(),
                    )],
                    true,
                    &BTreeMap::new(),
                )
                .unwrap(),
            );
        }
        assert_eq!(digests[0], digests[1]);
    }
}
