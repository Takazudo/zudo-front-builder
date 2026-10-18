//! Real esbuild output regression coverage for #4231; no budget normalization.
use std::{collections::BTreeMap, fs, path::Path};
use zfb_islands::{
    BundleConfig, ClientBundler, EsbuildSubprocessBundler, EsbuildSubprocessConfig, Island,
};
use zfb_test_utils::locate_esbuild;

fn bundler(
    root: &Path,
    virtuals: Vec<(String, String)>,
    conditions: &str,
) -> EsbuildSubprocessBundler {
    let binary = locate_esbuild().expect("graph identity tests require the pinned esbuild binary");
    EsbuildSubprocessBundler::new(
        EsbuildSubprocessConfig {
            extra_args: vec![
                "--external:@takazudo/*".into(),
                format!("--conditions={conditions}").into(),
            ],
            ..EsbuildSubprocessConfig::default()
        }
        .with_binary_path(binary)
        .with_working_dir(root)
        .with_virtual_modules(virtuals),
    )
}

fn snapshot(
    root: &Path,
    virtuals: Vec<(String, String)>,
    config: BundleConfig,
    conditions: &str,
) -> (String, BTreeMap<String, Vec<u8>>) {
    snapshot_with_bundler(root, bundler(root, virtuals, conditions), config)
}

fn snapshot_with_bundler(
    root: &Path,
    bundler: EsbuildSubprocessBundler,
    config: BundleConfig,
) -> (String, BTreeMap<String, Vec<u8>>) {
    let islands = [Island::new("Counter", root.join("counter.ts"))];
    let token = bundler
        .derive_build_token(&islands, &config, "test-compiler-v1")
        .unwrap();
    let output = bundler
        .bundle(&islands, &config.with_zudo_react_build(Some(token.clone())))
        .unwrap();
    let mut files = BTreeMap::from([("islands.js".into(), output.bytes)]);
    for chunk in output.chunks.into_iter().chain(output.workers) {
        files.insert(chunk.filename, chunk.bytes);
    }
    for resource in output.resources {
        files.insert(resource.filename, resource.bytes);
    }
    (token, files)
}

fn fixture(root: &Path) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("counter.ts"), "import {value} from './direct'; export function Counter(){return value + __LABEL__} export const load = () => import('./lazy'); console.log(load);" ).unwrap();
    fs::write(
        root.join("direct.ts"),
        "export {value} from './transitive';",
    )
    .unwrap();
    fs::write(root.join("transitive.ts"), "export const value = 1;").unwrap();
    fs::write(root.join("lazy.ts"), "export const value = 'lazy';").unwrap();
}
fn config() -> BundleConfig {
    BundleConfig::production().with_define(BTreeMap::from([("__LABEL__".into(), "'one'".into())]))
}

#[test]
fn unrelated_edits_keep_raw_entry_and_chunks_stable_relevant_inputs_change_them() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let before = snapshot(root, vec![], config(), "browser");
    assert!(
        before.1.len() > 1,
        "fixture must exercise actual split chunks"
    );
    fs::write(root.join("unrelated.md"), "# An unrelated page").unwrap();
    fs::write(root.join("site.json"), r#"{"title":"changed site title"}"#).unwrap();
    fs::write(root.join("unused.ts"), "export const untouched = 100;").unwrap();
    assert_eq!(before, snapshot(root, vec![], config(), "browser"));
    fs::write(root.join("transitive.ts"), "export const value = 2;").unwrap();
    let changed = snapshot(root, vec![], config(), "browser");
    assert_ne!(before.0, changed.0);
    assert_ne!(before.1, changed.1);
    let defined = snapshot(
        root,
        vec![],
        BundleConfig::production()
            .with_define(BTreeMap::from([("__LABEL__".into(), "'two'".into())])),
        "browser",
    );
    assert_ne!(changed.0, defined.0);
    assert_ne!(changed.1, defined.1);
    let nested = temp.path().join("nested/deep/site");
    fixture(&nested);
    assert_eq!(before, snapshot(&nested, vec![], config(), "browser"));
}

#[test]
fn virtual_inputs_only_invalidate_when_reached_and_probe_is_repeatable_in_dev() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let unused = vec![("virtual:unused".into(), "export default 1;".into())];
    assert_eq!(
        snapshot(root, vec![], config(), "browser"),
        snapshot(root, unused, config(), "browser")
    );
    fs::write(
        root.join("counter.ts"),
        "import value from 'virtual:used'; export function Counter(){return value()}",
    )
    .unwrap();
    let virtuals = vec![(
        "virtual:used".into(),
        "export default function(){return 'one'}".into(),
    )];
    let first = snapshot(root, virtuals.clone(), config(), "browser");
    assert_eq!(first, snapshot(root, virtuals.clone(), config(), "browser"));
    let dev = bundler(root, virtuals, "browser");
    let islands = [Island::new("Counter", root.join("counter.ts"))];
    let first_dev = dev
        .derive_build_token(&islands, &BundleConfig::dev(), "v1")
        .unwrap();
    assert_eq!(
        first_dev,
        dev.derive_build_token(&islands, &BundleConfig::dev(), "v1")
            .unwrap()
    );
    assert_ne!(
        first_dev,
        dev.derive_build_token(&islands, &BundleConfig::dev(), "v2")
            .unwrap()
    );
    let changed = snapshot(
        root,
        vec![(
            "virtual:used".into(),
            "export default function(){return 'two'}".into(),
        )],
        config(),
        "browser",
    );
    assert_ne!(first.0, changed.0);
    assert_ne!(first.1, changed.1);
}

#[test]
fn resolved_conditions_and_dynamic_module_identity_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let pkg = root.join("node_modules/choice");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(
        pkg.join("package.json"),
        r#"{"name":"choice","exports":{"custom":"./custom.js","default":"./default.js"}}"#,
    )
    .unwrap();
    fs::write(pkg.join("custom.js"), "export const value = 2;").unwrap();
    fs::write(pkg.join("default.js"), "export const value = 1;").unwrap();
    fs::write(
        root.join("counter.ts"),
        "import {value} from 'choice'; export function Counter(){return value}",
    )
    .unwrap();
    let default = snapshot(root, vec![], config(), "browser");
    let custom = snapshot(root, vec![], config(), "custom");
    assert_ne!(default.0, custom.0);
    assert_ne!(default.1, custom.1);
    for name in ["a", "b"] {
        fs::write(
            root.join(format!("{name}.js")),
            "export const url = import.meta.url;",
        )
        .unwrap();
    }
    fs::write(
        root.join("counter.ts"),
        "export function Counter(){return import('./a.js').then(m => m.url)}",
    )
    .unwrap();
    let a = snapshot(root, vec![], config(), "browser");
    fs::write(
        root.join("counter.ts"),
        "export function Counter(){return import('./b.js').then(m => m.url)}",
    )
    .unwrap();
    let b = snapshot(root, vec![], config(), "browser");
    assert_ne!(a.0, b.0);
    assert_ne!(a.1, b.1);
}

#[test]
fn alias_linked_package_worker_and_resource_changes_invalidate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("site");
    fixture(&root);
    let package = temp.path().join("linked");
    fs::create_dir_all(&package).unwrap();
    fs::write(
        package.join("package.json"),
        r#"{"name":"linked","version":"1.0.0"}"#,
    )
    .unwrap();
    fs::write(package.join("index.js"), "export const value = 1;").unwrap();
    fs::write(
        root.join("counter.ts"),
        "import {value} from 'selected'; export function Counter(){return value}",
    )
    .unwrap();
    fs::write(
        root.join("worker.js"),
        "import {value} from './worker-value'; postMessage(value);",
    )
    .unwrap();
    fs::write(root.join("worker-value.js"), "export const value = 1;").unwrap();
    let workers = vec![zfb_islands::ModuleWorkerBundleEntry::new(
        &root,
        &root.join("worker.js"),
        root.join("worker.js"),
    )
    .unwrap()];
    let build = || {
        let base = bundler(&root, vec![], "browser");
        let cfg = base.config().clone().with_alias_entries(vec![(
            "selected".into(),
            package.join("index.js").to_string_lossy().into_owned(),
        )]);
        let bundler = EsbuildSubprocessBundler::new(cfg);
        let islands = [Island::new("Counter", root.join("counter.ts"))];
        let cfg = config().with_module_workers(workers.clone());
        let token = bundler.derive_build_token(&islands, &cfg, "v1").unwrap();
        let bytes = bundler
            .bundle(&islands, &cfg.with_zudo_react_build(Some(token.clone())))
            .unwrap();
        (token, bytes.bytes, bytes.workers)
    };
    let initial = build();
    fs::write(package.join("unused.js"), "export const ignored = 2;").unwrap();
    assert_eq!(initial, build());
    fs::write(package.join("index.js"), "export const value = 2;").unwrap();
    let linked = build();
    assert_ne!(initial.0, linked.0);
    assert_ne!(initial.1, linked.1);
    fs::write(root.join("worker-value.js"), "export const value = 2;").unwrap();
    let worker = build();
    assert_ne!(linked.0, worker.0);
    assert_ne!(linked.1, worker.1);
    assert_ne!(linked.2, worker.2);
    fs::write(root.join("module.wasm"), b"\0asm\x01\0\0\0").unwrap();
    fs::write(
        root.join("counter.ts"),
        "import url from './module.wasm'; export function Counter(){return url}",
    )
    .unwrap();
    let before = snapshot(&root, vec![], config(), "browser");
    fs::write(root.join("module.wasm"), b"\0asm\x01\0\0\0\0\0").unwrap();
    let after = snapshot(&root, vec![], config(), "browser");
    assert_ne!(before.0, after.0);
    assert_ne!(before.1, after.1);
}

#[test]
fn dev_reflection_changes_in_resolved_and_virtual_sources_invalidate_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    fs::write(
        root.join("counter.ts"),
        "import {value} from './direct'; export function Counter(){return value.toString()}",
    )
    .unwrap();
    fs::write(
        root.join("direct.ts"),
        "export function value(){return 1 + 2}",
    )
    .unwrap();
    let before = snapshot(root, vec![], BundleConfig::dev(), "browser");
    fs::write(root.join("direct.ts"), "export function value(){return 3}").unwrap();
    let after = snapshot(root, vec![], BundleConfig::dev(), "browser");
    assert_ne!(
        before.0, after.0,
        "source differences erased by probe minification must invalidate dev clients"
    );
    assert_ne!(before.1, after.1);

    fs::write(root.join("counter.ts"), "import {value} from 'virtual:reflected'; export function Counter(){return value.toString()}").unwrap();
    let before = snapshot(
        root,
        vec![(
            "virtual:reflected".into(),
            "export function value(){return 1 + 2}".into(),
        )],
        BundleConfig::dev(),
        "browser",
    );
    let after = snapshot(
        root,
        vec![(
            "virtual:reflected".into(),
            "export function value(){return 3}".into(),
        )],
        BundleConfig::dev(),
        "browser",
    );
    assert_ne!(
        before.0, after.0,
        "reached virtual source bytes must be preserved in identity"
    );
    assert_ne!(before.1, after.1);
}

#[test]
fn dev_define_spelling_changes_observable_function_source_and_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("counter.ts"), "const first = 3; function meaning(){return __X__} export function Counter(){return meaning.toString()}").unwrap();
    let configured = |value: &str| {
        BundleConfig::dev().with_define(BTreeMap::from([("__X__".into(), value.into())]))
    };
    let before = snapshot(root, vec![], configured("first"), "browser");
    let after = snapshot(root, vec![], configured("3"), "browser");
    assert_ne!(
        before.0, after.0,
        "minified-equivalent defines must not share a dev token"
    );
    assert_ne!(before.1, after.1);
    assert_eq!(
        before.0,
        snapshot(root, vec![], configured("first"), "browser").0
    );
}

#[cfg(unix)]
#[test]
fn aliased_tmpdir_and_project_root_preserve_graph_tokens_and_raw_outputs() {
    const CHILD: &str = "ZFB_GRAPH_ALIAS_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let temp = tempfile::tempdir().unwrap();
        let physical = temp.path().join("private/var/tmp");
        fs::create_dir_all(&physical).unwrap();
        let logical = temp.path().join("tmp");
        std::os::unix::fs::symlink(&physical, &logical).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "aliased_tmpdir_and_project_root_preserve_graph_tokens_and_raw_outputs",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("TMPDIR", &logical)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "aliased TMPDIR child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let logical = temp.path();
    let physical = logical.canonicalize().unwrap();
    assert_ne!(logical, physical);
    for preserve in [false, true] {
        for root in [logical, physical.as_path()] {
            fixture(root);
            fs::write(root.join("counter.ts"), "import {value} from 'virtual:demo'; export function Counter(){return value.toString()}").unwrap();
            let build = |source: &str| {
                let base = bundler(
                    root,
                    vec![("virtual:demo".into(), source.into())],
                    "browser",
                );
                let mut cfg = base.config().clone();
                if preserve {
                    cfg.extra_args.push("--preserve-symlinks".into());
                }
                snapshot_with_bundler(
                    root,
                    EsbuildSubprocessBundler::new(cfg),
                    BundleConfig::production(),
                )
            };
            let before = build("export function value(){return 1 + 2}");
            fs::write(root.join("unrelated.md"), "irrelevant edit").unwrap();
            assert_eq!(before, build("export function value(){return 1 + 2}"));
            let after = build("export function value(){return 3}");
            assert_ne!(
                before.0, after.0,
                "authored source must invalidate the token"
            );
            assert_ne!(before.1, after.1, "raw emitted bytes must change");
        }
    }
}
