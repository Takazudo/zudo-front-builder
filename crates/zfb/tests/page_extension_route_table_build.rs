//! Sub #1993 (Page Extension Contract epic #1990, Wave 3) — production
//! route-table coverage for the widened page-extension contract.
//!
//! Wave 2 (#1992) centralized the accepted page extensions in
//! `zfb_types::ROUTABLE_PAGE_EXTENSIONS` and widened `zfb-router`'s
//! `scan_pages` to accept `.ts` / `.js` / `.jsx` alongside the pre-existing
//! `.tsx` / `.mdx` / `.md` / `.html`. That is a router-only change; the
//! production build path (`crates/zfb/src/commands/build.rs:230-235`) takes
//! its route table from `Router::scan`, so on paper the widening should
//! already flow through — but a router change that never actually reaches
//! `zfb build` would leave issue #1742 only half-fixed. This test proves it
//! does, at Level 4 (real `zfb build` binary), by rendering a `.ts` page
//! (plus `.js` / `.jsx` siblings for the same proof) all the way to emitted
//! HTML — not merely asserting the scanner returns a route.
//!
//! `.ts` (and `.js`/`.jsx`) pages carry no JSX syntax, so the pages below
//! build their element tree with preact's `h()` directly instead of JSX.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zfb_test_utils::{locate_esbuild, zfb_binary};

/// Wall-clock deadline for the `zfb build` subprocess. Generous enough for
/// a cold V8 + esbuild boot, far under the nextest `e2e-heavy` group's 600s
/// `terminate-after`, so a hang surfaces as this test's own diagnostic
/// panic rather than a runner-level kill.
const BUILD_DEADLINE: Duration = Duration::from_secs(180);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Owns the spawned `zfb build` process. Drop kills the whole process
/// group (on unix) so a hung build — or an esbuild child that outlives it —
/// can never outlive this test, whether the deadline loop below catches it
/// or the test unwinds for any other reason.
///
/// This mirrors `page_extension_full_matrix_e2e.rs`'s `BuildGuard`. Wave
/// 4's header documents why the plain `Command::output()` this test used
/// originally is unsafe here: it blocks unbounded and keeps the child alive
/// with no deadline and no group cleanup, in a non-`#[ignore]`d T1 test
/// that boots real V8 + esbuild.
struct BuildGuard {
    child: std::process::Child,
    #[cfg(unix)]
    pgid: libc::pid_t,
}

impl Drop for BuildGuard {
    fn drop(&mut self) {
        // Best-effort: ESRCH (already gone) is harmless.
        #[cfg(unix)]
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        #[cfg(not(unix))]
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn write_project(root: &Path) {
    fs::write(
        root.join("zfb.config.json"),
        r#"{ "framework": "preact" }
"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("pages")).unwrap();

    // `.ts` page — the sub-issue's headline case. `pages/index.ts` was
    // bundle-capable (zfb-build's `derive_route`) but never routed
    // (zfb-router's pre-#1992 `ACCEPTED_PAGE_EXTENSIONS`) — issue #1742.
    fs::write(
        root.join("pages/index.ts"),
        r#"import { h } from "preact";

export default function Page() {
  return h(
    "html",
    { lang: "en" },
    h("head", null, h("title", null, "ts page")),
    h("body", null, h("p", null, "hello from a .ts page")),
  );
}
"#,
    )
    .unwrap();

    // `.js` sibling — same widening, different extension.
    fs::write(
        root.join("pages/about.js"),
        r#"import { h } from "preact";

export default function Page() {
  return h(
    "html",
    { lang: "en" },
    h("head", null, h("title", null, "js page")),
    h("body", null, h("p", null, "hello from a .js page")),
  );
}
"#,
    )
    .unwrap();

    // `.jsx` sibling — same widening, different extension. `.jsx` DOES
    // support JSX syntax, but this page deliberately still uses `h()` so
    // the fixture doesn't accidentally depend on JSX transform behavior
    // that `.tsx` already covers elsewhere.
    fs::write(
        root.join("pages/contact.jsx"),
        r#"import { h } from "preact";

export default function Page() {
  return h(
    "html",
    { lang: "en" },
    h("head", null, h("title", null, "jsx page")),
    h("body", null, h("p", null, "hello from a .jsx page")),
  );
}
"#,
    )
    .unwrap();
}

/// Renders a `.ts` page (plus `.js` / `.jsx` siblings) through a real `zfb
/// build`, proving the widened `ROUTABLE_PAGE_EXTENSIONS` contract reaches
/// the production route table (`commands/build.rs`), not just the router's
/// own unit tests. Also asserts no spurious "unrecognised extension"
/// warning fires for any of the three newly accepted extensions.
#[test]
fn widened_script_page_extensions_render_end_to_end_via_real_build() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[page_extension_route_table_build] no esbuild binary available; \
             set ZFB_ESBUILD_BIN, place the binary at \
             crates/zfb/binaries/esbuild/esbuild, or install esbuild on PATH \
             to enable this test. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir for project root");
    let root = tmp.path();
    write_project(root);

    // Own process group, output captured to files (never pipes — a full
    // pipe buffer is its own deadlock), and a bounded poll loop so this
    // test always returns and the child is always killed.
    let stdout_path = root.join(".zfb-build-stdout.log");
    let stderr_path = root.join(".zfb-build-stderr.log");
    let stdout_file = fs::File::create(&stdout_path).expect("create build stdout log file");
    let stderr_file = fs::File::create(&stderr_path).expect("create build stderr log file");

    let mut cmd = Command::new(zfb_binary!());
    cmd.arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", &esbuild)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd.spawn().expect("spawn `zfb build`");
    #[cfg(unix)]
    let pgid = child.id() as libc::pid_t;
    let mut guard = BuildGuard {
        child,
        #[cfg(unix)]
        pgid,
    };

    let start = Instant::now();
    let status = loop {
        if let Some(status) = guard.child.try_wait().expect("try_wait on `zfb build`") {
            break status;
        }
        if start.elapsed() >= BUILD_DEADLINE {
            panic!(
                "`zfb build` did not exit within {}s — treating as a hang; process group \
                 killed by BuildGuard's Drop.\nstdout: {}\nstderr: {}",
                BUILD_DEADLINE.as_secs(),
                read_log(&stdout_path),
                read_log(&stderr_path),
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    let stdout = read_log(&stdout_path);
    let stderr = read_log(&stderr_path);
    let combined = format!("{stdout}{stderr}");

    if !status.success() {
        if combined.contains("embed_v8")
            || combined.contains("no esbuild")
            || combined.contains("no tailwind")
            || (combined.contains("tailwindcss") && combined.contains("not found"))
        {
            eprintln!(
                "[page_extension_route_table_build] zfb build exited non-zero with \
                 a known-skip indicator (V8/esbuild/tailwind unavailable); \
                 skipping test.\nstdout: {stdout}\nstderr: {stderr}"
            );
            return;
        }
        panic!(
            "zfb build failed unexpectedly for the .ts/.js/.jsx page fixture.\n\
             status: {status:?}\nstdout: {stdout}\nstderr: {stderr}",
        );
    }

    // Production route table coverage: all three widened script-page
    // extensions must have reached the build and rendered real HTML —
    // source to emitted output, the whole `zfb build` pipeline, not a
    // scanner-only assertion.
    let dist = root.join("dist");

    let ts_html_path = dist.join("index.html");
    let ts_html = fs::read_to_string(&ts_html_path).unwrap_or_else(|e| {
        panic!(
            "expected emitted page at {} (from pages/index.ts): {e}\n\
             dist/ contents: {:#?}",
            ts_html_path.display(),
            fs::read_dir(&dist)
                .ok()
                .map(|rd| rd.flatten().map(|e| e.path()).collect::<Vec<_>>())
        )
    });
    assert!(
        ts_html.contains("hello from a .ts page"),
        ".ts page content must reach the emitted HTML\n--- html ---\n{ts_html}"
    );

    let js_html_path = dist.join("about").join("index.html");
    let js_html = fs::read_to_string(&js_html_path)
        .unwrap_or_else(|e| panic!("expected emitted page at {}: {e}", js_html_path.display()));
    assert!(
        js_html.contains("hello from a .js page"),
        ".js page content must reach the emitted HTML\n--- html ---\n{js_html}"
    );

    let jsx_html_path = dist.join("contact").join("index.html");
    let jsx_html = fs::read_to_string(&jsx_html_path)
        .unwrap_or_else(|e| panic!("expected emitted page at {}: {e}", jsx_html_path.display()));
    assert!(
        jsx_html.contains("hello from a .jsx page"),
        ".jsx page content must reach the emitted HTML\n--- html ---\n{jsx_html}"
    );

    // No spurious "unrecognised extension" warning for any of the three
    // newly accepted extensions — a regression here would mean the router
    // widening didn't actually take effect on the production build path.
    assert!(
        !stderr.contains("unrecognised extension"),
        "expected no `unrecognised extension` warnings for .ts/.js/.jsx pages; \
         got stderr:\n{stderr}"
    );
}
