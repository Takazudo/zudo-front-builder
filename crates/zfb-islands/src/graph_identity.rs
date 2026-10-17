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
    let absolute = |path: &Path| normalize_path_lexical(&cwd.join(path));
    let cwd = absolute(cwd);
    let entry = absolute(entry);
    let virtual_paths: BTreeMap<PathBuf, &str> = virtual_paths
        .iter()
        .map(|(specifier, path)| (absolute(Path::new(path)), specifier.as_str()))
        .collect();
    let source_overrides: BTreeMap<PathBuf, &Vec<u8>> = source_overrides
        .iter()
        .map(|(path, source)| (absolute(path), source))
        .collect();
    let mut identities = BTreeMap::new();
    for key in meta.inputs.keys() {
        let path = normalize_path_lexical(&cwd.join(key));
        let identity = if path == entry {
            "entry:".to_owned()
        } else if let Some(specifier) = virtual_paths.get(&path) {
            format!("virtual:{specifier}")
        } else {
            input_identity(&path, &cwd)?
        };
        identities.insert(key.clone(), identity);
    }
    let entry_key = identities
        .iter()
        .find(|(_, id)| id.as_str() == "entry:")
        .map(|(key, _)| key.as_str())
        .context("esbuild graph identity metafile does not contain its entry")?;
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
                let path = normalize_path_lexical(&cwd.join(key));
                let bytes = match source_overrides.get(&path) {
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
