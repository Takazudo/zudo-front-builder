//! Issue #3189 (epic #3187) — regression for the #3185 repro: a plugin
//! `addAlias` whose target is a file inside an installed dependency must not
//! be staged THROUGH the package link the bundler created for that same
//! dependency.
//!
//! Layout (the #3185 repro, driven at the bundler layer): a pnpm-style nested
//! workspace (`apps/site` + `packages/core`, `exports` `./button`), a
//! non-empty tsconfig `paths` (`"~/*": ["./*"]`), an empty `bundle.exclude`,
//! a page importing `@x/core/button` (arming workspace-package staging), and
//! a plugin alias `preact/hooks` -> `<site>/node_modules/preact/hooks/dist/hooks.mjs`.
//! With those four ingredients the directory loop links `<shadow>/node_modules/preact`
//! to the canonical `.pnpm/.../preact`, and the exact-target file loop then
//! used to write `hooks.mjs` through that link: `zfb build` deleted the
//! installed file, `zfb dev` replaced the link with a partial directory.
//!
//! The installed preact is a small fake package inside the fixture's own
//! `.pnpm` store (a real directory the site reaches through a pnpm-style
//! symlink), never the repository's install — a regression here deletes
//! files from the store it stages from.
//!
//! Runs on T1 by the self-skip convention (`crates/CLAUDE.md`): health.yml
//! always provisions esbuild and asserts it did. A skip is not a pass, so the
//! test refuses to skip under `CI`, and each case asserts the linked-package
//! path was really taken.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use zfb_build::{bundle, bundle_with_session, BundleMode, BundlerInput, ShadowSession};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

const HOOKS_MJS: &str = "export const HOOKS_MARKER = \"REAL_INSTALLED_HOOKS_3185\";\n\
                         export function useState(value) { return [value, () => {}]; }\n";

fn esbuild_or_skip() -> Option<PathBuf> {
    let esbuild = locate_esbuild();
    if esbuild.is_none() {
        assert!(
            std::env::var_os("CI").is_none(),
            "[bundler_linked_package_alias_target_esbuild_regression] no esbuild binary under CI: \
             a skip is not a pass"
        );
        eprintln!(
            "[bundler_linked_package_alias_target_esbuild_regression] no esbuild binary; skipping."
        );
    }
    esbuild
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

struct Fixture {
    site: PathBuf,
    /// The installed `hooks.mjs` inside the fixture's `.pnpm` store.
    installed_hooks: PathBuf,
    /// Every regular file of the installed preact, with its bytes.
    installed_files: BTreeMap<PathBuf, Vec<u8>>,
}

fn write_fixture(ws: &Path) -> Fixture {
    write(&ws.join("package.json"), r#"{"private":true}"#);
    write(
        &ws.join("pnpm-workspace.yaml"),
        "packages:\n  - 'apps/*'\n  - 'packages/*'\n",
    );

    let store = ws.join("node_modules/.pnpm/preact@10.29.8/node_modules/preact");
    write(
        &store.join("package.json"),
        r#"{
  "name": "preact",
  "version": "10.29.8",
  "type": "module",
  "exports": {
    ".": { "import": "./dist/preact.mjs" },
    "./hooks": { "import": "./hooks/dist/hooks.mjs" },
    "./jsx-runtime": { "import": "./jsx-runtime/dist/jsxRuntime.mjs" }
  }
}"#,
    );
    write(
        &store.join("dist/preact.mjs"),
        "export const options = {};\n\
         export function h(type, props) { return { type, props }; }\n\
         export const createElement = h;\n\
         export function hydrate() {}\nexport function render() {}\n\
         export function Fragment(props) { return props.children; }\n",
    );
    write(&store.join("hooks/dist/hooks.mjs"), HOOKS_MJS);
    write(
        &store.join("jsx-runtime/dist/jsxRuntime.mjs"),
        "export function jsx(type, props) { return { type, props }; }\n\
         export const jsxs = jsx;\nexport const jsxDEV = jsx;\n\
         export function Fragment(props) { return props.children; }\n",
    );

    let core = ws.join("packages/core");
    write(
        &core.join("package.json"),
        r#"{"name":"@x/core","type":"module","exports":{"./button":"./button.tsx"},"dependencies":{"preact":"10.29.8"}}"#,
    );
    write(
        &core.join("button.tsx"),
        "export default function Button() { return <button type=\"button\">CORE_BUTTON_MARKER</button>; }\n",
    );
    fs::create_dir_all(core.join("node_modules")).unwrap();
    symlink(&store, core.join("node_modules/preact")).unwrap();

    let site = ws.join("apps/site");
    for dir in ["pages", "content", "components", "layouts", "scripts"] {
        fs::create_dir_all(site.join(dir)).unwrap();
    }
    write(
        &site.join("package.json"),
        r#"{"name":"site","private":true,"dependencies":{"@x/core":"workspace:*","preact":"10.29.8"}}"#,
    );
    write(
        &site.join("pages/index.tsx"),
        r#"
            import Button from "@x/core/button";
            import { HOOKS_MARKER } from "preact/hooks";
            export default function Home() {
              return <main>{HOOKS_MARKER}<Button /></main>;
            }
        "#,
    );
    fs::create_dir_all(site.join("node_modules/@x")).unwrap();
    symlink(&store, site.join("node_modules/preact")).unwrap();
    symlink(&core, site.join("node_modules/@x/core")).unwrap();

    let installed_files = walkdir_files(&store);
    assert!(installed_files.len() >= 4, "{installed_files:?}");
    Fixture {
        installed_hooks: store.join("hooks/dist/hooks.mjs"),
        site,
        installed_files,
    }
}

fn walkdir_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                let bytes = fs::read(&path).unwrap();
                files.insert(path, bytes);
            }
        }
    }
    files
}

fn input(fixture: &Fixture, esbuild: PathBuf, mode: BundleMode) -> BundlerInput {
    let site = &fixture.site;
    let mut input = BundlerInput::for_project(
        site.clone(),
        Framework::Preact,
        mode,
        site.join("dist"),
        None,
    );
    input.external = vec![
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input.esbuild_binary = Some(esbuild);
    input.node_modules_dir = Some(site.join("node_modules"));
    input.tsconfig_paths = BTreeMap::from([(
        "~/*".to_string(),
        vec![site.join("*").to_string_lossy().into_owned()],
    )]);
    // `plugins: [{ name: "./scripts/alias-plugin.mjs" }]` calling
    // `addAlias("preact/hooks", resolve(projectRoot, "node_modules/preact/hooks/dist/hooks.mjs"))`.
    input.plugin_alias_entries = vec![(
        "preact/hooks".to_string(),
        site.join("node_modules/preact/hooks/dist/hooks.mjs")
            .to_string_lossy()
            .into_owned(),
    )];
    assert!(input.bundle_exclude.is_empty());
    input
}

fn assert_install_untouched(fixture: &Fixture, when: &str) {
    assert_eq!(
        fs::read(&fixture.installed_hooks)
            .unwrap_or_else(|e| panic!("{when}: the installed hooks.mjs must survive: {e}")),
        HOOKS_MJS.as_bytes(),
        "{when}: the installed hooks.mjs bytes must be unchanged"
    );
    let store = fixture.installed_hooks.ancestors().nth(3).unwrap();
    assert_eq!(
        walkdir_files(store),
        fixture.installed_files,
        "{when}: the installed preact package must be byte-identical"
    );
}

fn assert_bundle_serves_real_files(bundle_path: &Path, when: &str) {
    let body = fs::read_to_string(bundle_path).expect("read bundle");
    assert!(
        body.contains("REAL_INSTALLED_HOOKS_3185"),
        "{when}: the alias must serve the installed hooks.mjs: {body}"
    );
    assert!(
        body.contains("CORE_BUTTON_MARKER"),
        "{when}: the workspace package must be bundled: {body}"
    );
}

#[test]
fn build_stages_alias_target_inside_linked_package_without_touching_the_install() {
    let Some(esbuild) = esbuild_or_skip() else {
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = fs::canonicalize(tmp.path()).unwrap();
    let fixture = write_fixture(&ws);

    let out = bundle(input(&fixture, esbuild, BundleMode::Production))
        .unwrap_or_else(|e| panic!("#3185: the build must succeed: {e:#}"));

    assert_install_untouched(&fixture, "after zfb build");
    assert_bundle_serves_real_files(&out.bundle_path, "zfb build");
}

#[test]
fn dev_session_keeps_the_linked_package_and_the_install_across_ticks() {
    let Some(esbuild) = esbuild_or_skip() else {
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = fs::canonicalize(tmp.path()).unwrap();
    let fixture = write_fixture(&ws);

    let mut session = ShadowSession::new(&fixture.site).unwrap();
    // The session shadow mirrors the workspace; the project sits at its
    // workspace-relative slot, and staged dependencies at their logical paths.
    let shadow_preact = session.shadow_root().join("apps/site/node_modules/preact");
    for tick in 1..=2 {
        let when = format!("dev tick {tick}");
        let out = bundle_with_session(
            input(&fixture, esbuild.clone(), BundleMode::Development),
            Some(&mut session),
        )
        .unwrap_or_else(|e| panic!("#3185: {when} must succeed: {e:#}"));

        // Proof the linked-package path was taken (not a skipped scenario):
        // the shadow's preact is a LINK to the installed package, never a
        // partial real directory holding only the alias target.
        let meta = fs::symlink_metadata(&shadow_preact)
            .unwrap_or_else(|e| panic!("{when}: {} must exist: {e}", shadow_preact.display()));
        assert!(
            meta.file_type().is_symlink(),
            "{when}: {} must stay linked to the install",
            shadow_preact.display()
        );
        assert_eq!(
            fs::canonicalize(&shadow_preact).unwrap(),
            fixture.installed_hooks.ancestors().nth(3).unwrap(),
        );
        assert_install_untouched(&fixture, &when);
        assert_bundle_serves_real_files(&out.bundle_path, &when);
    }
}
