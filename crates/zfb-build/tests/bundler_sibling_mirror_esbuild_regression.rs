//! Issue #1698 — end-to-end real-esbuild regression matrix confirming the
//! Sibling Mirror epic (#1691: wholesale sibling/root-file staging, the
//! `import.meta.glob` fixed-point queue, the workspace-hoisted
//! `node_modules` link under exclusions, and the sibling `.module.css`
//! work-mirror rewrite) is wired together correctly END TO END, driven by
//! the REAL esbuild binary — never `mock_subprocess_output`.
//!
//! Every implementation sub-issue's own test suite stops at the
//! mock/staging-shape level (`bundle_with_session` + `mock_subprocess_output`
//! — see e.g. `bundler_sibling_wholesale_mirror.rs` and the
//! `sibling_bare_import_resolves_via_workspace_node_modules_under_exclude`
//! unit test in `bundler.rs`, whose own doc comment says "real-esbuild
//! confirmation belongs to sub-issue #1698"). This file is that
//! confirmation — the epic's central heavy-verification pass.
//!
//! Every case below reproduces one leaf of the epic's problem statement
//! (issue #1685's repro + follow-up comments) with an UNRELATED, non-empty
//! `bundle.exclude` active — the exact combination that broke a real site:
//! root-level/sibling files reachable only through a wildcard alias, an
//! eager `import.meta.glob`, a bare dependency, or a `.module.css` import
//! all miss under the pre-epic "stage only what the AST-discovery graph
//! directly sees" model, because ANY non-empty `bundle.exclude` suppresses
//! the live-tree dual-target fallback (see `l-lessons-client-bundling`:
//! exclusion = absence from a staged shadow tree, esbuild remains the sole
//! resolver — Rust only ever collects candidate SETS).
//!
//! ## Regression criterion
//!
//! Every test here drives a genuine real-esbuild `bundle()` call (never a
//! mock), so a build-green assertion is directly observable, and most cases
//! fail pre-epic as an outright `bundle()` `Err` ("Could not resolve ...",
//! or an esbuild CSS-as-JS parse error for case (e)) rather than a subtler
//! content mismatch. Each test was verified to FAIL when cherry-picked onto
//! the epic's parent (`base/sweep-260718`, pre-epic) and PASS on the epic
//! branch — see the PR/issue description for both run transcripts.
//!
//! ## Case (b) — parked, not passing on the epic branch
//!
//! Case (b) is `#[ignore]`d: it exposes a genuine, real bug (build succeeds
//! GREEN but with the wrong content — a sibling alias target's own
//! `import.meta.glob` macro is staged raw and never expanded), not a fixture
//! mistake. Confirmed with both a wildcard and a concrete alias target;
//! root-caused and filed as issue #1724 (see the `#[ignore]` reason on the
//! test itself). It still fails pre-epic (for the ordinary "Could not
//! resolve" reason every other case fails for), so the regression-criterion
//! FAIL side holds — it just doesn't reach PASS on the epic branch, which is
//! exactly why it is parked rather than asserted.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{bundle, bundle_with_session, BundleMode, BundlerInput, ShadowSession};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Standard content-root dirs `bundle()` expects under a project.
fn scaffold_project_dirs(project: &Path) {
    for d in ["pages", "content", "components", "layouts"] {
        fs::create_dir_all(project.join(d)).unwrap();
    }
}

/// A minimal pnpm workspace with `project` as a `sub-packages/*` member.
/// Returns `(workspace_root, project_root)`.
fn write_workspace(tmp_root: &Path) -> (PathBuf, PathBuf) {
    fs::write(
        tmp_root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'sub-packages/*'\n  - 'packages/*'\n",
    )
    .unwrap();
    let project = tmp_root.join("sub-packages/host");
    scaffold_project_dirs(&project);
    (tmp_root.to_path_buf(), project)
}

/// Shared `BundlerInput` defaults for these fixtures: Preact production
/// build, real esbuild binary, runtime/preact bare specifiers marked
/// external (mirrors `bundler_exclude_glob.rs`'s `make_input`).
fn base_input(project: &Path, esbuild: PathBuf, bundle_exclude: Vec<String>) -> BundlerInput {
    let mut input = BundlerInput::for_project(
        project.to_path_buf(),
        Framework::Preact,
        BundleMode::Production,
        project.join("dist"),
        None,
    );
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input.esbuild_binary = Some(esbuild);
    input.bundle_exclude = bundle_exclude;
    input
}

/// A non-empty, entirely UNRELATED `bundle.exclude` pattern — the exact
/// shape from issue #1685's repro (an exclude that matches nothing in the
/// fixture, present only to arm the shadow-only / no-live-fallback regime).
fn unrelated_exclude() -> Vec<String> {
    vec!["components/never-matches/**".to_string()]
}

fn truncate(s: &str) -> String {
    const LIMIT: usize = 1200;
    if s.len() <= LIMIT {
        return s.to_string();
    }
    let cut = (0..=LIMIT)
        .rev()
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(0);
    format!("{}…[truncated]", &s[..cut])
}

// ---------------------------------------------------------------------------
// (a) project-root-file variant: tsconfig `"@/*": ["./*"]` -> a repo-root
// JSON, plus an unrelated non-empty `bundle.exclude` — the exact repro from
// issue #1685's comment (zzmod main site, `zfb@0.1.0-next.88`).
// ---------------------------------------------------------------------------

#[test]
fn a_root_wildcard_alias_reaches_root_json_under_unrelated_exclude() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path();
    scaffold_project_dirs(project);

    fs::write(
        project.join("metadata-db.json"),
        r#"{ "ROOT_JSON_MARKER": true }"#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import metadataDb from "@/metadata-db.json";
            export default function Home() {
              return JSON.stringify(metadataDb);
            }
        "#,
    )
    .unwrap();

    let tsconfig_paths = BTreeMap::from([(
        "@/*".to_string(),
        vec![project.join("*").to_string_lossy().into_owned()],
    )]);

    let mut input = base_input(project, esbuild, unrelated_exclude());
    input.tsconfig_paths = tsconfig_paths;

    let out = bundle(input).expect(
        "issue #1685: a project-root file reached via a wildcard root alias \
         (\"@/*\": [\"./*\"]) must still build green under an UNRELATED \
         non-empty bundle.exclude",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("ROOT_JSON_MARKER"),
        "the aliased root JSON's content must reach the bundle: {}",
        truncate(&body)
    );
}

// ---------------------------------------------------------------------------
// (b) sibling `import.meta.glob` under exclusions.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "pending-feature: https://github.com/Takazudo/zudo-front-builder/issues/1724 \
            — a sibling alias target's own import.meta.glob macro is staged \
            raw (mirror_sibling_root's wholesale copy) and never expanded, \
            because the exact-target discovery/materialise pass that would \
            expand it is gated to project_root-only targets (bundler.rs \
            ~2149-2185). Build stays GREEN but with the literal unexpanded \
            macro text in the bundle -- confirmed with both a wildcard and a \
            concrete alias target."]
fn b_sibling_import_meta_glob_expands_under_unrelated_exclude() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ws_root, project) = write_workspace(tmp.path());

    // `_gallery.ts` is reached through a CONCRETE (non-wildcard) tsconfig
    // alias, so it lands in `exact_target_staging_files` and gets its own
    // `discover_module_preprocessing_with_context` walk (claim source (a)) —
    // the ordinary exact-target staging/materialise pass therefore expands
    // its top-level `import.meta.glob` macro, same as any project file. A
    // WILDCARD alias target (case (c)/(d)/(e)'s `@shared/*` mechanism) only
    // ever gets the wholesale RAW-byte mirror (#1692) with no per-file
    // preprocessing — by design, since Rust cannot enumerate a wildcard's
    // members without walking the tree, so this file's own glob macro would
    // stay unexpanded there. Its glob TARGET (`items/`) is invisible to the
    // AST-discovery graph either way — only the #1695 fixed-point queue
    // reaches it.
    fs::create_dir_all(ws_root.join("lib/shared/items")).unwrap();
    fs::write(
        ws_root.join("lib/shared/_gallery.ts"),
        r#"
            const mods = import.meta.glob('./items/*.ts', { eager: true });
            export const galleryKeys = Object.keys(mods);
        "#,
    )
    .unwrap();
    fs::write(
        ws_root.join("lib/shared/items/one.ts"),
        "export const value = 'GLOB_ITEM_ONE';\n",
    )
    .unwrap();
    fs::write(
        ws_root.join("lib/shared/items/two.ts"),
        "export const value = 'GLOB_ITEM_TWO';\n",
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import { galleryKeys } from "@gallery";
            export default function Home() {
              return galleryKeys.join(",");
            }
        "#,
    )
    .unwrap();

    let tsconfig_paths = BTreeMap::from([(
        "@gallery".to_string(),
        vec![ws_root
            .join("lib/shared/_gallery.ts")
            .to_string_lossy()
            .into_owned()],
    )]);
    let mut input = base_input(&project, esbuild, unrelated_exclude());
    input.tsconfig_paths = tsconfig_paths;
    let out = bundle(input).expect(
        "issue #1691/#1695: a sibling file's own import.meta.glob must expand \
         (and its matched files be staged and resolvable) under an UNRELATED \
         non-empty bundle.exclude",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        !body.contains("import.meta.glob("),
        "the sibling's own glob macro must be expanded, not raw-copied \
         verbatim (the wholesale mirror alone only copies bytes): {}",
        truncate(&body)
    );
    for marker in ["GLOB_ITEM_ONE", "GLOB_ITEM_TWO"] {
        assert!(
            body.contains(marker),
            "the glob-matched sibling item {marker} must reach the bundle: {}",
            truncate(&body)
        );
    }
}

// ---------------------------------------------------------------------------
// (c) sibling wildcard-alias leaf under unrelated exclusions.
// ---------------------------------------------------------------------------

#[test]
fn c_sibling_wildcard_alias_leaf_resolves_under_unrelated_exclude() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ws_root, project) = write_workspace(tmp.path());

    // NOT imported anywhere via a relative path — the wildcard alias is the
    // ONLY claim source that ever sees this file (mirrors
    // `wildcard_alias_only_claim_mirrors_target_dir`'s mock fixture, but
    // here the alias is actually IMPORTED and must resolve for real).
    fs::create_dir_all(ws_root.join("lib/shared")).unwrap();
    fs::write(
        ws_root.join("lib/shared/helper.ts"),
        "export const help = 'WILDCARD_ALIAS_LEAF_MARKER';\n",
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import { help } from "@shared/helper";
            export default function Home() {
              return help;
            }
        "#,
    )
    .unwrap();

    let tsconfig_paths = BTreeMap::from([(
        "@shared/*".to_string(),
        vec![ws_root.join("lib/shared/*").to_string_lossy().into_owned()],
    )]);

    let mut input = base_input(&project, esbuild, unrelated_exclude());
    input.tsconfig_paths = tsconfig_paths;

    let out = bundle(input).expect(
        "issue #1691/#1692: a sibling file reachable ONLY through a wildcard \
         tsconfig alias must still resolve under an UNRELATED non-empty \
         bundle.exclude",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("WILDCARD_ALIAS_LEAF_MARKER"),
        "the wildcard-alias-only sibling leaf must reach the bundle: {}",
        truncate(&body)
    );
}

// ---------------------------------------------------------------------------
// (d) sibling bare dependency under exclusions.
// ---------------------------------------------------------------------------

#[test]
fn d_sibling_bare_dependency_resolves_via_workspace_node_modules_under_unrelated_exclude() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ws_root, project) = write_workspace(tmp.path());

    // A workspace-hoisted bare dependency resolved via an `exports` map, so
    // the fixture is independent of esbuild's `--platform=neutral`
    // main-fields gate (see `bundler_exclude_glob.rs`'s CJS-only fixture for
    // that unrelated failure mode, deliberately avoided here).
    let dep = ws_root.join("node_modules/sibling-dep");
    fs::create_dir_all(&dep).unwrap();
    fs::write(
        dep.join("package.json"),
        r#"{ "name": "sibling-dep", "version": "1.0.0", "exports": "./index.mjs" }"#,
    )
    .unwrap();
    fs::write(
        dep.join("index.mjs"),
        "export const greet = () => 'SIBLING_BARE_DEP_MARKER';\n",
    )
    .unwrap();

    fs::create_dir_all(ws_root.join("lib/shared")).unwrap();
    fs::write(
        ws_root.join("lib/shared/widget.ts"),
        r#"
            import { greet } from "sibling-dep";
            export const value = greet();
        "#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import { value } from "@shared/widget";
            export default function Home() {
              return value;
            }
        "#,
    )
    .unwrap();

    // Reached via a tsconfig wildcard alias (claim source (b) — same
    // reachability mechanism as case (c); a plain relative import escaping
    // `project_root` is rejected by the SSR page bundle pass, a
    // pre-existing #1386 decision orthogonal to this epic).
    let tsconfig_paths = BTreeMap::from([(
        "@shared/*".to_string(),
        vec![ws_root.join("lib/shared/*").to_string_lossy().into_owned()],
    )]);
    let mut input = base_input(&project, esbuild, unrelated_exclude());
    input.tsconfig_paths = tsconfig_paths;
    let out = bundle(input).expect(
        "issue #1691/#1693: a sibling file's bare dependency must resolve \
         via the workspace-hoisted node_modules link under an UNRELATED \
         non-empty bundle.exclude",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("SIBLING_BARE_DEP_MARKER"),
        "the sibling's bare dependency must reach the bundle: {}",
        truncate(&body)
    );
}

// ---------------------------------------------------------------------------
// (e) sibling `.module.css` through the pipeline.
// ---------------------------------------------------------------------------

#[test]
fn e_sibling_module_css_rewrite_reaches_bundle_under_unrelated_exclude() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ws_root, project) = write_workspace(tmp.path());

    fs::create_dir_all(ws_root.join("lib/shared")).unwrap();
    let css_path = ws_root.join("lib/shared/Widget.module.css");
    fs::write(&css_path, ".box { color: red; }\n").unwrap();
    fs::write(
        ws_root.join("lib/shared/Widget.ts"),
        r#"
            import styles from "./Widget.module.css";
            export const cls = styles.box;
        "#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import { cls } from "@shared/Widget";
            export default function Home() {
              return cls;
            }
        "#,
    )
    .unwrap();

    // Reached via a tsconfig wildcard alias (claim source (b) — same
    // reachability mechanism as case (c); a plain relative import escaping
    // `project_root` is rejected by the SSR page bundle pass, a
    // pre-existing #1386 decision orthogonal to this epic).
    let tsconfig_paths = BTreeMap::from([(
        "@shared/*".to_string(),
        vec![ws_root.join("lib/shared/*").to_string_lossy().into_owned()],
    )]);
    let mut input = base_input(&project, esbuild, unrelated_exclude());
    input.tsconfig_paths = tsconfig_paths;
    // This crate has no CSS discovery/scan layer of its own — that wiring
    // (`discover_css_source_files` / `compute_css_module_class_maps`) is the
    // job of the SEPARATE `crates/zfb` integration test in this matrix
    // (`sibling_css_module_command_layer_build.rs`). Here the class map is
    // pre-supplied, exactly the "shortcut" documented on
    // `crates/zfb-build/tests/bundler_css_modules.rs`, keyed by the
    // sibling's PHYSICAL absolute path — the same key
    // `rewrite_css_modules_in_work_mirror` reconstructs from the work-mirror
    // slot via `first_party_root`.
    input.css_module_class_maps = HashMap::from([(
        css_path.clone(),
        HashMap::from([("box".to_string(), "hashed_box_marker".to_string())]),
    )]);

    let out = bundle(input).expect(
        "issue #1691/#1697: a sibling `.module.css` staged wholesale into the \
         work mirror must be rewritten to a valid JS class-map shim (not left \
         as raw CSS bytes, which esbuild's `--loader:.module.css=js` flag \
         would fail to parse as JS) under an UNRELATED non-empty \
         bundle.exclude",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("hashed_box_marker"),
        "the sibling .module.css's rewritten scoped class name must reach \
         the bundle: {}",
        truncate(&body)
    );
}

// ---------------------------------------------------------------------------
// (f) package-name workspace sibling staging (#1901).
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn f_workspace_package_subpath_exports_resolve_only_from_real_staged_copies() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ws_root, project) = write_workspace(tmp.path());
    fs::write(
        project.join("package.json"),
        r#"{"name":"host","dependencies":{"@acme/ui":"workspace:*"}}"#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import { marker } from "@acme/ui/button";
            export default function Home() { return marker; }
        "#,
    )
    .unwrap();

    let ui = ws_root.join("packages/ui");
    fs::create_dir_all(ui.join("src/assets")).unwrap();
    fs::write(
        ui.join("package.json"),
        r#"{
          "name":"@acme/ui",
          "dependencies":{"toolkit":"workspace:*"},
          "exports":{"./button":{"browser":"./src/browser.ts","default":"./src/button.ts"}}
        }"#,
    )
    .unwrap();
    fs::write(
        ui.join("src/button.ts"),
        "import label from './assets/label.json';\n\
         import { suffix } from 'toolkit/feature';\n\
         export const marker = 'PACKAGE_EXPORT_SELECTED_' + label.value + suffix;\n",
    )
    .unwrap();
    fs::write(
        ui.join("src/assets/label.json"),
        r#"{"value":"RELATIVE_ASSET"}"#,
    )
    .unwrap();
    fs::write(
        ui.join("src/browser.ts"),
        "export const marker = 'INACTIVE_BROWSER_CONDITION';\n",
    )
    .unwrap();
    fs::create_dir_all(ui.join("dist")).unwrap();
    fs::write(
        ui.join("dist/decoy.js"),
        "export default 'UNSELECTED_DECOY';\n",
    )
    .unwrap();

    let toolkit = ws_root.join("packages/toolkit");
    fs::create_dir_all(toolkit.join("src")).unwrap();
    fs::write(
        toolkit.join("package.json"),
        r#"{"name":"toolkit","exports":{"./feature":"./src/feature.ts"}}"#,
    )
    .unwrap();
    fs::write(
        toolkit.join("src/feature.ts"),
        "export const suffix = '_TRANSITIVE_WORKSPACE_DEP';\n",
    )
    .unwrap();

    fs::create_dir_all(ws_root.join("node_modules/@acme")).unwrap();
    std::os::unix::fs::symlink(&ui, ws_root.join("node_modules/@acme/ui")).unwrap();
    std::os::unix::fs::symlink(&toolkit, ws_root.join("node_modules/toolkit")).unwrap();

    for (case, exclude, project_local_link) in [
        ("empty exclude with hoisted install", Vec::new(), false),
        (
            "active unrelated exclude with hoisted install",
            unrelated_exclude(),
            false,
        ),
        ("empty exclude with project-local link", Vec::new(), true),
    ] {
        if project_local_link {
            fs::create_dir_all(project.join("node_modules/@acme")).unwrap();
            std::os::unix::fs::symlink(&ui, project.join("node_modules/@acme/ui")).unwrap();
        }
        let input = base_input(&project, esbuild.clone(), exclude);
        let mut session = ShadowSession::new(&project).unwrap();
        let out = bundle_with_session(input, Some(&mut session)).unwrap_or_else(|error| {
            panic!("{case}: declared workspace package subpath export must stage: {error:#}")
        });
        let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
        assert!(body.contains("PACKAGE_EXPORT_SELECTED_"), "{case}: {body}");
        assert!(body.contains("RELATIVE_ASSET"), "{case}: {body}");
        assert!(body.contains("TRANSITIVE_WORKSPACE_DEP"), "{case}: {body}");
        assert!(
            !body.contains("INACTIVE_BROWSER_CONDITION"),
            "{case}: {body}"
        );
        assert!(!body.contains("UNSELECTED_DECOY"), "{case}: {body}");

        let work = fs::canonicalize(session.shadow_root()).unwrap();
        let staged = work.join("sub-packages/host/node_modules/@acme/ui");
        assert!(staged.join("package.json").is_file(), "{case}");
        assert!(staged.join("src/button.ts").is_file(), "{case}");
        assert!(staged.join("src/assets/label.json").is_file(), "{case}");
        assert!(
            !staged.join("dist").exists(),
            "{case}: workspace package infra is pruned"
        );
        assert!(
            staged.join("node_modules/toolkit/package.json").is_file(),
            "{case}: the staged package manifest must authorize its transitive bare dependency"
        );
        assert!(
            !fs::symlink_metadata(&staged)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false),
            "{case}: staged package must be a usable real directory, not a live symlink"
        );
    }
}
