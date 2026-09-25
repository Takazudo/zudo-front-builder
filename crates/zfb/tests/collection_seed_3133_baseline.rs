//! Repro fixture + staging-stats harness for issue #3133 (epic #3135,
//! sub-issues #3138 / Track B0, #3141 / Track D, #3142 / Track B2, and #3143
//! / Track B3) — "Out-of-root content collection seeds whole source dir,
//! triggering full node_modules staging walk".
//!
//! ## Two confirmations
//!
//! - **`zfb build`** (below): asserts the `[zfb-staging-stats]` line for
//!   both fixture variants — the ORIGINAL #3138/#3141 baseline.
//! - **`zfb dev`** (#3143, the "Dev confirmation" section further down):
//!   asserts a real `zfb dev --port 0` becomes ready within a deadline derived from a
//!   measured `control` distribution, AND that the same staging-stats line
//!   holds for a dev boot, not just a build. This binary now spawns `zfb
//!   dev`, so it moved from nextest's `e2e-heavy-unlocked` group into
//!   `e2e-heavy-locked` and adopts issue #1339's cross-binary flock — see
//!   `crates/CLAUDE.md`'s manifest.
//!
//! ## What this proves (`zfb build`)
//!
//! Runs a real `zfb build` over `tests/fixtures/collection-seeds-3133/` in
//! two variants — `with-collection` (a `componentDocs` collection at
//! `../../packages/ui/src/components`, `include: ["**/*.mdx"]`,
//! `allowOutsideRoot: true`, mirroring the issue's repro exactly) and
//! `control` (identical workspace, same `apps/site` cwd, but the config has
//! no `collections` key at all) — and asserts the `[zfb-staging-stats]`
//! line (#3139's `ZFB_STAGING_STATS=1` hook in
//! `extend_node_modules_dependency_staging`, `crates/zfb-build/src/bundler.rs`)
//! for each:
//!
//! - `control`: no package is closure-walked at all and workspace staging
//!   never activates — the site's own pages import nothing.
//! - `with-collection`: identical to `control` since #3142. The collection's
//!   `include: ["**/*.mdx"]` drops both sibling `.tsx` files
//!   (`button.tsx`, `orphan.tsx`) from the shadow, and the seed walk applies
//!   the same filter, so they no longer seed the closure. Before #3142 they
//!   did: they reached a workspace package, flipped staging on, and dragged
//!   every deferred live dependency (preact, preact-render-to-string, the
//!   zfb runtime, hono) plus the workspace packages' pnpm-private closure
//!   through the import-parsing walk (`7/9/true`, measured in #3141 — the
//!   RED state of this assertion). `button.mdx`'s own `import` is dropped
//!   by the MDX compiler, so it seeds nothing either.
//!
//! ## Why the fixture needs a real `apps/site/node_modules` + `tsconfig.json`
//!
//! Diagnosed in #3141. #3138's first cut had NO `apps/site/node_modules` and
//! no `tsconfig.json`, and never reproduced the flip:
//!
//! 1. Without a project-local `node_modules`, `commands::bundler_input`
//!    falls back to the binary-embedded vendor tree and sets
//!    `node_modules_preserve_symlinks = true`, which disables BOTH the
//!    canonical-package resolution (`resolve_from_canonical_package`) and
//!    the workspace physical fallback in `extend_node_modules_dependency_staging`,
//!    so a workspace package's pnpm-private siblings are never reached.
//! 2. With a project-local `node_modules` but an EMPTY `tsconfig` `paths`
//!    map, `esbuild_will_preserve_symlinks` is true, so pnpm-private
//!    siblings are only DEFERRED (`deferred_physical_dependencies`) until
//!    staging flips — and out-of-root staged dirs (`packages/ui/node_modules/…`)
//!    are invisible to the flip predicate (it only inspects `staging_dirs`
//!    entries under `project_root`). Nothing flips, nothing is walked.
//!
//! A non-empty `paths` map (`apps/site/tsconfig.json`, the same gate
//! `bundler_staging_scan_memo.rs` documents) makes esbuild — and therefore
//! the closure walk — resolve through canonical package dirs, which is what
//! follows a store dir's private siblings and lands the workspace-package
//! ALIASES (`…/shared-utils/node_modules/shared-icons`) in
//! `staging_alias_dirs`, where the flip predicate does see them. A second,
//! independent real-world trigger — any in-root source importing a
//! workspace package — also flips staging (measured in #3141's decision
//! record) but would flip the `control` variant too, so the fixture uses
//! the `tsconfig` gate to keep the control at zero.
//!
//! Also learned there: the collection root was joined onto the project root
//! UNNORMALISED (`<ws>/apps/site/../../packages/ui/src/components`), so the
//! out-of-root seeds passed `seed.starts_with(project_root)` lexically and
//! resolved their bare imports from their own physical location (walking up
//! through the `..` into `packages/ui/node_modules/`). #3142 normalises the
//! root and resolves a matched collection file from its shadow location
//! `<site>/content/<name>/<rel>` instead — where esbuild meets it.
//!
//! The tsconfig gate keeps this fixture sensitive: a regression that lets
//! the non-included `.tsx` files seed again flips `with-collection` back to
//! a non-zero, staging-activated line.
//!
//! ## node_modules layout (built at setup time, never committed)
//!
//! `materialise_fixture` lays the tree down in a tempdir per variant:
//!
//! - `<ws>/node_modules/.pnpm/leftpad-priv@1.0.0/node_modules/leftpad-priv/`
//!   — the ONE physical `leftpad-priv`, linked from `packages/ui`,
//!   `packages/shared-utils` and `packages/shared-icons` (3 logical paths).
//! - `<ws>/node_modules/.pnpm/@takazudo+zfb-runtime@embedded/node_modules/`
//!   — `@takazudo/zfb-runtime` + `@takazudo/zfb` copied from the binary's
//!   own vendor snapshot (`ZFB_VENDOR_DIR`, the same bytes the embedded
//!   fallback would serve) so they canonicalise INSIDE a `node_modules`
//!   like a real install, plus a `hono` link to this monorepo's installed
//!   copy. The runtime's server router imports `hono` and
//!   `@takazudo/zfb/content`, and after the flip every dependency must be
//!   present in the isolated staged view — a symlink to the repo's
//!   `packages/zfb-runtime` workspace package would canonicalise OUTSIDE
//!   any `node_modules` and be rejected by `workspace_package_source_is_eligible`.
//! - `<ws>/apps/site/node_modules/` — `preact`, `preact-render-to-string`
//!   (this monorepo's installed store copies, found by prefix so a version
//!   bump cannot strand the test), `@takazudo/{zfb,zfb-runtime}` (the store
//!   copies above), and the `shared-utils` / `ui` workspace links
//!   `apps/site/package.json` declares.
//!
//! ## Level / tier
//!
//! Level 4 (real `zfb build` / `zfb dev` process). Registered in nextest's
//! `e2e-heavy-locked` test-group (`.config/nextest.toml`) since #3143 added
//! the dev test — see `crates/CLAUDE.md`'s heavy-binary manifest. The `zfb
//! build` test is not `#[ignore]`d: the embedded V8 / esbuild toolchain
//! built into the `zfb` binary itself means no external esbuild slot is
//! needed (unlike the `locate_esbuild()`-gated tests), matching
//! `end_to_end_basic_blog_build`'s self-skip convention for the one
//! exceptional environment (no `embed_v8` feature / stripped CI image) via
//! [`is_known_skip`]. The `zfb dev` test self-skips the same way it does in
//! `dev_out_of_root_collection_e2e.rs`: via [`locate_esbuild`] returning
//! `None`, not an `#[ignore]` attribute — `zfb dev`'s client bundling always
//! shells out to esbuild, unlike `zfb build`'s embedded-V8-only SSR path.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use zfb_test_utils::zfb_binary;

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::sync::LazyLock;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use zfb_test_utils::{locate_esbuild, CrossBinaryE2eLock};

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

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ dir")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

/// The binary-embedded vendor snapshot (`crates/zfb/build.rs` exports it as
/// `ZFB_VENDOR_DIR`; `render_pipeline.rs` embeds it with `include_dir!`).
/// Its `@takazudo/{zfb,zfb-runtime}` entries are `package.json` + `src`
/// only, so copying them is cheap and version-consistent with the binary
/// under test.
fn vendor_dir() -> PathBuf {
    PathBuf::from(env!("ZFB_VENDOR_DIR"))
}

/// The first `node_modules/.pnpm/<prefix>*/node_modules/<package_name>`
/// entry of this monorepo's own install — same helper shape as
/// `client_bundling_cross_pipeline.rs`, so a dependency bump never strands
/// this test on a pinned version string.
fn find_pnpm_store_package(pnpm_dir: &Path, prefix: &str, package_name: &str) -> PathBuf {
    let mut candidates: Vec<PathBuf> = fs::read_dir(pnpm_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", pnpm_dir.display()))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect();
    candidates.sort();
    let chosen = candidates.into_iter().next().unwrap_or_else(|| {
        panic!(
            "no node_modules/.pnpm entry starting with {prefix:?} under {} — run `pnpm install` \
             at the repo root",
            pnpm_dir.display()
        )
    });
    chosen.join("node_modules").join(package_name)
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

    /// The staging stats each variant must produce. Both are `0/0/false`
    /// since #3142: the collection contributes no seeds under `**/*.mdx`.
    ///
    /// Before #3142 `with-collection` measured `7/9/true` (#3141): the
    /// non-included `button.tsx` / `orphan.tsx` seeded
    /// `packages/ui/node_modules/shared-utils`, its pnpm-private
    /// `leftpad-priv` / `shared-icons` aliases flipped workspace staging on,
    /// and the deferred live dependencies (`@takazudo/zfb-runtime`, `hono`,
    /// `preact`, `preact-render-to-string`, `packages/ui`'s `leftpad-priv`)
    /// were drained. See the fixture README for the full derivation.
    fn expected_stats(&self) -> StagingStats {
        match self {
            Variant::WithCollection | Variant::Control => StagingStats {
                physical_scans: 0,
                logical_visits: 0,
                workspace_staging_activated: false,
            },
        }
    }
}

/// Materialises one fixture variant into a fresh tempdir: copies the
/// checked-in workspace template, selects the variant's `zfb.config.json`,
/// and lays down the synthetic pnpm-store node_modules tree (workspace
/// package symlinks + 3 logical paths to the same physical `leftpad-priv`
/// directory + the site's own real `node_modules`) that both variants
/// share. Returns `(tempdir_guard, apps/site project root)`.
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

    // ---- the zfb runtime as a pnpm-store package ----
    // `@takazudo/zfb-runtime` + its `@takazudo/zfb` peer, copied from the
    // binary's vendor snapshot, beside a `hono` link — pnpm's
    // `.pnpm/<pkg>@<ver>/node_modules/{<pkg>,<deps…>}` sibling layout, so the
    // closure walk resolves the runtime's bare imports from its canonical
    // dir exactly as it would for a real install.
    let runtime_store = workspace_root.join("node_modules/.pnpm/@takazudo+zfb-runtime@embedded");
    let runtime_store_node_modules = runtime_store.join("node_modules");
    for package in ["zfb-runtime", "zfb"] {
        copy_dir(
            &vendor_dir().join("@takazudo").join(package),
            &runtime_store_node_modules.join("@takazudo").join(package),
        )
        .unwrap_or_else(|e| {
            panic!("copy vendored @takazudo/{package} into the fixture store: {e}")
        });
    }
    let repo_pnpm_dir = repo_root().join("node_modules/.pnpm");
    symlink(
        &find_pnpm_store_package(&repo_pnpm_dir, "hono@", "hono"),
        &runtime_store_node_modules.join("hono"),
    );

    // ---- the site's own node_modules (what `pnpm install` gives `apps/site`) ----
    let site_node_modules = site_root.join("node_modules");
    fs::create_dir_all(site_node_modules.join("@takazudo")).unwrap();
    for package in ["zfb-runtime", "zfb"] {
        symlink(
            &runtime_store_node_modules.join("@takazudo").join(package),
            &site_node_modules.join("@takazudo").join(package),
        );
    }
    symlink(
        &find_pnpm_store_package(&repo_pnpm_dir, "preact@", "preact"),
        &site_node_modules.join("preact"),
    );
    symlink(
        &find_pnpm_store_package(
            &repo_pnpm_dir,
            "preact-render-to-string@",
            "preact-render-to-string",
        ),
        &site_node_modules.join("preact-render-to-string"),
    );
    symlink(
        &workspace_root.join("packages/shared-utils"),
        &site_node_modules.join("shared-utils"),
    );
    symlink(
        &workspace_root.join("packages/ui"),
        &site_node_modules.join("ui"),
    );

    (tmp, site_root)
}

/// The parsed `[zfb-staging-stats] physical_scans=N logical_visits=M
/// workspace_staging_activated=bool` line (`NodeModulesStagingStats` in
/// `crates/zfb-build/src/bundler.rs`, #3139).
#[derive(Debug, PartialEq, Eq)]
struct StagingStats {
    physical_scans: usize,
    logical_visits: usize,
    workspace_staging_activated: bool,
}

fn parse_staging_stats(line: &str) -> Option<StagingStats> {
    let rest = line.split("[zfb-staging-stats]").nth(1)?;
    let mut physical_scans = None;
    let mut logical_visits = None;
    let mut workspace_staging_activated = None;
    for token in rest.split_whitespace() {
        let (key, value) = token.split_once('=')?;
        match key {
            "physical_scans" => physical_scans = value.parse().ok(),
            "logical_visits" => logical_visits = value.parse().ok(),
            "workspace_staging_activated" => workspace_staging_activated = value.parse().ok(),
            _ => {}
        }
    }
    Some(StagingStats {
        physical_scans: physical_scans?,
        logical_visits: logical_visits?,
        workspace_staging_activated: workspace_staging_activated?,
    })
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

/// Runs real `zfb build` over both fixture variants, prints each
/// wall-clock time, and asserts both builds succeed AND that the staging
/// stats match each variant's expectation — since #3142 neither variant
/// walks any package or flips workspace staging. See the file header for what
/// is (and is not) proved by this synthetic-scale fixture.
#[cfg(unix)]
#[test]
fn zfb_build_staging_stats_match_expectation_for_both_fixture_variants() {
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

        let stats_line = outcome.staging_stats_line.clone().unwrap_or_else(|| {
            panic!(
                "no [zfb-staging-stats] line for the {} variant (ZFB_STAGING_STATS=1 was set; \
                 #3139's hook must be present).\n--- combined output ---\n{}",
                variant.label(),
                outcome.combined
            )
        });
        let stats = parse_staging_stats(&stats_line)
            .unwrap_or_else(|| panic!("unparseable staging stats line: {stats_line}"));
        eprintln!(
            "[collection-seeds-3133 baseline] variant={} elapsed_ms={} {}",
            variant.label(),
            outcome.elapsed_ms,
            stats_line.trim(),
        );
        assert_eq!(
            stats,
            variant.expected_stats(),
            "staging stats for the {} variant drifted from #3141's measured expectation \
             (see this file's header and the fixture README)",
            variant.label()
        );
        outcomes.push((variant.label(), outcome));
    }

    assert_eq!(outcomes.len(), 2, "expected exactly 2 baseline runs");
}

#[test]
fn parse_staging_stats_reads_the_hook_line() {
    assert_eq!(
        parse_staging_stats(
            "[zfb-staging-stats] physical_scans=4 logical_visits=6 workspace_staging_activated=true"
        ),
        Some(StagingStats {
            physical_scans: 4,
            logical_visits: 6,
            workspace_staging_activated: true,
        })
    );
    assert_eq!(
        parse_staging_stats("[zfb-staging-stats] physical_scans=4"),
        None
    );
}

// ---------------------------------------------------------------------------
// Dev confirmation (#3143, Track B3): `zfb dev --port 0` becomes ready
// within a deadline derived from a measured `control` distribution, and the
// SAME `[zfb-staging-stats]` line holds for a dev boot, not just `zfb
// build`. This binary now spawns `zfb dev`, so it lives in nextest's
// `e2e-heavy-locked` group and adopts issue #1339's cross-binary flock — see
// this file's header and `crates/CLAUDE.md`'s manifest.
// ---------------------------------------------------------------------------

/// Cross-binary flock guard's in-binary serial companion — acquired AFTER
/// the flock, same lock-ordering convention as `dev_out_of_root_collection_e2e.rs`
/// (see `zfb-test-utils/src/cross_binary_lock.rs`).
#[cfg(unix)]
static DEV_SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

#[cfg(unix)]
const DEV_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[cfg(unix)]
const DEV_BOOT_DEADLINE: Duration = Duration::from_secs(90);

/// `zfb dev --port 0` "dev-ready" deadline (first `GET /` 200) for BOTH
/// fixture variants.
///
/// Derivation (CLAUDE.md rule 8 — from a measured latency distribution, not
/// a guess): 5 fresh `control`-variant boots were timed on this container
/// (shared 4 CPUs; other worktrees' `cargo`/`rustc` work contends for the
/// same cores) by `measure_control_dev_ready_distribution` below
/// (`#[ignore = "verification: ..."]`, run once by hand with `--ignored
/// --exact --nocapture`, 2026-09-25):
///
/// 2943 / 2982 / 2835 / 2933 / 2831 ms — mean 2904 ms, max 2982 ms.
///
/// `with-collection` costs no more than `control` since #3142 (both
/// variants seed zero packages and flip no staging — see the file header),
/// so the SAME deadline covers both variants. `DEV_READY_DEADLINE` is the
/// measured max (2982 ms) rounded up and given roughly 3.4x headroom (10s)
/// to absorb this container's shared-CPU noise (other worktrees' cargo/
/// rustc contending for the same 4 cores) rather than pin a tight per-run
/// bound — the same generous-multiplier convention
/// `dev_out_of_root_collection_e2e.rs`'s `BOOT_DEADLINE` (90s against
/// sub-second real boots) and this file's own `zfb build` numbers
/// (#3138/#3141's 3-run spreads) already use.
#[cfg(unix)]
const DEV_READY_DEADLINE: Duration = Duration::from_secs(10);

#[cfg(unix)]
struct DevGuard {
    child: std::process::Child,
    pgid: libc::pid_t,
}

#[cfg(unix)]
impl Drop for DevGuard {
    fn drop(&mut self) {
        unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

/// Extract the port from the dev ready banner (`→ ready on http://localhost:PORT/`)
/// — same parsing as `dev_out_of_root_collection_e2e.rs`'s `parse_ready_port`
/// (a deliberate copy; Rust integration tests are separate binaries and
/// cannot import another test file's private items).
#[cfg(unix)]
fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(idx) = rest.find("http://") {
        let candidate = &rest[idx + "http://".len()..];
        let token: &str = candidate.split_whitespace().next().unwrap_or("");
        if let Some(colon) = token.find(':') {
            let digits: String = token[colon + 1..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
        rest = &rest[idx + "http://".len()..];
    }
    None
}

#[cfg(unix)]
fn read_log(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_default()
}

/// One dev-boot outcome: the process guard (kept alive so the caller can
/// read logs / let it Drop-kill), elapsed ms to the first `GET /` 200, and
/// the first `[zfb-staging-stats]` line if one was printed.
#[cfg(unix)]
struct DevReadyOutcome {
    _guard: DevGuard,
    elapsed_ms: u128,
    staging_stats_line: Option<String>,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

/// Boots a real `zfb dev --port 0` over `project_root` and waits for the
/// FIRST `GET /` 200 — the "dev-ready" instant `DEV_READY_DEADLINE` is
/// measured against. Returns `None` when the process exits with a known
/// environmental skip indicator (no V8 / no esbuild / no tailwind),
/// matching `is_known_skip`'s convention above and
/// `dev_out_of_root_collection_e2e.rs`'s `boot_and_handshake`.
#[cfg(unix)]
async fn boot_dev_and_wait_ready(project_root: &Path, esbuild: &Path) -> Option<DevReadyOutcome> {
    let stdout_path = project_root.join(".zfb-dev-stdout.log");
    let stderr_path = project_root.join(".zfb-dev-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log file");

    let mut cmd = Command::new(zfb_binary!());
    cmd.arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(project_root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .env("ZFB_STAGING_STATS", "1")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    cmd.process_group(0);
    let child = cmd.spawn().expect("spawn `zfb dev`");
    let pgid = child.id() as libc::pid_t;
    let mut guard = DevGuard { child, pgid };

    let start = Instant::now();
    let port = loop {
        if let Some(status) = guard.child.try_wait().expect("try_wait on `zfb dev`") {
            let combined = format!("{}{}", read_log(&stdout_path), read_log(&stderr_path));
            if is_known_skip(&combined) {
                return None;
            }
            panic!(
                "`zfb dev` exited prematurely (status {status:?}) before printing the ready \
                 banner.\n--- stdout ---\n{}\n--- stderr ---\n{}",
                read_log(&stdout_path),
                read_log(&stderr_path),
            );
        }
        if let Some(port) = parse_ready_port(&read_log(&stdout_path)) {
            break port;
        }
        assert!(
            start.elapsed() < DEV_BOOT_DEADLINE,
            "`zfb dev` did not print a parseable ready banner within {}s.\n--- stdout ---\n{}\n\
             --- stderr ---\n{}",
            DEV_BOOT_DEADLINE.as_secs(),
            read_log(&stdout_path),
            read_log(&stderr_path),
        );
        tokio::time::sleep(DEV_POLL_INTERVAL).await;
    };

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client");
    let base = format!("http://localhost:{port}");
    loop {
        if let Ok(resp) = client.get(&base).send().await {
            if resp.status().as_u16() == 200 {
                break;
            }
        }
        assert!(
            start.elapsed() < DEV_BOOT_DEADLINE,
            "GET / never answered 200 within {}s after `zfb dev` started.\n--- stdout ---\n{}\n\
             --- stderr ---\n{}",
            DEV_BOOT_DEADLINE.as_secs(),
            read_log(&stdout_path),
            read_log(&stderr_path),
        );
        tokio::time::sleep(DEV_POLL_INTERVAL).await;
    }
    let elapsed_ms = start.elapsed().as_millis();

    let stderr = read_log(&stderr_path);
    let stdout = read_log(&stdout_path);
    let staging_stats_line = stderr
        .lines()
        .chain(stdout.lines())
        .find(|line| line.contains("[zfb-staging-stats]"))
        .map(str::to_string);

    Some(DevReadyOutcome {
        _guard: guard,
        elapsed_ms,
        staging_stats_line,
        stdout_path,
        stderr_path,
    })
}

/// One-time measurement of the `control`-variant dev-ready distribution used
/// to derive [`DEV_READY_DEADLINE`]'s doc comment above. NOT a regression
/// guard (CLAUDE.md rule 6) — run once by hand whenever the deadline needs
/// re-deriving (e.g. after a change that legitimately shifts dev-boot cost):
///
/// ```text
/// cargo test -p zfb --test collection_seed_3133_baseline \
///   -- --ignored --exact measure_control_dev_ready_distribution --nocapture
/// ```
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "verification: one-time measurement of the control dev-ready distribution used to \
            derive DEV_READY_DEADLINE (#3143)"]
async fn measure_control_dev_ready_distribution() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[collection_seed_3133_baseline] no esbuild binary available; skipping measurement."
        );
        return;
    };
    let mut samples = Vec::new();
    for i in 0..5 {
        let (_tmp, project_root) = materialise_fixture(&Variant::Control);
        let Some(outcome) = boot_dev_and_wait_ready(&project_root, &esbuild).await else {
            eprintln!("[measure] run {i}: known-skip; aborting measurement.");
            return;
        };
        eprintln!(
            "[measure] control run {i}: dev-ready in {} ms",
            outcome.elapsed_ms
        );
        samples.push(outcome.elapsed_ms);
    }
    let max = samples.iter().max().copied().unwrap_or(0);
    let mean = samples.iter().sum::<u128>() / samples.len() as u128;
    eprintln!("[measure] control dev-ready samples: {samples:?} mean={mean}ms max={max}ms");
}

/// Falsifiability: temporarily reverting #3142's `bundler.rs` seed-filter
/// change (`git show a87b8ea -- crates/zfb-build/src/bundler.rs | git apply
/// -R`, see this file's header and the fixture README) makes
/// `with-collection` walk the whole sibling package's closure again — the
/// staging-stats assertion below fails (`0/0/false` -> `7/9/true`) even
/// though `zfb dev` still becomes ready within the deadline (the extra work
/// is invisible to a bare readiness check on this toy-scale fixture, which
/// is exactly why the stats assertion, not just the deadline, is
/// load-bearing here — see the #3143 sub-issue and decision-3141.md's
/// "Target stats" section).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn zfb_dev_becomes_ready_within_deadline_and_matches_staging_stats() {
    // Cross-binary lock acquired BEFORE the in-binary serial guard — see the
    // lock-ordering note in zfb-test-utils/src/cross_binary_lock.rs.
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let _serial = DEV_SERIAL.lock().await;
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[collection_seed_3133_baseline] no esbuild binary available; skipping dev \
             confirmation. Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };

    for variant in [Variant::Control, Variant::WithCollection] {
        let (_tmp, project_root) = materialise_fixture(&variant);
        let Some(outcome) = boot_dev_and_wait_ready(&project_root, &esbuild).await else {
            eprintln!(
                "[collection_seed_3133_baseline] `zfb dev` exited with a known-skip indicator \
                 for the {} variant; skipping test.",
                variant.label(),
            );
            return;
        };

        assert!(
            outcome.elapsed_ms <= DEV_READY_DEADLINE.as_millis(),
            "zfb dev for the {} variant took {} ms to become ready, over the {} ms deadline \
             derived from the measured control distribution (see DEV_READY_DEADLINE's doc \
             comment).\n--- stdout ---\n{}\n--- stderr ---\n{}",
            variant.label(),
            outcome.elapsed_ms,
            DEV_READY_DEADLINE.as_millis(),
            read_log(&outcome.stdout_path),
            read_log(&outcome.stderr_path),
        );

        let stats_line = outcome.staging_stats_line.clone().unwrap_or_else(|| {
            panic!(
                "no [zfb-staging-stats] line for the {} variant's dev boot (ZFB_STAGING_STATS=1 \
                 was set).\n--- stdout ---\n{}\n--- stderr ---\n{}",
                variant.label(),
                read_log(&outcome.stdout_path),
                read_log(&outcome.stderr_path),
            )
        });
        let stats = parse_staging_stats(&stats_line)
            .unwrap_or_else(|| panic!("unparseable staging stats line: {stats_line}"));
        eprintln!(
            "[collection-seeds-3133 dev confirmation] variant={} elapsed_ms={} {}",
            variant.label(),
            outcome.elapsed_ms,
            stats_line.trim(),
        );
        assert_eq!(
            stats,
            variant.expected_stats(),
            "dev-boot staging stats for the {} variant drifted from #3141's measured \
             expectation (see this file's header and the fixture README)",
            variant.label(),
        );
    }
}
