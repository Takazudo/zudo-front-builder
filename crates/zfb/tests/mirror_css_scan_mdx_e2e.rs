//! Level-4 dev e2e for issue #1819 (Mirror Root CSS Scan epic #1995).
//!
//! Written RED in Wave 1 (#1996) — it asserted the BROKEN behaviour and
//! passed — and inverted here in Wave 2 (#1997), which implemented the fix.
//! After epic #1799 a sibling mirror-root edit correctly *delivers* a
//! filesystem event to a real `zfb dev` session; what #1819 was about is
//! that for a `.md`/`.mdx` file the event did not rerun the Tailwind content
//! scan, so a utility class authored only in a sibling markdown file never
//! reached the served dev CSS without a restart. Dev-loop only; prod builds
//! were never affected (`discover_css_source_files`,
//! `crates/zfb/src/commands/build.rs`, already walks `.md`/`.mdx` inside a
//! claimed mirror root on every build).
//!
//! ## The mechanism under test
//!
//! `discover_css_source_files` scans `.md`/`.mdx` inside a claimed sibling
//! mirror root. But an out-of-root `.md`/`.mdx` classifies as
//! `PathClass::Content` (`crates/zfb-build/src/policy.rs`), and the #1288
//! `mark_css` rule in the dev orchestrator was gated on `PathClass::Module`
//! ONLY (`crates/zfb-build/src/orchestrator.rs`) — in the live arm, the
//! removed-path fold, and the `External` arm alike. #1997 implements option
//! (b): `Content` (and `External`) reruns the CSS scan when — and only when
//! — the changed path lies under a registered `css_mirror_root`
//! (`GranularityPolicy::is_under_css_mirror_root`, the #1802 registry). An
//! ordinary in-root markdown edit is deliberately left alone; that negative
//! is pinned by
//! `zfb_build::orchestrator::tests::in_root_content_edit_outside_mirror_roots_does_not_rerun_css`.
//!
//! ## Fixture discipline (modeled on
//! `dev_sibling_watch_1678_e2e.rs::e2e_dev_sibling_tailwind_utility_class_refreshes_served_css`)
//!
//! - The sibling directory is claimed ONLY through a tsconfig wildcard alias
//!   (`SiblingMirrorPlan` claim source (b), which is purely alias-based per
//!   `SiblingMirrorPlan::compute` — no import of any file under it is
//!   required for the mirror root to be claimed and watched). NOTHING in
//!   this fixture imports the `.mdx` file, or anything else in its
//!   directory: if it did, the file-parent `watch_additional_files` channel
//!   would already keep it watched and this test would keep passing even
//!   with the eventual #1997 fix fully reverted (see
//!   `l-lessons-dev-watcher-narrowing`'s "a second dynamic-watch registry"
//!   entry).
//! - The project carries its own `.gitignore` excluding `.zfb-build/` —
//!   Tailwind v4's automatic content detection respects `.gitignore`, and
//!   dev's own intermediate SSR bundle under `.zfb-build/` would otherwise
//!   leak the class string into auto-detection and mask whether the mirror
//!   root's CSS scan did anything (same confound documented at
//!   `sibling_css_module_command_layer_build.rs:649-662`).
//! - Proof the FS event genuinely arrives (the whole point of #1819 is that
//!   delivery already worked and only the CSS rerun was missing): this test
//!   waits for a `[zfb-timing] tick(): kinds=[<mdx filename>:...]` line under
//!   `ZFB_DEV_TIMING=1` before asserting on the served CSS. If the event
//!   never arrived, that wait times out first, and the test fails for THAT
//!   reason instead of attributing a delivery regression to the CSS-rerun
//!   rule under test.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary, CrossBinaryE2eLock};

/// Locate a tailwindcss v4 binary, mirroring
/// `dev_sibling_watch_1678_e2e.rs::locate_tailwind` /
/// `sibling_css_module_command_layer_build.rs`'s resolution
/// (`ZFB_TAILWIND_BIN` env var, else the workspace-staged
/// `crates/zfb/binaries/tailwindcss-v4` slot).
fn locate_tailwind() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZFB_TAILWIND_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let slot = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/tailwindcss-v4");
    slot.is_file().then_some(slot)
}

const BOOT_DEADLINE: Duration = Duration::from_secs(120);
const BOOT_CONTENT_DEADLINE: Duration = Duration::from_secs(60);
const SIGNAL_DEADLINE: Duration = Duration::from_secs(30);
// The affirmative assertion polls until the recompiled stylesheet carries
// the new class. Generous: the tick has to rerun the Tailwind subprocess
// over the whole content set before the served bytes change.
const CSS_REFRESH_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

struct DevServerGuard {
    child: std::process::Child,
    pgid: libc::pid_t,
}

impl DevServerGuard {
    fn try_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().expect("poll `zfb dev` child")
    }
}

impl Drop for DevServerGuard {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

struct DevSession {
    guard: DevServerGuard,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl DevSession {
    fn logs(&self) -> String {
        format!(
            "--- zfb dev stdout ---\n{}\n--- zfb dev stderr ---\n{}",
            fs::read_to_string(&self.stdout_path).unwrap_or_default(),
            fs::read_to_string(&self.stderr_path).unwrap_or_default(),
        )
    }

    fn stderr(&self) -> String {
        fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }
}

/// Spawn `zfb dev --port 0` with `ZFB_DEV_TIMING=1` (surfaces both the
/// `watch-extra registered:` and `tick():` signals) and both binaries
/// pinned.
fn spawn_dev(root: &Path, esbuild: &Path, tailwind: &Path) -> DevSession {
    let stdout_path = root.join(".zfb-dev-stdout.log");
    let stderr_path = root.join(".zfb-dev-stderr.log");
    let stdout = fs::File::create(&stdout_path).expect("create dev stdout log");
    let stderr = fs::File::create(&stderr_path).expect("create dev stderr log");
    let mut command = Command::new(zfb_binary!());
    command
        .arg("dev")
        .arg("--port")
        .arg("0")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .env("ZFB_TAILWIND_BIN", tailwind)
        .env("ZFB_DEV_TIMING", "1")
        .env_remove("ZFB_DEV_EAGER")
        .env_remove("ZFB_LAZY_DEV_RENDER")
        .env_remove("ZFB_DEV_DEFER_BUNDLE")
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    command.process_group(0);
    let child = command.spawn().expect("spawn `zfb dev --port 0`");
    let pgid = child.id() as libc::pid_t;
    DevSession {
        guard: DevServerGuard { child, pgid },
        stdout_path,
        stderr_path,
    }
}

fn parse_ready_port(log: &str) -> Option<u16> {
    let mut rest = log;
    while let Some(index) = rest.find("http://") {
        let candidate = &rest[index + "http://".len()..];
        let token = candidate.split_whitespace().next().unwrap_or_default();
        if let Some(colon) = token.find(':') {
            let digits = token[colon + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>();
            if let Ok(port) = digits.parse() {
                return Some(port);
            }
        }
        rest = &rest[index + "http://".len()..];
    }
    None
}

async fn wait_for_ready(session: &mut DevSession) -> Option<u16> {
    let started = Instant::now();
    loop {
        if let Some(status) = session.guard.try_status() {
            let logs = session.logs();
            if logs.contains("embed_v8") || logs.contains("no esbuild") {
                eprintln!(
                    "[mirror_css_scan_mdx] `zfb dev` exited with a known-skip indicator \
                     (V8/esbuild unavailable); skipping test.\n{logs}"
                );
                return None;
            }
            panic!("`zfb dev` exited before readiness with {status:?}\n{logs}");
        }
        if let Some(port) =
            parse_ready_port(&fs::read_to_string(&session.stdout_path).unwrap_or_default())
        {
            assert_ne!(port, 0, "ready banner reported literal port 0");
            return Some(port);
        }
        assert!(
            started.elapsed() < BOOT_DEADLINE,
            "`zfb dev --port 0` did not become ready in {}s\n{}",
            BOOT_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until a `watch-extra registered:` line whose directory ends with
/// `dir_suffix` appears in the dev server's stderr — the deterministic
/// signal that the sibling mirror root has entered the recursive watch set
/// (`css_mirror_root_paths()`, issue #1802).
async fn wait_for_watch_extra(session: &DevSession, dir_suffix: &str) {
    let started = Instant::now();
    while started.elapsed() < SIGNAL_DEADLINE {
        if session.stderr().lines().any(|line| {
            line.contains("watch-extra registered:") && line.trim_end().ends_with(dir_suffix)
        }) {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "no `watch-extra registered:` line ending in {dir_suffix:?} within {}s\n{}",
        SIGNAL_DEADLINE.as_secs(),
        session.logs(),
    );
}

/// Wait until a `[zfb-timing] tick(): kinds=[...]` line whose `kinds` list
/// mentions `filename` appears in stderr — proof the orchestrator actually
/// processed a filesystem event for that file (delivery), independent of
/// whatever the tick decided to do about CSS.
async fn wait_for_tick_mentioning(session: &DevSession, filename: &str) -> String {
    let started = Instant::now();
    while started.elapsed() < SIGNAL_DEADLINE {
        if let Some(line) = session
            .stderr()
            .lines()
            .find(|line| line.contains("tick(): kinds=[") && line.contains(filename))
        {
            return line.to_string();
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "no `[zfb-timing] tick(): kinds=[...]` line mentioning {filename:?} within {}s — the \
         filesystem event for the sibling .mdx edit never reached the orchestrator at all, \
         which would be a DIFFERENT bug than #1819 (CSS-rerun gap after successful \
         delivery)\n{}",
        SIGNAL_DEADLINE.as_secs(),
        session.logs(),
    );
}

/// Build a fresh pnpm-workspace fixture: a HOST project reaching a SIBLING
/// directory (`lib/ushared-mdx`) through a tsconfig wildcard alias — the
/// same claim shape as
/// `dev_sibling_watch_1678_e2e.rs::write_tailwind_sibling_dev_fixture`, but
/// the sibling here contains ONLY an `.mdx` file, never a `.tsx`/`.ts`
/// module, and nothing anywhere in the fixture imports it.
fn write_sibling_mdx_dev_fixture(ws_root: &Path) -> (PathBuf, tempfile::TempDir) {
    fs::write(
        ws_root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'sub-packages/*'\n",
    )
    .expect("write pnpm-workspace.yaml");
    let (nm_handle, embedded_nm_path) =
        zfb::render_pipeline::embedded_node_modules().expect("embedded_node_modules");
    std::os::unix::fs::symlink(&embedded_nm_path, ws_root.join("node_modules"))
        .expect("symlink workspace node_modules");

    let project = ws_root.join("sub-packages/mdxhost");
    fs::create_dir_all(project.join("pages")).expect("create pages/");

    // Same `.zfb-build/`-leak confound as
    // `sibling_css_module_command_layer_build.rs:649-662` and
    // `dev_sibling_watch_1678_e2e.rs`'s Scenario E fixture, restated for
    // this DEV session: without this, dev's own generated SSR bundle under
    // `.zfb-build/` could leak the new utility class into Tailwind's
    // automatic content detection and mask whether the mirror-root scan
    // under test did anything.
    fs::write(
        project.join(".gitignore"),
        ".zfb-build/\ndist/\nnode_modules/\n",
    )
    .expect("write project .gitignore");

    // No `tailwind` key -> CSS (and the Tailwind utility scan) is enabled by
    // default.
    fs::write(
        project.join("zfb.config.json"),
        "{\n  \"framework\": \"preact\"\n}\n",
    )
    .expect("write zfb.config.json");

    // The alias is the ONLY claim source for the sibling — no page or
    // script anywhere imports `@ushared-mdx/*`. Per `SiblingMirrorPlan::compute`
    // (crates/zfb-build/src/bundler.rs), a tsconfig `paths` wildcard target
    // alone is enough to claim the mirror root; no import is required.
    fs::write(
        project.join("tsconfig.json"),
        "{\n  \"compilerOptions\": {\n    \"baseUrl\": \".\",\n    \"paths\": { \"@ushared-mdx/*\": [\"../../lib/ushared-mdx/*\"] }\n  }\n}\n",
    )
    .expect("write tsconfig.json");

    fs::write(
        project.join("pages/index.tsx"),
        "export default function HomePage() {\n  \
         return (\n    <main>\n      <p>mdx red repro host</p>\n    </main>\n  );\n}\n",
    )
    .expect("write pages/index.tsx");

    let sibling = ws_root.join("lib/ushared-mdx");
    fs::create_dir_all(&sibling).expect("create lib/ushared-mdx");
    // Boot state: no Tailwind utility class the assertion below looks for
    // exists anywhere in the fixture yet. Plain markdown prose, no JSX —
    // the bug is about the FILE'S PathClass (Content, by extension), not
    // about whether the content happens to look like JSX.
    fs::write(
        sibling.join("notes.mdx"),
        "# Sibling notes\n\nJust some sibling markdown content.\n",
    )
    .expect("write lib/ushared-mdx/notes.mdx");

    (project, nm_handle)
}

/// Issue #1819 (epic #1995, fix landed in #1997): editing a
/// sibling-mirror-root `.mdx` file to introduce a brand-new Tailwind utility
/// class refreshes the served `/assets/styles.css` without a `zfb dev`
/// restart. The filesystem event for that edit is separately proven to reach
/// the dev orchestrator (the `tick():` timing line) so a delivery regression
/// can never be misread as a CSS-rerun regression.
///
/// Needs the Tailwind binary in addition to esbuild — skips cleanly when
/// unavailable.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "env-gate: tailwindcss v4 + esbuild — cargo test -p zfb --test \
            mirror_css_scan_mdx_e2e -- --ignored --exact \
            sibling_mdx_utility_class_reaches_dev_css_scan \
            (ZFB_TAILWIND_BIN or the staged crates/zfb/binaries/tailwindcss-v4 \
            slot; also needs ZFB_ESBUILD_BIN or an esbuild on PATH). Written RED \
            in #1996, inverted in #1997 — see epic #1995 / issue #1819."]
async fn sibling_mdx_utility_class_reaches_dev_css_scan() {
    let _e2e_lock = CrossBinaryE2eLock::acquire();
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[mirror_css_scan_mdx] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };
    let Some(tailwind) = locate_tailwind() else {
        eprintln!(
            "[mirror_css_scan_mdx] no tailwindcss v4 binary available; skipping. \
             Set ZFB_TAILWIND_BIN or stage crates/zfb/binaries/tailwindcss-v4."
        );
        return;
    };

    let workspace = tempfile::tempdir().expect("mirror-css-scan-mdx fixture tempdir");
    let (project, _nm_handle) = write_sibling_mdx_dev_fixture(workspace.path());
    let sibling_mdx = workspace.path().join("lib/ushared-mdx/notes.mdx");

    let mut session = spawn_dev(&project, &esbuild, &tailwind);
    let Some(port) = wait_for_ready(&mut session).await else {
        return; // environmental skip (no V8/esbuild)
    };
    let origin = format!("http://localhost:{port}");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build loopback HTTP client");
    let css_url = format!("{origin}/assets/styles.css");

    // Proof the mirror root is claimed and recursively watched (the same
    // `css_mirror_roots` / `sync_recursive_dir_watches` registry
    // `dev_sibling_watch_1678_e2e.rs`'s Scenario E confirms) — the registry
    // this repro's `.mdx` edit depends on for delivery at all.
    wait_for_watch_extra(&session, "lib/ushared-mdx").await;

    // Baseline: the served stylesheet must be reachable and must NOT yet
    // contain the utility class the edit below introduces for the first
    // time.
    let boot_css = {
        let started = Instant::now();
        loop {
            if let Ok(response) = client.get(&css_url).send().await {
                if response.status().as_u16() == 200 {
                    break response.text().await.unwrap_or_default();
                }
            }
            assert!(
                started.elapsed() < BOOT_CONTENT_DEADLINE,
                "GET {css_url} never answered 200 within {}s\n{}",
                BOOT_CONTENT_DEADLINE.as_secs(),
                session.logs(),
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    };
    assert!(
        !boot_css.contains("3c9a7b"),
        "boot stylesheet must not already contain the sibling-only utility class before \
         the edit below introduces it\n{}",
        session.logs(),
    );

    // THE EDIT — a sibling `.mdx` file (`PathClass::Content`, the
    // classification the #1288 `mark_css` rule skipped) gains a NEW
    // Tailwind-shaped utility-class token, used nowhere else in the
    // fixture. MDX content doesn't need to be real JSX for the Tailwind
    // automatic content scanner to pick up an arbitrary-value class token —
    // it scans raw text for candidate class-like substrings.
    fs::write(
        &sibling_mdx,
        "# Sibling notes\n\n<span class=\"bg-[#3c9a7b]\">edited</span>\n",
    )
    .expect("edit sibling notes.mdx to add the utility class");

    // Proof of delivery: the orchestrator DOES see this edit (a `tick()`
    // line mentions the file). This is the load-bearing assertion that
    // distinguishes #1819 (CSS-rerun gap after successful delivery) from a
    // delivery-channel gap — if this line never appears, the fixture itself
    // is wrong, not the product.
    let tick_line = wait_for_tick_mentioning(&session, "notes.mdx").await;
    eprintln!("[mirror_css_scan_mdx] observed delivery: {tick_line}");
    assert!(
        tick_line.contains("Modified") || tick_line.contains("Created"),
        "expected the tick() line for notes.mdx to report a Modified/Created kind, got: \
         {tick_line}\n{}",
        session.logs(),
    );

    // THE ASSERTION (inverted from #1996's RED form by #1997) — the tick
    // now marks CSS for a `PathClass::Content` path under a registered
    // `css_mirror_root`, so the Tailwind content scan reruns and the served
    // stylesheet picks up the sibling-only utility class, restart-free.
    let started = Instant::now();
    loop {
        if let Ok(response) = client.get(&css_url).send().await {
            if response.status().as_u16() == 200
                && response.text().await.unwrap_or_default().contains("3c9a7b")
            {
                break;
            }
        }
        assert!(
            started.elapsed() < CSS_REFRESH_DEADLINE,
            "/assets/styles.css never picked up the sibling-.mdx-only utility class within \
             {}s of the edit, even though the tick line above proves the event reached the \
             orchestrator — the mirror-root CSS rerun (#1819 / #1997) regressed.\n{}",
            CSS_REFRESH_DEADLINE.as_secs(),
            session.logs(),
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    session.guard.child.try_wait().ok();
}
