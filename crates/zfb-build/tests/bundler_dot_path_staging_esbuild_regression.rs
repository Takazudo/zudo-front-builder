//! Issue #1841 (epic #1836) — real-esbuild regression + stage-escape-audit
//! confirmation for the first-party dot-path staging allowlist (#1840).
//!
//! `bundler_dot_path_staging.rs` proves the staging MATERIALISE pass writes
//! the allowlisted `.zudo-doc/routes-src/**` + `.zfb/*.json` spellings into
//! the shadow tree, but every one of its fixtures drives
//! `bundle_with_session` with `mock_subprocess_output` (see its own header
//! comment) — the stage-escape audit (#1706, wired in `bundler.rs` wherever
//! `workspace_rel.is_some()`) never runs under a mock, since `metafile_path`
//! stays `None` when esbuild is never actually invoked. This file is the REAL
//! esbuild confirmation the mock suite explicitly deferred: it proves esbuild
//! genuinely resolves both allowlisted dot-path spellings out of the shadow
//! (not the live tree) and that the stage-escape audit passes them.
//!
//! ## Fixture shape
//!
//! A 2-member pnpm workspace (`.` + `sub-packages/*`) with `sub-packages/host`
//! as the zfb project — the widened-stage condition
//! (`workspace_rel.is_some()`) that arms the stage-escape audit
//! UNCONDITIONALLY, independent of `bundle.exclude` (see the audit's own
//! call-site comment in `bundler.rs`). This mirrors zudo-doc 4.x's real
//! deployment shape: this very repo's `docs/` package is a `zudo-doc`
//! consumer with its own `tsconfig.json` carrying a
//! `"#doc-history-meta": [".zfb/doc-history-meta.json"]` alias (copied
//! verbatim below).
//!
//! `pages/index.tsx` reaches the generated route module through a tsconfig
//! alias (`#generated-route`) rather than a plain relative import —
//! `KNOWN_FIRST_PARTY_STAGING_DIRS`'s own doc comment names the "dual-target
//! tsconfig fallback" as the pre-#1840 escape mechanism, and only an alias
//! target gets that shadow-first/real-fallback dual-target rebase
//! (`rebase_tsconfig_paths_to_shadow`); a plain relative import out of an
//! unstaged `.zudo-doc/` would simply fail to resolve, not silently escape.
//! The generated route module itself imports `.zfb/doc-history-meta.json`
//! through the same alias shape zudo-doc's real generated source uses today.
//!
//! ## Red-first evidence
//!
//! Verified by temporarily changing both `KNOWN_FIRST_PARTY_STAGING_DIRS` and
//! `KNOWN_FIRST_PARTY_STAGING_JSON_DIRS` (the two allowlist consts in
//! `bundler.rs`, just above this test's target) to `&[]`, simulating a
//! pre-#1840 tree (no staged spelling can ever be materialised) with
//! `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test
//! bundler_dot_path_staging_esbuild_regression -- --ignored`. This reproduced
//! the exact case-4 stage-escape failure the audit exists to catch (paths
//! shortened for readability — the real message names the full absolute
//! tempdir path esbuild's `..`-climbing metafile key resolves to):
//!
//! ```text
//! panicked: ...: bundler: SSR work-mirror stage-escape audit failed
//!
//! Caused by:
//!     zfb bundler: stage-escape audit — the following metafile input(s)
//!     escaped their stage: ../../.../<tmp>/sub-packages/host/.zfb/doc-history-meta.json
//!     (first-party input resolved outside every stage root, no staged
//!     spelling), ../../.../<tmp>/sub-packages/host/.zudo-doc/routes-src/generated-route.tsx
//!     (first-party input resolved outside every stage root, no staged
//!     spelling)
//! ```
//!
//! Restoring the two consts made the test pass again (GREEN).
//!
//! ## Issue #3004/#3021 (epic #3019) twin
//!
//! `real_esbuild_resolves_injected_route_entrypoint_staged_spelling_and_passes_stage_escape_audit`
//! below reuses this file's exact fixture shape (same 2-member pnpm
//! workspace, same alias-reached generated route module) but roots the
//! generated route + its relative helper under a hidden dir that is
//! deliberately NOT in `KNOWN_FIRST_PARTY_STAGING_DIRS`
//! (`.example-package/routes-src/`, mirroring the epic's own
//! `injectRoute`-registered-plugin repro). The staged spelling can only come
//! from `BundlerInput::injected_route_entrypoints` (#3004) — never from the
//! dot-path allowlist above. Red-first evidence recorded on that test's own
//! doc comment.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{bundle_with_session, BundleMode, BundlerInput, ShadowSession};
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
        "packages:\n  - '.'\n  - 'sub-packages/*'\n",
    )
    .unwrap();
    let project = tmp_root.join("sub-packages/host");
    scaffold_project_dirs(&project);
    (tmp_root.to_path_buf(), project)
}

/// The zudo-doc 4.x dot-path fixture: a generated route module under the
/// hidden, gitignored `.zudo-doc/routes-src/` allowlist dir, itself importing
/// the docHistory meta file under the hidden, gitignored `.zfb/` allowlist
/// dir — both reached through a tsconfig alias.
fn write_dot_path_fixture(project: &Path) {
    fs::create_dir_all(project.join(".zudo-doc/routes-src")).unwrap();
    fs::write(
        project.join(".zudo-doc/routes-src/generated-route.tsx"),
        r##"
            import docHistory from "#doc-history-meta";
            export default function GeneratedRoute() {
              return "GENERATED_ROUTE_MARKER:" + JSON.stringify(docHistory);
            }
        "##,
    )
    .unwrap();
    fs::create_dir_all(project.join(".zfb")).unwrap();
    fs::write(
        project.join(".zfb/doc-history-meta.json"),
        r#"{ "history": ["DOC_HISTORY_META_MARKER"] }"#,
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r##"
            import GeneratedRoute from "#generated-route";
            export default function Home() {
              return GeneratedRoute();
            }
        "##,
    )
    .unwrap();
    // Real-world shape — issue #1840's whole premise: both dirs are
    // conventionally gitignored, so every generic staging walker (which
    // honors `.gitignore` and prunes hidden dirs) would otherwise skip them
    // entirely, and no staged spelling would ever exist.
    fs::write(project.join(".gitignore"), ".zudo-doc/\n.zfb/\n").unwrap();
}

/// Shared `BundlerInput` defaults, mirroring
/// `bundler_sibling_mirror_esbuild_regression.rs`'s `base_input`, plus the
/// two dot-path tsconfig aliases.
fn base_input(project: &Path, esbuild: PathBuf) -> BundlerInput {
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
    input.tsconfig_paths = BTreeMap::from([
        (
            "#generated-route".to_string(),
            vec![project
                .join(".zudo-doc/routes-src/generated-route.tsx")
                .to_string_lossy()
                .into_owned()],
        ),
        (
            "#doc-history-meta".to_string(),
            vec![project
                .join(".zfb/doc-history-meta.json")
                .to_string_lossy()
                .into_owned()],
        ),
    ]);
    input
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

/// Metafile input KEYS (relative to esbuild's cwd — the project's shadow
/// mirror, since `run_esbuild` sets `cmd.current_dir(shadow)`). A genuinely
/// staged file's key mirrors its project-relative path; a live-tree escape
/// instead records a `..`-climbing or absolute key.
fn metafile_input_keys(shadow: &Path) -> Vec<String> {
    let bytes = fs::read(shadow.join(".zfb-metafile.json")).expect("read metafile");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("parse metafile json");
    value["inputs"]
        .as_object()
        .expect("metafile inputs object")
        .keys()
        .cloned()
        .collect()
}

#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_dot_path_staging_esbuild_regression -- --ignored"]
fn real_esbuild_resolves_dot_path_staged_spellings_and_passes_stage_escape_audit() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_dot_path_staging_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (_ws_root, project) = write_workspace(tmp.path());
    write_dot_path_fixture(&project);

    let mut session = ShadowSession::new(&project).expect("shadow session");
    let input = base_input(&project, esbuild);
    let out = bundle_with_session(input, Some(&mut session)).expect(
        "issue #1840/#1841: a workspace project's .zudo-doc/routes-src route \
         module and .zfb/*.json docHistory meta file, both reached through a \
         tsconfig alias, must build green AND pass the stage-escape audit \
         (workspace_rel.is_some() arms it unconditionally)",
    );

    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    for marker in ["GENERATED_ROUTE_MARKER", "DOC_HISTORY_META_MARKER"] {
        assert!(
            body.contains(marker),
            "the dot-path staged content must reach the bundle: {}",
            truncate(&body)
        );
    }

    // Review-mandated: prove BOTH dot-path inputs actually went through
    // esbuild as STAGED spellings, not merely that nothing exploded — a
    // build can go green via the live-tree dual-target fallback too, which
    // is exactly the case-4 escape the audit above independently rejects.
    let shadow = fs::canonicalize(session.shadow_root())
        .expect("canonicalize persistent shadow root")
        .join("sub-packages/host");
    // EXACT match, not a substring check (codex review finding): esbuild's
    // cwd is `shadow` (the project's own mirror root), so a genuinely staged
    // input's key is byte-identical to its project-relative path. A
    // `.contains()` check would also accept a live-tree escape, whose key is
    // a `..`-climbing or absolute path that still CONTAINS this same
    // trailing substring (e.g. `../../../<tmp>/host/.zfb/doc-history-meta.json`)
    // — exactly the case-4 shape the audit above independently rejects, and
    // exactly what this assertion must independently distinguish from too.
    let keys = metafile_input_keys(&shadow);
    for expected in [
        ".zudo-doc/routes-src/generated-route.tsx",
        ".zfb/doc-history-meta.json",
    ] {
        assert!(
            keys.iter().any(|k| k == expected),
            "metafile inputs must record the EXACT staged spelling {expected}; got {keys:?}"
        );
    }
}

/// Issue #3004/#3021 fixture: an injected route's project-local entrypoint
/// under a hidden dir that is deliberately NOT in
/// `KNOWN_FIRST_PARTY_STAGING_DIRS` (`.example-package/routes-src/`, the
/// epic's own repro shape), importing a relative helper, reached from
/// `pages/index.tsx` through a tsconfig alias — the SAME dual-target
/// alias-rebase escape mechanism `write_dot_path_fixture` above uses, so the
/// only difference from that fixture is which allowlisting mechanism (const
/// vs. `injected_route_entrypoints`) is expected to produce the staged
/// spelling.
fn write_injected_entrypoint_fixture(project: &Path) -> PathBuf {
    fs::create_dir_all(project.join(".example-package/routes-src")).unwrap();
    let entrypoint = project.join(".example-package/routes-src/generated-route.tsx");
    fs::write(
        &entrypoint,
        r##"
            import { helper } from "./helper.tsx";
            export default function InjectedRoute() {
              return "INJECTED_ROUTE_MARKER:" + helper;
            }
        "##,
    )
    .unwrap();
    fs::write(
        project.join(".example-package/routes-src/helper.tsx"),
        "export const helper = 'INJECTED_HELPER_MARKER';\n",
    )
    .unwrap();
    fs::write(
        project.join("pages/index.tsx"),
        r##"
            import InjectedRoute from "#injected-route";
            export default function Home() {
              return InjectedRoute();
            }
        "##,
    )
    .unwrap();
    // Same real-world shape as issue #1840's fixture: a conventionally
    // gitignored dir the generic staging walker would otherwise skip.
    fs::write(project.join(".gitignore"), ".example-package/\n").unwrap();
    entrypoint
}

/// Issue #3004/#3021 (epic #3019): real-esbuild confirmation that
/// `BundlerInput::injected_route_entrypoints` produces the same staged
/// spelling + stage-escape-audit pass as the dot-path allowlist above, for a
/// hidden dir the allowlist does not cover.
///
/// ## Red-first evidence
///
/// Verified by temporarily dropping the `input.injected_route_entrypoints =
/// vec![entrypoint.clone()];` line below (leaving the field at its
/// `BundlerInput::for_project` default, `Vec::new()`) and re-running
/// `ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test
/// bundler_dot_path_staging_esbuild_regression -- --ignored --exact
/// real_esbuild_resolves_injected_route_entrypoint_staged_spelling_and_passes_stage_escape_audit`.
/// This reproduced the same case-4 stage-escape shape the sibling test's own
/// header documents — both the entrypoint and its relative helper resolve via
/// the alias's live-tree dual-target fallback and are rejected as escapes
/// (paths shortened for readability; the real message names the full
/// absolute tempdir path esbuild's `..`-climbing metafile key resolves to,
/// and case-4 also prints the workspace-sibling remediation paragraph, which
/// does not apply here — omitted below):
///
/// ```text
/// panicked: ...: bundler: SSR work-mirror stage-escape audit failed
///
/// Caused by:
///     zfb bundler: stage-escape audit — the following metafile input(s)
///     escaped their stage: ../../.../<tmp>/sub-packages/host/.example-package/routes-src/helper.tsx
///     (first-party input resolved outside every stage root, no staged
///     spelling), ../../.../<tmp>/sub-packages/host/.example-package/routes-src/generated-route.tsx
///     (first-party input resolved outside every stage root, no staged
///     spelling)
/// ```
///
/// Restoring the `injected_route_entrypoints` assignment made the test pass
/// again (GREEN).
#[test]
#[ignore = "env-gate: esbuild — ZFB_ESBUILD_BIN=<abs path> cargo test -p zfb-build --test bundler_dot_path_staging_esbuild_regression -- --ignored"]
fn real_esbuild_resolves_injected_route_entrypoint_staged_spelling_and_passes_stage_escape_audit() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_dot_path_staging_esbuild_regression] no esbuild binary; skipping.");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let (_ws_root, project) = write_workspace(tmp.path());
    let entrypoint = write_injected_entrypoint_fixture(&project);

    let mut session = ShadowSession::new(&project).expect("shadow session");
    let mut input = base_input(&project, esbuild);
    input.tsconfig_paths.insert(
        "#injected-route".to_string(),
        vec![entrypoint.to_string_lossy().into_owned()],
    );
    // The field under test: without this, the entrypoint has no staged
    // spelling (see the red-first evidence above).
    input.injected_route_entrypoints = vec![entrypoint.clone()];

    let out = bundle_with_session(input, Some(&mut session)).expect(
        "issue #3004/#3021: an injected route's project-local entrypoint under a \
         hidden dir OUTSIDE the KNOWN_FIRST_PARTY_STAGING_DIRS allowlist, reached \
         through a tsconfig alias, must build green AND pass the stage-escape audit \
         once its absolute path is threaded through \
         BundlerInput::injected_route_entrypoints",
    );

    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    for marker in ["INJECTED_ROUTE_MARKER", "INJECTED_HELPER_MARKER"] {
        assert!(
            body.contains(marker),
            "the injected-entrypoint staged content must reach the bundle: {}",
            truncate(&body)
        );
    }

    // Same exact-match rationale as the sibling test above (codex review
    // finding): a `.contains()` check would also accept a live-tree escape.
    let shadow = fs::canonicalize(session.shadow_root())
        .expect("canonicalize persistent shadow root")
        .join("sub-packages/host");
    let keys = metafile_input_keys(&shadow);
    for expected in [
        ".example-package/routes-src/generated-route.tsx",
        ".example-package/routes-src/helper.tsx",
    ] {
        assert!(
            keys.iter().any(|k| k == expected),
            "metafile inputs must record the EXACT staged spelling {expected}; got {keys:?}"
        );
    }
}
