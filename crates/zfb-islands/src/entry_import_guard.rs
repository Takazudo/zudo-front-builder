//! Metafile audit: no emitted output may import the islands entry (#4464).
//!
//! Production renames the entry (`islands.js` -> `islands-<hash>.js`) and
//! writes chunks verbatim, so a chunk importing `./islands.js` names a file
//! that is never published (#4444). Chunk placement is the bundler's decision,
//! so this reads the bundler's own `meta.json` after the splitting pass instead
//! of predicting it, on both backends:
//!
//! - esbuild spells output keys and import paths relative to its working dir;
//! - Rolldown spells both as absolute `outdir.join(filename)` and writes
//!   `entryPoint: null` on every chunk without a facade module.
//!
//! The islands entry is the one output whose non-null `entryPoint` is the
//! synthesized islands entry input. Every other output, dynamic-entry facades
//! included, is inspected. Malformed metadata fails closed.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use zfb_types::normalize_path_lexical;

use crate::esbuild::metafile_output_path;

/// Import kinds that load a module at runtime and would 404 on the renamed entry.
const LOADING_IMPORT_KINDS: [&str; 2] = ["import-statement", "dynamic-import"];

/// Audit the metafile at `metafile_path`, written by a splitting pass run in
/// `cwd` on the synthesized islands entry `entry_input`. Returns the number of
/// loading edges not marked external that were inspected on non-entry outputs.
pub(crate) fn audit_entry_imports_at_path(
    metafile_path: &Path,
    cwd: &Path,
    entry_input: &Path,
) -> Result<usize> {
    let bytes = std::fs::read(metafile_path).with_context(|| {
        format!(
            "islands entry-import audit requires metafile {}",
            metafile_path.display()
        )
    })?;
    let metafile: Value = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "islands entry-import audit could not parse metafile {}",
            metafile_path.display()
        )
    })?;
    audit_entry_imports(&metafile, cwd, entry_input)
        .with_context(|| format!("metafile {}", metafile_path.display()))
}

fn audit_entry_imports(metafile: &Value, cwd: &Path, entry_input: &Path) -> Result<usize> {
    let outputs = metafile
        .get("outputs")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("malformed metafile: missing `outputs` object"))?;

    let bases = path_bases(cwd);
    let entry_input_forms = path_forms(&entry_input.to_string_lossy(), &bases);
    let mut entries = Vec::new();
    for (key, output) in outputs {
        let output = output
            .as_object()
            .ok_or_else(|| anyhow!("malformed metafile: output {key:?} is not an object"))?;
        match output.get("entryPoint") {
            None | Some(Value::Null) => {}
            // esbuild tags a JS entry's CSS sidecar with the same entryPoint;
            // the read-back rejects that output with its own #1395 message.
            Some(Value::String(_)) if key.ends_with(".css") => {}
            Some(Value::String(entry_point)) => {
                if intersects(&path_forms(entry_point, &bases), &entry_input_forms) {
                    entries.push(key.as_str());
                }
            }
            Some(other) => {
                bail!("malformed metafile: output {key:?} has non-string entryPoint {other}")
            }
        }
    }
    let entry_key = match entries.as_slice() {
        [entry] => *entry,
        [] => bail!(
            "malformed metafile: no output has the synthesized islands entry {} as its entryPoint",
            entry_input.display()
        ),
        many => bail!(
            "malformed metafile: {} outputs claim the synthesized islands entry {} as their entryPoint: {many:?}",
            many.len(),
            entry_input.display()
        ),
    };
    let entry_forms = path_forms(entry_key, &bases);

    let mut inspected = 0;
    let mut violations = Vec::new();
    for (key, output) in outputs {
        if key == entry_key {
            continue;
        }
        let imports = match output.get("imports") {
            Some(Value::Array(imports)) => imports.as_slice(),
            Some(other) => {
                bail!("malformed metafile: output {key:?} has non-array imports {other}")
            }
            // A chunk always lists its imports; only a copied asset (a Rolldown
            // file-loader resource such as `.zfb-resource.mjs`, or a sourcemap)
            // has neither field, so the extension cannot tell them apart.
            None if output.get("entryPoint").is_some() => {
                bail!("malformed metafile: chunk output {key:?} has no imports array")
            }
            None => continue,
        };
        for import in imports {
            let path = import.get("path").and_then(Value::as_str).ok_or_else(|| {
                anyhow!("malformed metafile: output {key:?} has an import without a string path")
            })?;
            let kind = import.get("kind").and_then(Value::as_str).ok_or_else(|| {
                anyhow!("malformed metafile: output {key:?} import {path:?} has no string kind")
            })?;
            let external = match import.get("external") {
                None => false,
                Some(Value::Bool(external)) => *external,
                Some(other) => bail!(
                    "malformed metafile: output {key:?} import {path:?} has non-boolean external {other}"
                ),
            };
            if external || !LOADING_IMPORT_KINDS.contains(&kind) {
                continue;
            }
            inspected += 1;
            if intersects(&path_forms(path, &bases), &entry_forms) {
                violations.push(format!("`{key}` imports `{entry_key}` ({kind})"));
            }
        }
    }
    if !violations.is_empty() {
        bail!(
            "zfb-islands: {}; no emitted chunk may import the islands entry, because production \
             renames the entry (`islands.js` -> `islands-<hash>.js`) and writes chunks verbatim (#4444)",
            violations.join("; ")
        );
    }
    Ok(inspected)
}

/// `cwd` plus the physical cwd, which Rolldown may relativize against.
fn path_bases(cwd: &Path) -> Vec<PathBuf> {
    let mut bases = vec![cwd.to_path_buf()];
    if let Ok(physical) = std::fs::canonicalize(cwd) {
        bases.push(physical);
    }
    bases
}

/// Every spelling a metadata path can take: resolved against each of `bases`
/// then normalized lexically, plus the canonical path when it exists, so a
/// symlinked prefix such as macOS `/var` -> `/private/var` still compares equal.
fn path_forms(raw: &str, bases: &[PathBuf]) -> Vec<PathBuf> {
    let mut forms = Vec::new();
    for base in bases {
        let path = Path::new(raw);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        };
        forms.push(normalize_path_lexical(&resolved));
        if let Ok(canonical) = metafile_output_path(raw, base) {
            forms.push(canonical);
        }
    }
    forms.sort();
    forms.dedup();
    forms
}

fn intersects(a: &[PathBuf], b: &[PathBuf]) -> bool {
    a.iter().any(|path| b.contains(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type MetafileEdit<'a> = Box<dyn Fn(&mut Value) + 'a>;

    /// A staging layout like the real splitting pass: `cwd` holds the
    /// synthesized entry input and an `out/` dir with the emitted files.
    struct Layout {
        _tmp: tempfile::TempDir,
        cwd: PathBuf,
        entry_input: PathBuf,
    }

    const OUTPUTS: [&str; 4] = [
        "islands.js",
        "islands-chunk-AAA.js",
        "islands-chunk-BBB.js",
        "islands.js.map",
    ];

    fn layout() -> Layout {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(cwd.join("out")).unwrap();
        let entry_input = cwd.join(".zfb-esbuild-entry-x.tsx");
        std::fs::write(&entry_input, "").unwrap();
        std::fs::write(cwd.join("lazy.tsx"), "").unwrap();
        for name in OUTPUTS {
            std::fs::write(cwd.join("out").join(name), "").unwrap();
        }
        Layout {
            _tmp: tmp,
            cwd,
            entry_input,
        }
    }

    #[derive(Clone, Copy)]
    enum Spelling {
        /// esbuild: output keys and imports relative to cwd.
        Esbuild,
        /// Rolldown: absolute `outdir.join(filename)`.
        Rolldown,
    }

    const SPELLINGS: [Spelling; 2] = [Spelling::Esbuild, Spelling::Rolldown];

    impl Layout {
        fn out(&self, spelling: Spelling, name: &str) -> String {
            match spelling {
                Spelling::Esbuild => format!("out/{name}"),
                Spelling::Rolldown => self.cwd.join("out").join(name).display().to_string(),
            }
        }

        fn imp(&self, spelling: Spelling, name: &str, kind: &str) -> Value {
            json!({"path": self.out(spelling, name), "kind": kind})
        }

        /// A normal split graph: entry -> chunk A (static), entry -> chunk B
        /// (dynamic, a Rolldown facade with its own entryPoint), B -> A.
        fn metafile(&self, spelling: Spelling, b_imports: Vec<Value>) -> Value {
            let null_or_absent = |output: Value| match spelling {
                Spelling::Rolldown => {
                    let mut output = output;
                    output["entryPoint"] = Value::Null;
                    output
                }
                Spelling::Esbuild => output,
            };
            let mut outputs = serde_json::Map::new();
            outputs.insert(
                self.out(spelling, "islands.js"),
                json!({
                    "imports": [
                        self.imp(spelling, "islands-chunk-AAA.js", "import-statement"),
                        self.imp(spelling, "islands-chunk-BBB.js", "dynamic-import"),
                    ],
                    "entryPoint": ".zfb-esbuild-entry-x.tsx",
                }),
            );
            outputs.insert(
                self.out(spelling, "islands-chunk-AAA.js"),
                null_or_absent(json!({"imports": []})),
            );
            outputs.insert(
                self.out(spelling, "islands-chunk-BBB.js"),
                json!({"imports": b_imports, "entryPoint": "lazy.tsx"}),
            );
            outputs.insert(
                self.out(spelling, "islands.js.map"),
                match spelling {
                    Spelling::Esbuild => json!({"imports": []}),
                    Spelling::Rolldown => json!({"inputs": {}}),
                },
            );
            json!({"outputs": outputs})
        }

        fn audit(&self, metafile: &Value) -> Result<usize> {
            audit_entry_imports(metafile, &self.cwd, &self.entry_input)
        }
    }

    fn assert_rejected(result: Result<usize>, offender: &str, kind: &str) {
        let message = format!("{:#}", result.expect_err("chunk -> entry must be rejected"));
        for needle in [offender, "islands.js`", kind, "#4444"] {
            assert!(
                message.contains(needle),
                "{needle:?} missing from: {message}"
            );
        }
    }

    #[test]
    fn accepts_entry_to_chunk_and_chunk_to_chunk() {
        let layout = layout();
        for spelling in SPELLINGS {
            let b_imports = vec![layout.imp(spelling, "islands-chunk-AAA.js", "import-statement")];
            let metafile = layout.metafile(spelling, b_imports);
            assert_eq!(layout.audit(&metafile).unwrap(), 1);
        }
    }

    #[test]
    fn rejects_chunk_importing_the_entry_statically() {
        let layout = layout();
        for spelling in SPELLINGS {
            let mut metafile = layout.metafile(spelling, vec![]);
            metafile["outputs"][layout.out(spelling, "islands-chunk-AAA.js")]["imports"] =
                json!([layout.imp(spelling, "islands.js", "import-statement")]);
            assert_rejected(
                layout.audit(&metafile),
                "islands-chunk-AAA.js",
                "import-statement",
            );
        }
    }

    #[test]
    fn rejects_chunk_importing_the_entry_dynamically() {
        let layout = layout();
        for spelling in SPELLINGS {
            let mut metafile = layout.metafile(spelling, vec![]);
            metafile["outputs"][layout.out(spelling, "islands-chunk-AAA.js")]["imports"] =
                json!([layout.imp(spelling, "islands.js", "dynamic-import")]);
            assert_rejected(
                layout.audit(&metafile),
                "islands-chunk-AAA.js",
                "dynamic-import",
            );
        }
    }

    #[test]
    fn rejects_dynamic_entry_facade_importing_the_entry() {
        let layout = layout();
        for spelling in SPELLINGS {
            let b_imports = vec![layout.imp(spelling, "islands.js", "import-statement")];
            let metafile = layout.metafile(spelling, b_imports);
            assert_rejected(
                layout.audit(&metafile),
                "islands-chunk-BBB.js",
                "import-statement",
            );
        }
    }

    #[test]
    fn null_entry_point_chunks_are_not_entries() {
        let layout = layout();
        let mut metafile = layout.metafile(Spelling::Rolldown, vec![]);
        let chunk = layout.out(Spelling::Rolldown, "islands-chunk-AAA.js");
        assert!(metafile["outputs"][&chunk]["entryPoint"].is_null());
        // Were a null entryPoint taken for the entry, this edge would read as
        // entry -> entry and pass; it must be caught as chunk -> entry.
        metafile["outputs"][&chunk]["imports"] =
            json!([layout.imp(Spelling::Rolldown, "islands.js", "import-statement")]);
        assert_rejected(
            layout.audit(&metafile),
            "islands-chunk-AAA.js",
            "import-statement",
        );
    }

    #[test]
    fn absolute_entry_point_and_mixed_spelling_import_match() {
        let layout = layout();
        let mut metafile = layout.metafile(Spelling::Rolldown, vec![]);
        let entry = layout.out(Spelling::Rolldown, "islands.js");
        metafile["outputs"][&entry]["entryPoint"] = json!(layout.entry_input.display().to_string());
        // An esbuild-spelled import into a Rolldown-spelled key still matches.
        metafile["outputs"][layout.out(Spelling::Rolldown, "islands-chunk-AAA.js")]["imports"] =
            json!([layout.imp(Spelling::Esbuild, "./islands.js", "import-statement")]);
        assert_rejected(
            layout.audit(&metafile),
            "islands-chunk-AAA.js",
            "import-statement",
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_prefix_alias_matches() {
        // Mirrors macOS `/var` -> `/private/var`: metadata spelled under the
        // physical directory while the configured cwd is the symlink.
        let layout = layout();
        let alias = layout.cwd.parent().unwrap().join("alias");
        std::os::unix::fs::symlink(&layout.cwd, &alias).unwrap();
        let physical = std::fs::canonicalize(&layout.cwd).unwrap();
        let out = |name: &str| physical.join("out").join(name).display().to_string();
        let metafile = json!({"outputs": {
            out("islands.js"): {
                "imports": [{"path": out("islands-chunk-AAA.js"), "kind": "import-statement"}],
                "entryPoint": physical.join(".zfb-esbuild-entry-x.tsx").display().to_string(),
            },
            out("islands-chunk-AAA.js"): {
                "imports": [{"path": out("islands.js"), "kind": "dynamic-import"}],
                "entryPoint": null,
            },
        }});
        let alias_entry = alias.join(".zfb-esbuild-entry-x.tsx");
        assert_rejected(
            audit_entry_imports(&metafile, &alias, &alias_entry),
            "islands-chunk-AAA.js",
            "dynamic-import",
        );
        // The reverse: alias-spelled metadata, physical cwd.
        let out = |name: &str| alias.join("out").join(name).display().to_string();
        let metafile = json!({"outputs": {
            out("islands.js"): {"imports": [], "entryPoint": ".zfb-esbuild-entry-x.tsx"},
            out("islands-chunk-AAA.js"): {
                "imports": [{"path": out("islands.js"), "kind": "import-statement"}],
            },
        }});
        assert_rejected(
            audit_entry_imports(&metafile, &physical, &layout.entry_input),
            "islands-chunk-AAA.js",
            "import-statement",
        );
    }

    #[test]
    fn css_sidecar_sharing_the_entry_point_is_not_a_second_entry() {
        let layout = layout();
        let mut metafile = layout.metafile(Spelling::Esbuild, vec![]);
        metafile["outputs"][layout.out(Spelling::Esbuild, "islands.css")] =
            json!({"imports": [], "entryPoint": ".zfb-esbuild-entry-x.tsx"});
        assert_eq!(layout.audit(&metafile).unwrap(), 0);
    }

    #[test]
    fn external_imports_are_ignored() {
        let layout = layout();
        for spelling in SPELLINGS {
            let mut external = layout.imp(spelling, "islands.js", "import-statement");
            external["external"] = json!(true);
            let metafile = layout.metafile(spelling, vec![external]);
            assert_eq!(layout.audit(&metafile).unwrap(), 0);
        }
    }

    #[test]
    fn malformed_metadata_is_an_error() {
        let layout = layout();
        let spelling = Spelling::Esbuild;
        let chunk = layout.out(spelling, "islands-chunk-AAA.js");
        let entry = layout.out(spelling, "islands.js");
        let cases: Vec<(&str, MetafileEdit<'_>)> = vec![
            (
                "missing `outputs`",
                Box::new(|m| *m = json!({"inputs": {}})),
            ),
            (
                "not an object",
                Box::new(|m| m["outputs"][&chunk] = json!([])),
            ),
            (
                "non-array imports",
                Box::new(|m| m["outputs"][&chunk]["imports"] = json!({})),
            ),
            (
                "no imports array",
                Box::new(|m| {
                    m["outputs"][&chunk] = json!({"entryPoint": null});
                }),
            ),
            (
                "without a string path",
                Box::new(|m| m["outputs"][&chunk]["imports"] = json!([{"kind": "dynamic-import"}])),
            ),
            (
                "no string kind",
                Box::new(|m| m["outputs"][&chunk]["imports"] = json!([{"path": "out/x.js"}])),
            ),
            (
                "non-boolean external",
                Box::new(|m| {
                    m["outputs"][&chunk]["imports"] =
                        json!([{"path": "out/x.js", "kind": "import-statement", "external": "yes"}])
                }),
            ),
            (
                "non-string entryPoint",
                Box::new(|m| m["outputs"][&chunk]["entryPoint"] = json!(7)),
            ),
            (
                "no output has the synthesized islands entry",
                Box::new(|m| m["outputs"][&entry]["entryPoint"] = Value::Null),
            ),
            (
                "2 outputs claim the synthesized islands entry",
                Box::new(|m| {
                    m["outputs"][&chunk]["entryPoint"] = json!(".zfb-esbuild-entry-x.tsx")
                }),
            ),
        ];
        for (needle, mutate) in cases {
            let mut metafile = layout.metafile(spelling, vec![]);
            mutate(&mut metafile);
            let message = format!("{:#}", layout.audit(&metafile).unwrap_err());
            assert!(
                message.contains(needle),
                "{needle:?} missing from: {message}"
            );
        }
    }

    #[test]
    fn unparsable_metafile_is_an_error() {
        let layout = layout();
        let path = layout.cwd.join("out/meta.json");
        std::fs::write(&path, "{").unwrap();
        let err = audit_entry_imports_at_path(&path, &layout.cwd, &layout.entry_input);
        assert!(format!("{:#}", err.unwrap_err()).contains("could not parse"));
        std::fs::remove_file(&path).unwrap();
        let err = audit_entry_imports_at_path(&path, &layout.cwd, &layout.entry_input);
        assert!(format!("{:#}", err.unwrap_err()).contains("requires metafile"));
    }

    /// Real Rolldown splitting bundles, run exactly as the shared islands pass
    /// asks, pass the audit with their real metafile spelling; the bundle is
    /// the oracle for chunk placement.
    #[cfg(feature = "rolldown-prototype")]
    #[test]
    fn real_rolldown_splitting_bundle_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let entry = cwd.join(".zfb-esbuild-entry-x.tsx");
        std::fs::write(
            &entry,
            "import { shared } from \"./shared.js\";\n\
             shared(\"eager\");\n\
             globalThis.a = () => import(\"./lazy-a.js\");\n\
             globalThis.b = () => import(\"./lazy-b.js\");\n",
        )
        .unwrap();
        std::fs::write(
            cwd.join("shared.js"),
            "export function shared(x) { return [x, globalThis.marker]; }\n",
        )
        .unwrap();
        for name in ["lazy-a", "lazy-b"] {
            std::fs::write(
                cwd.join(format!("{name}.js")),
                format!(
                    "import {{ shared }} from \"./shared.js\";\n\
                     import {{ common }} from \"./common.js\";\n\
                     export const v = shared(\"{name}\") + common;\n"
                ),
            )
            .unwrap();
        }
        std::fs::write(cwd.join("common.js"), "export const common = 42;\n").unwrap();
        let mut cmd = std::process::Command::new("unused-prepared-esbuild");
        cmd.current_dir(cwd).arg(&entry).args([
            "--bundle",
            "--format=esm",
            "--platform=browser",
            "--splitting=true",
            "--outdir=out",
            "--entry-names=islands",
            "--chunk-names=islands-chunk-[hash]",
            "--metafile=out/meta.json",
        ]);
        zfb_rolldown_prototype::run_prepared(&cmd).unwrap();
        let inspected =
            audit_entry_imports_at_path(&cwd.join("out/meta.json"), cwd, &entry).unwrap();
        assert!(
            inspected > 0,
            "the shared and common modules must yield chunk -> chunk edges"
        );
    }
}
