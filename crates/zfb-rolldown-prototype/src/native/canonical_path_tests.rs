//! Canonical shadow spellings that reach Rolldown outside the prepared argv.
//! ZFB writes `fs::canonicalize` results into tsconfig `paths`, rewritten
//! import specifiers, and out-of-cwd inputs; on Windows those are verbatim
//! (`\\?\C:\…`), which the argv decoder never sees (#4336 ZFB-RD-007, B2).
//! Elsewhere these pass as ordinary absolute-path regressions.
use super::run_prepared;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

struct Dir(PathBuf);
impl Dir {
    fn new(role: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "zfb-native-canonical-{role}-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn prepared(cwd: &Path) -> Command {
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(cwd).args([
        "entry.js",
        "--bundle",
        "--format=esm",
        "--outfile=out/bundle.mjs",
        "--metafile=out/meta.json",
    ]);
    cmd
}

/// A JS string literal for `path`, as the shadow rewriter emits it.
fn specifier(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).unwrap()
}

fn bundle_and_inputs(cwd: &Path) -> (String, Vec<String>) {
    let bundle = fs::read_to_string(cwd.join("out/bundle.mjs")).unwrap();
    let meta: Value =
        serde_json::from_slice(&fs::read(cwd.join("out/meta.json")).unwrap()).unwrap();
    let inputs = meta["inputs"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    (bundle, inputs)
}

fn assert_no_verbatim(inputs: &[String]) {
    for key in inputs {
        assert!(
            !key.starts_with(r"\\?\") && !key.starts_with("//?/") && !key.contains('?'),
            "metafile key keeps a verbatim spelling: {key} ({inputs:?})"
        );
    }
}

#[test]
fn tsconfig_paths_to_a_canonical_target_resolve() {
    let site = Dir::new("tsconfig");
    fs::create_dir(site.0.join("lib")).unwrap();
    fs::write(
        site.0.join("lib/marker.js"),
        "export const m = 'TSCONFIG_MARKER';\n",
    )
    .unwrap();
    let target = site.0.join("lib").join("*");
    fs::write(
        site.0.join("tsconfig.json"),
        format!(
            r#"{{"compilerOptions":{{"baseUrl":".","paths":{{"@lib/*":[{}]}}}}}}"#,
            specifier(&target)
        ),
    )
    .unwrap();
    fs::write(
        site.0.join("entry.js"),
        "import { m } from '@lib/marker.js'; export default m;\n",
    )
    .unwrap();
    let mut cmd = prepared(&site.0);
    cmd.arg("--tsconfig=tsconfig.json");
    run_prepared(&cmd).unwrap();
    let (bundle, inputs) = bundle_and_inputs(&site.0);
    assert!(bundle.contains("TSCONFIG_MARKER"), "{bundle}");
    assert!(inputs.contains(&"lib/marker.js".to_string()), "{inputs:?}");
    assert_no_verbatim(&inputs);
}

#[test]
fn rewritten_absolute_import_of_a_canonical_path_resolves() {
    let site = Dir::new("absolute");
    fs::write(
        site.0.join("dep.js"),
        "export const d = 'ABSOLUTE_MARKER';\n",
    )
    .unwrap();
    fs::write(
        site.0.join("entry.js"),
        format!(
            "import {{ d }} from {}; export default d;\n",
            specifier(&site.0.join("dep.js"))
        ),
    )
    .unwrap();
    run_prepared(&prepared(&site.0)).unwrap();
    let (bundle, inputs) = bundle_and_inputs(&site.0);
    assert!(bundle.contains("ABSOLUTE_MARKER"), "{bundle}");
    assert!(inputs.contains(&"dep.js".to_string()), "{inputs:?}");
    assert_no_verbatim(&inputs);
}

#[test]
fn input_outside_cwd_is_keyed_relative_to_cwd_like_esbuild() {
    let site = Dir::new("site");
    let outside = Dir::new("outside");
    let dep = outside.0.join("dep.js");
    fs::write(&dep, "export const o = 'OUTSIDE_MARKER';\n").unwrap();
    fs::write(
        site.0.join("entry.js"),
        format!(
            "import {{ o }} from {}; export default o;\n",
            specifier(&dep)
        ),
    )
    .unwrap();
    run_prepared(&prepared(&site.0)).unwrap();
    let (bundle, inputs) = bundle_and_inputs(&site.0);
    assert!(bundle.contains("OUTSIDE_MARKER"), "{bundle}");
    assert_no_verbatim(&inputs);
    let key = inputs
        .iter()
        .find(|key| key.ends_with("dep.js"))
        .unwrap_or_else(|| panic!("no outside input in {inputs:?}"));
    // Esbuild spells a file outside its working directory with `..` segments,
    // and zfb-build's stage audits match that spelling.
    assert!(key.starts_with("../"), "{key}");
    assert_eq!(fs::canonicalize(site.0.join(key)).unwrap(), dep, "{key}");
}
