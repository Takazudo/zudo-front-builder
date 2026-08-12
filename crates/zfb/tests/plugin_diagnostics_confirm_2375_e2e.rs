//! Confirm e2e for the "Plugin Diagnostics & Hygiene" epic (#2368,
//! sub-issue #2375) — the `zfb build` half. #2369 (log/stderr rendering),
//! #2370 (failed re-invoke surfacing, a `zfb dev`-only concern not
//! exercised here), #2371 (stale plugin-bundle sweep), and #2373 (console
//! redirection / `watchFiles` directory rejection) each have their own
//! crate-level unit coverage, stubbing the plugin host directly. None of
//! them drives a real `zfb build` process end to end and reads what a user
//! actually sees on the terminal — issue #2367 reported exactly that gap:
//! `postBuild` logging is silent even though the hook provably runs. This
//! test closes it for the build path (a sibling dev e2e covers #2370's
//! re-invoke-failure surfacing separately).
//!
//! ## Fixture
//!
//! Generated inline (matching `build_cleans_outdir.rs`'s style — no
//! standing fixture directory needed for a project this small): a minimal
//! preact page plus a `.ts` plugin registered via `zfb.config.json`'s
//! relative-path plugin loading (`config.rs`'s `resolve_json_plugin_modules`).
//! The plugin entry being a genuine `.ts` file (not `.mjs`) is load-bearing
//! — it is what routes the plugin through `plugin_bundler::bundle_plugin_entry`
//! (issue #2308) at all, which is the code path issue #2371's stale-bundle
//! sweep and this test's cleanup assertion actually exercise. A plain
//! `.mjs` plugin would never touch `bundle_plugin_entry` and would prove
//! nothing about either.
//!
//! The plugin's `setup` hook (invoked for `zfb build` via
//! `commands::plugins::run_plugin_setup`, `SetupCommand::Build`) and its
//! `postBuild` hook each call `ctx.logger.info(...)` with a distinct
//! marker, so a regression in either call site's terminal rendering names
//! itself. `postBuild` also writes directly to `process.stderr` — bypassing
//! the redirected `console` (#2373) entirely — which the plugin host
//! forwards to the REAL OS pipe `plugin_runner.rs`'s `run_stderr_reader`
//! drains; this is deliberately a different code path from the log-envelope
//! rendering the two `logger.info` calls exercise, per the sub-issue's
//! acceptance criteria (assertion 2 vs. assertions 1).
//!
//! ## Terminal-rendering assertions (issue #2369 confirm)
//!
//! Checked against the REAL `zfb build` subprocess's captured stderr
//! (`std::process::Command::output()`, not an in-process call — no
//! `tracing_subscriber` is installed anywhere in the `zfb` binary, so only
//! the `eprintln!` channel `plugin_runner.rs` added is visible to
//! production users, exactly as `format_plugin_log_line`'s and
//! `format_plugin_host_warn_line`'s own doc comments describe):
//!
//! 1. `zfb info: [plugin:diag-plugin] SETUP-INFO-MARKER-2375` (setup hook).
//! 2. `zfb info: [plugin:diag-plugin] POSTBUILD-INFO-MARKER-2375` (postBuild
//!    hook) — the literal symptom issue #2367 reported missing.
//! 3. `zfb warn: [plugin-host stderr] POSTBUILD-STDERR-MARKER-2375` (raw
//!    `process.stderr.write`, issue #2369's `run_stderr_reader` arm).
//!
//! ## Stale-bundle sweep + clean-exit cleanup assertions (issue #2371 confirm)
//!
//! Before running the build, a `.zfb-plugin-bundle-STALE2375.mjs` file is
//! planted next to the plugin's `.ts` source (`bundle_plugin_entry` stages
//! into the entry's OWN parent directory — see that function's doc comment
//! — so this is the exact directory `sweep_stale_plugin_bundle_files`
//! scans) and its mtime is backdated past
//! `PLUGIN_BUNDLE_TEMP_STALE_AFTER` (600s) via `File::set_modified`, mirroring
//! the established backdate idiom used by `dev_poll_backend_e2e.rs` and
//! `bundler.rs`'s `backdate_dir_mtime`. This file has a FIXED, hand-chosen
//! name distinct from any name `tempfile::Builder` would ever generate for
//! the run's own live staged bundle, so its removal can only be attributed
//! to the sweep in `bundle_plugin_entry` — never to that run's own
//! `StagedPluginBundle` `Drop` guard, which only ever deletes the file IT
//! created.
//!
//! After the build exits (status checked first — the sweep is best-effort
//! and must never fail a build, but a non-zero exit here would mean
//! something else broke and the cleanup assertions below would be
//! meaningless), the test walks the WHOLE fixture tree with `WalkDir` and
//! asserts NO `.zfb-plugin-bundle-*.mjs` file remains anywhere — covering
//! both halves of the sub-issue's assertion 3/4 pair in one pass: the
//! pre-planted stale file (proving the sweep ran) AND this run's own
//! freshly-staged bundle (proving `StagedPluginBundle`'s `Drop` guard fires
//! on a normal, successful process exit — `HostInner::_staged_plugin_bundles`
//! is held for the whole host session and dropped when the in-process
//! `PluginHost` value goes out of scope at the end of `commands::build::run`,
//! well before the subprocess's `main()` returns).
//!
//! ## Self-skip, not `#[ignore]` (per `crates/CLAUDE.md`'s taxonomy)
//!
//! Skips (without failing) when esbuild is unavailable
//! (`zfb_test_utils::locate_esbuild`, matching every other real-binary e2e
//! in this crate) or when `node` is not on PATH (the plugin host always
//! spawns `node`, and bundling a `.ts` plugin entry additionally shells out
//! to esbuild — `build_cleans_outdir.rs`'s `prebuild_plugin_emitted_file_survives_wipe`
//! is the precedent for gating a plugin-bearing build test on both). health.yml
//! always stages a pinned esbuild and a pinned node, so this is a
//! local-dev convenience only, not a CI blocker. None of the 5 `#[ignore]`
//! taxonomy prefixes apply — this is a `zfb build`-only e2e (`.output()`,
//! no long-lived server), so it joins the nextest `e2e-heavy`
//! **build-only** bucket (no flock needed, matching that bucket's own
//! documented rationale — the test-group is the sole serialization guard
//! for build-only binaries) rather than the flock-adopting one.
//!
//! ## Modeled on
//!
//! `build_cleans_outdir.rs`'s inline-fixture-generation + `Command::output()`
//! style for the build-only e2e shape, and its own
//! `prebuild_plugin_emitted_file_survives_wipe` test for the
//! esbuild-and-node dual self-skip gate. The mtime-backdate idiom follows
//! `dev_poll_backend_e2e.rs`'s `backdate` helper and `bundler.rs`'s
//! `backdate_dir_mtime`.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

use walkdir::WalkDir;
use zfb_test_utils::{locate_esbuild, zfb_binary};

/// Comfortably past `zfb_build::plugin_bundler::PLUGIN_BUNDLE_TEMP_STALE_AFTER`
/// (600s — itself pinned above the 5-minute esbuild bundle timeout, since a
/// staged bundle is unlocked while esbuild writes it) — not imported directly
/// since `plugin_bundler` is a private module or would pull in a heavier
/// dependency surface than this black-box e2e needs; the sweep's own unit
/// tests in that crate already pin the constant's value, so this is a second,
/// independent measurement of the public contract rather than a coupling to
/// the private constant.
const STALE_BACKDATE: Duration = Duration::from_secs(1200);

fn host_node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn dump(output: &std::process::Output) -> String {
    format!(
        "status={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

/// Write the fixture: a minimal preact page plus a `.ts` plugin registered
/// via `zfb.config.json`. Returns the plugin's own directory (where the
/// staged bundle — live and stale — lands).
fn write_fixture(root: &Path) -> std::path::PathBuf {
    fs::write(
        root.join("zfb.config.json"),
        r#"{
  "framework": "preact",
  "plugins": [{ "name": "./plugin/diag-plugin.ts" }]
}
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("pages")).unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        r#"export default function Page() {
  return (
    <html lang="en">
      <head><title>plugin diagnostics confirm</title></head>
      <body><p>hello</p></body>
    </html>
  );
}
"#,
    )
    .unwrap();

    let plugin_dir = root.join("plugin");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(
        plugin_dir.join("diag-plugin.ts"),
        r#"const diagPlugin = {
  name: "diag-plugin",
  setup(ctx: { logger: { info(msg: string): void } }) {
    ctx.logger.info("SETUP-INFO-MARKER-2375");
  },
  postBuild(ctx: { logger: { info(msg: string): void } }) {
    ctx.logger.info("POSTBUILD-INFO-MARKER-2375");
    process.stderr.write("POSTBUILD-STDERR-MARKER-2375\n");
  },
};

export default diagPlugin;
"#,
    )
    .unwrap();

    plugin_dir
}

/// Plant a stale `.zfb-plugin-bundle-*.mjs` file next to the plugin's `.ts`
/// source, backdated past the sweep's staleness threshold. Fixed, hand-
/// chosen filename — see the file header comment for why this matters to
/// the "swept by the sweep, not by this run's own Drop guard" assertion.
fn plant_stale_bundle(plugin_dir: &Path) -> std::path::PathBuf {
    let stale = plugin_dir.join(".zfb-plugin-bundle-STALE2375.mjs");
    fs::write(
        &stale,
        b"// leaked plugin bundle from an aborted prior run\n",
    )
    .unwrap();
    fs::File::open(&stale)
        .and_then(|f| f.set_modified(SystemTime::now() - STALE_BACKDATE))
        .expect("backdate stale plugin bundle mtime");
    stale
}

/// Every `.zfb-plugin-bundle-*.mjs` path found anywhere under `root`.
fn find_plugin_bundle_temp_files(root: &Path) -> Vec<std::path::PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.starts_with(".zfb-plugin-bundle-") && name.ends_with(".mjs")
        })
        .collect()
}

#[test]
fn setup_and_postbuild_logging_and_stderr_reach_the_terminal_and_temp_bundle_is_swept_and_cleaned()
{
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[plugin_diagnostics_confirm_2375_e2e] no esbuild binary available; skipping. \
             Set ZFB_ESBUILD_BIN or install esbuild on PATH."
        );
        return;
    };
    if !host_node_available() {
        eprintln!(
            "[plugin_diagnostics_confirm_2375_e2e] node not on PATH; skipping — the plugin \
             host always spawns node, and bundling a .ts plugin entry additionally needs it."
        );
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let plugin_dir = write_fixture(root);
    let stale_bundle = plant_stale_bundle(&plugin_dir);
    assert!(
        stale_bundle.exists(),
        "sanity: the pre-planted stale bundle file must exist before the build runs"
    );

    let out = Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .output()
        .expect("spawn `zfb build`");

    assert!(out.status.success(), "zfb build failed\n{}", dump(&out));

    let stderr = String::from_utf8_lossy(&out.stderr);

    // Assertion 1: logger.info in `setup` reaches the terminal.
    assert!(
        stderr.contains("zfb info: [plugin:diag-plugin] SETUP-INFO-MARKER-2375"),
        "expected the setup hook's logger.info line on stderr; got:\n{}",
        dump(&out)
    );

    // Assertion 1 (postBuild half) — issue #2367's literal reported symptom.
    assert!(
        stderr.contains("zfb info: [plugin:diag-plugin] POSTBUILD-INFO-MARKER-2375"),
        "expected the postBuild hook's logger.info line on stderr; got:\n{}",
        dump(&out)
    );

    // Assertion 2: a raw `process.stderr.write` from inside a plugin hook
    // is rendered too — a different code path from the log-envelope lines
    // above (plugin_runner.rs's `run_stderr_reader`, not `handle_line`'s
    // `HostLine::Log` arm).
    assert!(
        stderr.contains("zfb warn: [plugin-host stderr] POSTBUILD-STDERR-MARKER-2375"),
        "expected the postBuild hook's process.stderr.write line on stderr; got:\n{}",
        dump(&out)
    );

    // Sanity: the build actually produced output (a broken fixture failing
    // silently before ever reaching the plugin hooks would otherwise be
    // indistinguishable from a genuine terminal-rendering regression).
    assert!(
        root.join("dist/index.html").is_file(),
        "dist/index.html must be emitted by a successful build\n{}",
        dump(&out)
    );

    // Assertion 3 + 4: after a successful, clean process exit, NO
    // `.zfb-plugin-bundle-*.mjs` file remains anywhere in the fixture tree
    // — neither the pre-planted stale one (swept by
    // `sweep_stale_plugin_bundle_files` before this run's bundle staged)
    // nor this run's own live staged bundle (removed by
    // `StagedPluginBundle`'s `Drop` guard when the host is torn down at
    // the end of a normal `zfb build` process).
    let leftover = find_plugin_bundle_temp_files(root);
    assert!(
        leftover.is_empty(),
        "no .zfb-plugin-bundle-*.mjs file may remain after a clean `zfb build` exit; found: \
         {leftover:#?}\n{}",
        dump(&out)
    );
}
