//! Repro fixture + baseline harness for issue #3133 (epic #3135, sub-issue
//! #3138 / Track B0) — "Out-of-root content collection seeds whole source
//! dir, triggering full node_modules staging walk".
//!
//! ## What this proves today (pre-fix)
//!
//! Runs a real `zfb build` over `tests/fixtures/collection-seeds-3133/` in
//! two variants — `with-collection` (a `componentDocs` collection at
//! `../../packages/ui/src/components`, `include: ["**/*.mdx"]`,
//! `allowOutsideRoot: true`, mirroring the issue's repro exactly) and
//! `control` (identical workspace, same `apps/site` cwd, but the config has
//! no `collections` key at all) — and prints the wall-clock delta between
//! them. Both builds must succeed either way; #3133 is a *performance* bug
//! (dev never becomes ready / build OOMs on a large real dependency tree),
//! not a correctness bug, and this fixture's synthetic `node_modules` tree
//! is deliberately tiny (a handful of files) so it stays fast and commit-
//! able — it cannot itself reproduce a multi-minute hang. What it DOES
//! reproduce structurally is the exact shape the root cause needs: a
//! `.tsx` sibling of the included `.mdx` that imports a workspace package
//! (`button.tsx`), a second sibling `.tsx` that NO `.mdx` imports and that
//! also imports a workspace package (`orphan.tsx` — proves code merely
//! sitting next to the `.mdx` must not matter), and a pnpm-private
//! transitive dependency (`leftpad-priv`) reachable along >= 3 distinct
//! *logical* `node_modules/leftpad-priv` paths (`packages/ui`,
//! `packages/shared-utils`, `packages/shared-icons`) that all resolve to
//! ONE physical pnpm-store directory — the exact multiplier
//! `extend_node_modules_dependency_staging`'s logical-root-keyed `visited`
//! set re-scans today (bundler.rs, see the code-map in
//! `_temp-resource/3135-provenance-guard-and-collection-seeds/code-map.md`).
//!
//! ## The staging-stats hook (owned by sibling issue #3139, NOT this file)
//!
//! #3139 (parallel Track B1, working in `crates/zfb-build/src/bundler.rs`)
//! adds a `ZFB_STAGING_STATS=1`-gated stderr line to
//! `extend_node_modules_dependency_staging`:
//!
//! ```text
//! [zfb-staging-stats] physical_scans=N logical_visits=M workspace_staging_activated=bool
//! ```
//!
//! This file does NOT touch `bundler.rs` (worktree coordination rule) and
//! the hook does not exist yet on this branch. Both build invocations below
//! set `ZFB_STAGING_STATS=1` and scan captured stderr for that line: when
//! present it's parsed and printed (a future revision, once #3139 merges,
//! can promote the printed counts into hard assertions — e.g.
//! `logical_visits >= 3` and `physical_scans == 1` after B1's memoization,
//! `workspace_staging_activated == false` for `control`); when absent
//! (this tree, today) the harness prints a note and does not fail — the
//! manager re-runs this file after both Wave-1 topics merge.
//!
//! ## Level / tier
//!
//! Level 4 (real `zfb build` process). Registered in nextest's
//! `e2e-heavy-unlocked` build-only test-group (`.config/nextest.toml`) —
//! see `crates/CLAUDE.md`'s heavy-binary manifest. Not `#[ignore]`d: the
//! embedded V8 / esbuild toolchain built into the `zfb` binary itself means
//! no external esbuild slot is needed (unlike the `locate_esbuild()`-gated
//! tests), matching `end_to_end_basic_blog_build`'s self-skip convention
//! for the one exceptional environment (no `embed_v8` feature / stripped
//! CI image) via [`is_known_skip`].

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use zfb_test_utils::zfb_binary;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("collection-seeds-3133")
}

fn workspace_template_dir() -> PathBuf {
    fixture_root().join("workspace")
}

fn pnpm_store_template_dir() -> PathBuf {
    fixture_root()
        .join("pnpm-store-template")
        .join("leftpad-priv")
}

/// `true` when the non-zero build is a known-skip (no embedded V8 / no
/// esbuild / no tailwindcss-v4 binary) — same convention as
/// `end_to_end_basic_blog_build.rs`'s `is_known_skip`.
fn is_known_skip(combined: &str) -> bool {
    combined.contains("embed_v8")
        || combined.contains("no esbuild")
        || combined.contains("no tailwind")
        || (combined.contains("tailwindcss") && combined.contains("not found"))
}

/// Recursive directory copy (files only; creates target subdirs as needed).
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(original: &Path, link: &Path) {
    std::os::unix::fs::symlink(original, link)
        .unwrap_or_else(|e| panic!("symlink {} -> {}: {e}", link.display(), original.display()));
}

/// One fixture variant: `with-collection` (real #3133 repro shape) or
/// `control` (identical workspace minus the collection).
enum Variant {
    WithCollection,
    Control,
}

impl Variant {
    fn config_source_name(&self) -> &'static str {
        match self {
            Variant::WithCollection => "zfb.config.with-collection.json",
            Variant::Control => "zfb.config.control.json",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Variant::WithCollection => "with-collection",
            Variant::Control => "control",
        }
    }
}

/// Materialises one fixture variant into a fresh tempdir: copies the
/// checked-in workspace template, selects the variant's `zfb.config.json`,
/// and lays down the synthetic pnpm-store node_modules tree (workspace
/// package symlinks + >= 3 logical paths to the same physical
/// `leftpad-priv` directory) that both variants share. Returns
/// `(tempdir_guard, apps/site project root)`.
#[cfg(unix)]
fn materialise_fixture(variant: &Variant) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let workspace_root = tmp.path().to_path_buf();
    copy_dir(&workspace_template_dir(), &workspace_root).expect("copy fixture workspace");

    let site_root = workspace_root.join("apps/site");
    let both_configs = [
        Variant::WithCollection.config_source_name(),
        Variant::Control.config_source_name(),
    ];
    for name in both_configs {
        let candidate = site_root.join(name);
        if name == variant.config_source_name() {
            fs::rename(&candidate, site_root.join("zfb.config.json"))
                .expect("select variant zfb.config.json");
        } else {
            fs::remove_file(&candidate).expect("remove unused variant config");
        }
    }

    // ---- synthetic pnpm-store node_modules tree ----
    // Physical package, once, at the workspace-root virtual store — mirrors
    // real pnpm's `node_modules/.pnpm/<name>@<version>/node_modules/<name>`
    // layout (see `bundler_exact_match_resolution.rs`'s
    // `write_bare_package_in`/virtual-store precedent).
    let physical_leftpad =
        workspace_root.join("node_modules/.pnpm/leftpad-priv@1.0.0/node_modules/leftpad-priv");
    copy_dir(&pnpm_store_template_dir(), &physical_leftpad).expect("copy leftpad-priv into store");

    // Logical path 1: `packages/ui` depends on `leftpad-priv` directly
    // (imported by button.tsx) and on workspace package `shared-utils`.
    let ui_node_modules = workspace_root.join("packages/ui/node_modules");
    fs::create_dir_all(&ui_node_modules).unwrap();
    symlink(&physical_leftpad, &ui_node_modules.join("leftpad-priv"));
    symlink(
        &workspace_root.join("packages/shared-utils"),
        &ui_node_modules.join("shared-utils"),
    );

    // Logical path 2: `packages/shared-utils` depends on `leftpad-priv`
    // directly and on workspace package `shared-icons`.
    let shared_utils_node_modules = workspace_root.join("packages/shared-utils/node_modules");
    fs::create_dir_all(&shared_utils_node_modules).unwrap();
    symlink(
        &physical_leftpad,
        &shared_utils_node_modules.join("leftpad-priv"),
    );
    symlink(
        &workspace_root.join("packages/shared-icons"),
        &shared_utils_node_modules.join("shared-icons"),
    );

    // Logical path 3: `packages/shared-icons` depends on `leftpad-priv`
    // directly.
    let shared_icons_node_modules = workspace_root.join("packages/shared-icons/node_modules");
    fs::create_dir_all(&shared_icons_node_modules).unwrap();
    symlink(
        &physical_leftpad,
        &shared_icons_node_modules.join("leftpad-priv"),
    );

    (tmp, site_root)
}

struct RunOutcome {
    success: bool,
    combined: String,
    elapsed_ms: u128,
    staging_stats_line: Option<String>,
}

fn run_zfb_build(project_root: &Path) -> RunOutcome {
    let start = Instant::now();
    let output = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(project_root)
        .env("ZFB_STAGING_STATS", "1")
        .output()
        .expect("spawn zfb binary");
    let elapsed_ms = start.elapsed().as_millis();

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let combined = format!("{stdout}{stderr}");
    let staging_stats_line = stderr
        .lines()
        .chain(stdout.lines())
        .find(|line| line.contains("[zfb-staging-stats]"))
        .map(str::to_string);

    RunOutcome {
        success: output.status.success(),
        combined,
        elapsed_ms,
        staging_stats_line,
    }
}

/// Baseline pre-fix measurement (issue #3138 / Track B0): runs real
/// `zfb build` over both fixture variants, prints wall-clock time and the
/// staging-stats line (when present) for each, and asserts both builds
/// succeed. See the file header for what is (and is not) proved by this
/// synthetic-scale fixture, and for the staging-stats hook's ownership.
#[cfg(unix)]
#[test]
fn zfb_build_succeeds_for_both_fixture_variants_and_prints_baseline() {
    let mut outcomes = Vec::new();
    for variant in [Variant::WithCollection, Variant::Control] {
        let (_tmp, project_root) = materialise_fixture(&variant);
        let outcome = run_zfb_build(&project_root);

        if !outcome.success {
            if is_known_skip(&outcome.combined) {
                eprintln!(
                    "[collection_seed_3133_baseline] zfb build exited non-zero with a \
                     known-skip indicator (V8/esbuild/tailwind unavailable); skipping test."
                );
                return;
            }
            panic!(
                "zfb build must succeed for the {} fixture variant.\n--- combined output ---\n{}",
                variant.label(),
                outcome.combined
            );
        }

        eprintln!(
            "[collection-seeds-3133 baseline] variant={} elapsed_ms={} staging_stats={}",
            variant.label(),
            outcome.elapsed_ms,
            outcome
                .staging_stats_line
                .as_deref()
                .unwrap_or("<absent: ZFB_STAGING_STATS hook not present on this tree — #3139>"),
        );
        outcomes.push((variant.label(), outcome));
    }

    assert_eq!(outcomes.len(), 2, "expected exactly 2 baseline runs");
}
