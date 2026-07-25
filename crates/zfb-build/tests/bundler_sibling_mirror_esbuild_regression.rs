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
//! ## Case (b) — was parked as `#[ignore]`, fixed and asserted since #1985
//!
//! Case (b) exposed a genuine bug (build succeeds GREEN but with the wrong
//! content — a sibling alias target's own `import.meta.glob` macro staged
//! raw and never expanded), not a fixture mistake; it was filed as issue
//! #1724 and parked. Issue #1985 (Staging Correctness epic #1982, Wave 2)
//! fixed it by enrolling every mirrored sibling SOURCE file in
//! `plugin_preprocessing_files`, so the preprocessing-materialise pass
//! overwrites the mirror's raw byte copy with the expanded form. Case (b)
//! and its three siblings below (g/j/k) are ordinary asserted tests now.

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
fn b_sibling_import_meta_glob_expands_under_unrelated_exclude() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ws_root, project) = write_workspace(tmp.path());

    // `_gallery.ts` is reached through a CONCRETE (non-wildcard) tsconfig
    // alias; case (g) below is the same shape through a WILDCARD one. Since
    // #1985 the alias shape no longer matters: both reach the sibling only
    // through the wholesale RAW-byte mirror (#1692), and every mirrored
    // SOURCE file is now enrolled in `plugin_preprocessing_files`, so the
    // preprocessing-materialise pass overwrites the raw copy with the
    // expanded macro. Its glob TARGET (`items/`) is invisible to the
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
// Issue #1983 (Staging Correctness epic #1982) — the six-case matrix for
// #1724: three macro kinds (`import.meta.glob`, `?raw`, module-worker
// `new URL(...)`) x two alias shapes (wildcard `@shared/*`, concrete
// `@gallery`-style). Case (b) above is the glob/concrete cell; the five
// siblings below fill in the remaining cells.
//
// ## Confirmed result: 4 of 6 cells reproduced, 2 did not — all 6 assert now
//
// `import.meta.glob` (case (b) + case (g) below) and the module-worker
// `new URL(...)` macro (cases (j)/(k) below) DID reproduce, both alias
// shapes: no pass ever handed a claimed sibling's macro-bearing source to
// `materialise_source_file`, so the wholesale raw-byte mirror's verbatim
// copy was final regardless of which alias shape reached it. Wave 2 (#1985)
// fixed that by enrolling every mirrored SOURCE file in
// `plugin_preprocessing_files`; the four `pending-feature: #1724` `#[ignore]`
// tags are gone and all four assert.
//
// `?raw` never reproduced (cases (h)/(i) below, proven not assumed — see
// their own section doc comment) — a SEPARATE preflight pass in
// `mirror_sibling_root` already covered it. Those two landed as ordinary
// passing regression tests, never red #1724 tests.
//
// ## Module-worker assertion shape
//
// The plain `bundle()` SSR pipeline never compiles a module worker's own
// companion file — `ssr_worker_island_bundles_without_browser_entry_in_server_graph`
// in `crates/zfb-build/src/bundler.rs` documents this: "the rewritten island
// itself bundles, but the browser-only worker entry cannot appear because the
// transform injected no import edge." So the worker cases here assert on the
// REWRITTEN SPECIFIER TEXT the macro-expansion pass would leave in the SSR
// bundle (a stable `<slug>.js?v=<hash>` filename replacing the literal
// `new URL('./worker.ts', import.meta.url)`), not on worker-companion bytes.
// ---------------------------------------------------------------------------

#[test]
fn g_sibling_import_meta_glob_wildcard_alias_expands_under_unrelated_exclude() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ws_root, project) = write_workspace(tmp.path());

    fs::create_dir_all(ws_root.join("lib/shared/items-w")).unwrap();
    fs::write(
        ws_root.join("lib/shared/gallery-w.ts"),
        r#"
            const mods = import.meta.glob('./items-w/*.ts', { eager: true });
            export const galleryKeys = Object.keys(mods);
        "#,
    )
    .unwrap();
    fs::write(
        ws_root.join("lib/shared/items-w/one.ts"),
        "export const value = 'GLOB_W_ITEM_ONE';\n",
    )
    .unwrap();
    fs::write(
        ws_root.join("lib/shared/items-w/two.ts"),
        "export const value = 'GLOB_W_ITEM_TWO';\n",
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import { galleryKeys } from "@shared/gallery-w";
            export default function Home() {
              return galleryKeys.join(",");
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
        "issue #1724: a sibling file's own import.meta.glob reached via a \
         WILDCARD alias must expand (and its matched files be staged and \
         resolvable) under an UNRELATED non-empty bundle.exclude",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        !body.contains("import.meta.glob("),
        "the sibling's own glob macro must be expanded, not raw-copied \
         verbatim: {}",
        truncate(&body)
    );
    for marker in ["GLOB_W_ITEM_ONE", "GLOB_W_ITEM_TWO"] {
        assert!(
            body.contains(marker),
            "the glob-matched sibling item {marker} must reach the bundle: {}",
            truncate(&body)
        );
    }
}

// ---------------------------------------------------------------------------
// `?raw` does NOT reproduce #1724 — proven, not assumed.
//
// The issue speculates the same root cause "presumably" affects `?raw`
// imports too. It does not: `mirror_sibling_root`'s wholesale copy pass runs
// `preflight_raw_file` (see its doc comment: "a best-effort first pass used
// only to establish terminal target identity before the broad SSR mirror
// visits those files") for every mirrored sibling file, INDEPENDENTLY of the
// preprocessing-materialise pass that #1985 had to reach for glob/worker
// expansion. So a sibling alias target's own terminal `?raw` import already
// resolves and inlines correctly today, confirmed both wildcard- and
// concrete-alias-reached, real esbuild, under an UNRELATED non-empty
// `bundle.exclude` — the exact matrix cell the issue predicted would be
// broken. These two tests are therefore ordinary (non-`#[ignore]`d)
// regression coverage locking in the CORRECT current behavior, not red
// #1724 tests.
// ---------------------------------------------------------------------------

#[test]
fn h_sibling_raw_import_wildcard_alias_already_expands_under_unrelated_exclude() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ws_root, project) = write_workspace(tmp.path());

    fs::create_dir_all(ws_root.join("lib/shared")).unwrap();
    fs::write(
        ws_root.join("lib/shared/payload-w.txt"),
        "RAW_W_PAYLOAD_MARKER",
    )
    .unwrap();
    fs::write(
        ws_root.join("lib/shared/raw-holder-w.ts"),
        r#"
            import payload from './payload-w.txt?raw';
            export const rawMarker = payload;
        "#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import { rawMarker } from "@shared/raw-holder-w";
            export default function Home() {
              return rawMarker;
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
        "a sibling file's own terminal ?raw import reached via a WILDCARD \
         alias must build green under an UNRELATED non-empty bundle.exclude",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("RAW_W_PAYLOAD_MARKER"),
        "the ?raw target's inlined text must reach the bundle: {}",
        truncate(&body)
    );
    // Only the source-name COMMENT esbuild banners each module with may
    // still show the `?raw` suffix (cosmetic, from the original specifier)
    // — the actual code must be a plain inlined string assignment, never a
    // literal unresolved `import ... from "...?raw"` declaration.
    assert!(
        !body.contains("import payload from"),
        "the ?raw import declaration itself must not survive verbatim: {}",
        truncate(&body)
    );
}

#[test]
fn i_sibling_raw_import_concrete_alias_already_expands_under_unrelated_exclude() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ws_root, project) = write_workspace(tmp.path());

    fs::create_dir_all(ws_root.join("lib/shared")).unwrap();
    fs::write(
        ws_root.join("lib/shared/payload-c.txt"),
        "RAW_C_PAYLOAD_MARKER",
    )
    .unwrap();
    fs::write(
        ws_root.join("lib/shared/raw-holder-c.ts"),
        r#"
            import payload from './payload-c.txt?raw';
            export const rawMarker = payload;
        "#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import { rawMarker } from "@rawholder";
            export default function Home() {
              return rawMarker;
            }
        "#,
    )
    .unwrap();

    let tsconfig_paths = BTreeMap::from([(
        "@rawholder".to_string(),
        vec![ws_root
            .join("lib/shared/raw-holder-c.ts")
            .to_string_lossy()
            .into_owned()],
    )]);
    let mut input = base_input(&project, esbuild, unrelated_exclude());
    input.tsconfig_paths = tsconfig_paths;
    let out = bundle(input).expect(
        "a sibling file's own terminal ?raw import reached via a CONCRETE \
         alias must build green under an UNRELATED non-empty bundle.exclude",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("RAW_C_PAYLOAD_MARKER"),
        "the ?raw target's inlined text must reach the bundle: {}",
        truncate(&body)
    );
    assert!(
        !body.contains("import payload from"),
        "the ?raw import declaration itself must not survive verbatim: {}",
        truncate(&body)
    );
}

#[test]
fn j_sibling_module_worker_wildcard_alias_expands_under_unrelated_exclude() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ws_root, project) = write_workspace(tmp.path());

    fs::create_dir_all(ws_root.join("lib/shared")).unwrap();
    fs::write(
        ws_root.join("lib/shared/worker-w.worker.ts"),
        "self.postMessage('WORKER_W_MARKER');\n",
    )
    .unwrap();
    fs::write(
        ws_root.join("lib/shared/worker-holder-w.ts"),
        r#"
            export function makeWorker() {
              return new Worker(new URL('./worker-w.worker.ts', import.meta.url), { type: 'module' });
            }
        "#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import { makeWorker } from "@shared/worker-holder-w";
            export default function Home() {
              return typeof makeWorker;
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
        "issue #1724: a sibling file's own module-worker macro reached via a \
         WILDCARD alias must be rewritten under an UNRELATED non-empty \
         bundle.exclude",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        !body.contains("new URL(\"./worker-w.worker.ts\""),
        "the sibling's own module-worker macro must be rewritten to a \
         stable companion URL, not left pointing at the raw relative \
         source path: {}",
        truncate(&body)
    );
    assert!(
        body.contains(".js?v="),
        "the rewritten module-worker specifier must carry the stable \
         `<slug>.js?v=<hash>` companion filename: {}",
        truncate(&body)
    );
}

#[test]
fn k_sibling_module_worker_concrete_alias_expands_under_unrelated_exclude() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_sibling_mirror_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (ws_root, project) = write_workspace(tmp.path());

    fs::create_dir_all(ws_root.join("lib/shared")).unwrap();
    fs::write(
        ws_root.join("lib/shared/worker-c.worker.ts"),
        "self.postMessage('WORKER_C_MARKER');\n",
    )
    .unwrap();
    fs::write(
        ws_root.join("lib/shared/worker-holder-c.ts"),
        r#"
            export function makeWorker() {
              return new Worker(new URL('./worker-c.worker.ts', import.meta.url), { type: 'module' });
            }
        "#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r#"
            import { makeWorker } from "@workerholder";
            export default function Home() {
              return typeof makeWorker;
            }
        "#,
    )
    .unwrap();

    let tsconfig_paths = BTreeMap::from([(
        "@workerholder".to_string(),
        vec![ws_root
            .join("lib/shared/worker-holder-c.ts")
            .to_string_lossy()
            .into_owned()],
    )]);
    let mut input = base_input(&project, esbuild, unrelated_exclude());
    input.tsconfig_paths = tsconfig_paths;
    let out = bundle(input).expect(
        "issue #1724: a sibling file's own module-worker macro reached via a \
         CONCRETE alias must be rewritten under an UNRELATED non-empty \
         bundle.exclude",
    );
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        !body.contains("new URL(\"./worker-c.worker.ts\""),
        "the sibling's own module-worker macro must be rewritten to a \
         stable companion URL, not left pointing at the raw relative \
         source path: {}",
        truncate(&body)
    );
    assert!(
        body.contains(".js?v="),
        "the rewritten module-worker specifier must carry the stable \
         `<slug>.js?v=<hash>` companion filename: {}",
        truncate(&body)
    );
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
