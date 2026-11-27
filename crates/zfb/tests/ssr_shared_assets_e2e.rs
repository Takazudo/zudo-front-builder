//! Real `zfb build` + executed Cloudflare worker coverage for finalized
//! production assets in request-time SSR pages (#4887).
//!
//! Each test copies `tests/fixtures/ssr-shared-assets`, optionally reshapes it
//! into a variant, runs a real `zfb build` with the Cloudflare adapter, then
//! executes the emitted `dist/_worker.js` in Node through
//! `tests/ssr-shared-assets/drive.mjs` (a Workers Static Assets stand-in
//! serving `dist/`). SSG pages come from disk; SSR pages come from the real
//! runtime bundle and router. The root and subpath builds are preserved under
//! `target/ssr-shared-assets/` for the Chromium companion in
//! `tests/ssr-shared-assets/`.
//!
//! Requires `node` and `pnpm`; skips with a log line when either is absent.
//! Serialized by nextest's `e2e-heavy-unlocked` group.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

#[path = "support/e2e_platform.rs"]
mod e2e_platform;

use e2e_platform::link_dir;
use serde_json::Value;
use zfb_test_utils::zfb_binary;

static SERIAL: LazyLock<std::sync::Mutex<()>> = LazyLock::new(|| std::sync::Mutex::new(()));

const STABLE_CSS: &str = "/assets/styles.css";
const STABLE_GREETING: &str = "/assets/client/greeting.js";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn tools_available() -> bool {
    let node = Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if node {
        return true;
    }
    // CI must never report a skipped acceptance run as a pass.
    assert!(
        std::env::var_os("CI").is_none(),
        "[ssr_shared_assets_e2e] node is required in CI"
    );
    eprintln!("[ssr_shared_assets_e2e] node unavailable; skipping.");
    false
}

struct Project {
    _temp: tempfile::TempDir,
    _node_modules: tempfile::TempDir,
    root: PathBuf,
}

impl Project {
    fn new(reshape: impl FnOnce(&Path)) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("project");
        copy_dir(
            &workspace_root().join("crates/zfb/tests/fixtures/ssr-shared-assets"),
            &root,
        );
        reshape(&root);
        let (nm_handle, node_modules) =
            zfb::render_pipeline::embedded_node_modules().expect("extract embedded node_modules");
        let scope = node_modules.join("@takazudo");
        fs::create_dir_all(&scope).unwrap();
        link_dir(
            &workspace_root().join("packages/zfb-adapter-cloudflare"),
            &scope.join("zfb-adapter-cloudflare"),
        )
        .unwrap();
        let bin_dir = node_modules.join(".bin");
        fs::create_dir_all(&bin_dir).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            "../@takazudo/zfb-adapter-cloudflare/bin/cli.mjs",
            bin_dir.join("zfb-adapter-cloudflare"),
        )
        .unwrap();
        #[cfg(windows)]
        fs::write(
            bin_dir.join("zfb-adapter-cloudflare.cmd"),
            "@node \"%~dp0\\..\\@takazudo\\zfb-adapter-cloudflare\\bin\\cli.mjs\" %*\r\n",
        )
        .unwrap();
        link_dir(&node_modules, &root.join("node_modules")).unwrap();
        Self {
            _temp: temp,
            _node_modules: nm_handle,
            root,
        }
    }

    fn write(&self, rel: &str, contents: &str) {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn build(&self) -> PathBuf {
        let output = Command::new(zfb_binary!())
            .arg("build")
            .current_dir(&self.root)
            .env_remove("ZFB_SCRATCH_DIR")
            .output()
            .expect("spawn zfb build");
        assert!(
            output.status.success(),
            "`zfb build` must exit 0\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let dist = self.root.join("dist");
        assert!(
            dist.join("_worker.js").is_file(),
            "adapter must emit _worker.js"
        );
        dist
    }
}

fn remove(root: &Path, rel: &str) {
    let path = root.join(rel);
    if path.is_dir() {
        fs::remove_dir_all(path).unwrap();
    } else {
        fs::remove_file(path).unwrap();
    }
}

fn set_base(root: &Path, base: &str) {
    fs::write(
        root.join("zfb.config.json"),
        format!(
            "{{\n  \"wind\": false,\n  \"base\": \"{base}\",\n  \"adapter\": \"@takazudo/zfb-adapter-cloudflare\"\n}}\n"
        ),
    )
    .unwrap();
}

/// Replace the island with a no-op so the build has no islands bundle.
fn without_islands(root: &Path) {
    for rel in [
        "components/counter.tsx",
        "components/lazy-panel.ts",
        "components/shared-state.ts",
    ] {
        remove(root, rel);
    }
    fs::write(
        root.join("components/island-slot.tsx"),
        "export function IslandSlot(_props: { label: string }) {\n  return null;\n}\n",
    )
    .unwrap();
}

fn without_client_scripts(root: &Path) {
    remove(root, "src");
}

fn without_css(root: &Path) {
    remove(root, "styles");
}

struct Driven {
    results: Value,
    extra: Value,
}

impl Driven {
    fn get(&self, path: &str) -> &Value {
        self.results
            .get(path)
            .unwrap_or_else(|| panic!("no driven result for {path}"))
    }

    fn body(&self, path: &str) -> &str {
        self.get(path)["body"].as_str().unwrap()
    }

    fn status(&self, path: &str) -> u64 {
        self.get(path)["status"].as_u64().unwrap()
    }
}

fn drive(dist: &Path, base: &str, paths: Option<&[&str]>, http_extras: bool) -> Driven {
    let mut command = Command::new("node");
    command
        .arg(workspace_root().join("tests/ssr-shared-assets/drive.mjs"))
        .arg(dist)
        .arg(base)
        .env(
            "SSR_SHARED_ASSETS_HTTP",
            if http_extras { "1" } else { "0" },
        );
    if let Some(paths) = paths {
        command.env(
            "SSR_SHARED_ASSETS_PATHS",
            serde_json::to_string(paths).unwrap(),
        );
    }
    let output = command.output().expect("spawn node driver");
    assert!(
        output.status.success(),
        "worker driver failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("driver JSON");
    Driven {
        results: json["results"].clone(),
        extra: json["extra"].clone(),
    }
}

/// Value of the first `attr="..."` after `needle` in `html`.
fn attr_after(html: &str, needle: &str, attr: &str) -> Option<String> {
    let at = html.find(needle)?;
    let rest = &html[at..];
    let key = format!("{attr}=\"");
    let start = rest.find(&key)? + key.len();
    let end = rest[start..].find('"')?;
    Some(rest[start..start + end].to_string())
}

fn css_url(html: &str) -> Option<String> {
    attr_after(html, "<link rel=\"stylesheet\"", "href")
}

fn script_srcs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find("<script type=\"module\" src=\"") {
        let start = at + "<script type=\"module\" src=\"".len();
        let end = rest[start..].find('"').unwrap();
        out.push(rest[start..start + end].to_string());
        rest = &rest[start + end..];
    }
    out
}

fn islands_url(html: &str) -> Option<String> {
    script_srcs(html)
        .into_iter()
        .find(|src| src.contains("/assets/islands-"))
}

fn greeting_url(html: &str) -> Option<String> {
    script_srcs(html)
        .into_iter()
        .find(|src| src.contains("/assets/client/greeting"))
}

fn build_token(html: &str) -> Option<String> {
    attr_after(html, "data-zfb-island=", "data-zfb-build")
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

fn header<'a>(result: &'a Value, name: &str) -> Option<&'a str> {
    result["headers"][name].as_str()
}

fn preserve(dist: &Path, name: &str) {
    let dest = workspace_root().join("target/ssr-shared-assets").join(name);
    if dest.exists() {
        fs::remove_dir_all(&dest).unwrap();
    }
    copy_dir(dist, &dest.join("dist"));
}

fn hashed_file_bytes(dist: &Path, url: &str, base: &str) -> Vec<u8> {
    let prefix = base.trim_end_matches('/');
    let rel = url
        .strip_prefix(prefix)
        .unwrap_or(url)
        .trim_start_matches('/');
    fs::read(dist.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

#[test]
fn root_build_shares_finalized_assets_between_ssg_and_ssr() {
    if !tools_available() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let project = Project::new(|_| {});
    let dist = project.build();
    let driven = drive(&dist, "", None, true);

    let ssg = fs::read_to_string(dist.join("index.html")).unwrap();
    let css = css_url(&ssg).expect("SSG page links the hashed stylesheet");
    let islands = islands_url(&ssg).expect("SSG page loads the islands entry");
    let greeting = greeting_url(&ssg).expect("SSG page loads the greeting script");
    assert!(
        css.starts_with("/assets/styles-") && css.ends_with(".css"),
        "{css}"
    );
    assert!(
        greeting.starts_with("/assets/client/greeting-"),
        "{greeting}"
    );
    let ssg_token = build_token(&ssg).expect("SSG island marker");

    // The record is embedded in the SSR bundle itself; nothing is looked up
    // at request time.
    let inner = fs::read_to_string(dist.join("_zfb_inner.mjs")).unwrap();
    assert!(
        inner.contains(&css) && inner.contains(&greeting),
        "record embedded"
    );

    // The asset-first wrapper serves the prerendered page from disk.
    assert_eq!(driven.body("/"), ssg, "GET / is the prerendered file");

    // A request-time page with the same Layout uses the same final URLs.
    let live = driven.body("/live");
    assert_eq!(driven.status("/live"), 200);
    assert_eq!(css_url(live).as_deref(), Some(css.as_str()), "{live}");
    assert_eq!(
        islands_url(live).as_deref(),
        Some(islands.as_str()),
        "{live}"
    );
    assert_eq!(
        greeting_url(live).as_deref(),
        Some(greeting.as_str()),
        "{live}"
    );
    assert!(
        !live.contains(STABLE_GREETING),
        "stable client URL resolved: {live}"
    );
    assert!(
        !live.contains("/assets/client/unused"),
        "unreferenced script not added: {live}"
    );
    assert_eq!(count(live, &format!("href=\"{css}\"")), 1, "{live}");
    assert_eq!(count(live, &format!("src=\"{islands}\"")), 1, "{live}");
    assert_eq!(
        build_token(live).as_deref(),
        Some(ssg_token.as_str()),
        "SSR HTML and SSG HTML share one build identity"
    );
    let islands_bytes = String::from_utf8(hashed_file_bytes(&dist, &islands, "")).unwrap();
    assert!(
        islands_bytes.contains(&ssg_token),
        "the browser islands bundle carries the same build identity"
    );

    // Shared CSS and islands without explicit script references.
    let plain = driven.body("/plain");
    assert_eq!(css_url(plain).as_deref(), Some(css.as_str()), "{plain}");
    assert_eq!(
        islands_url(plain).as_deref(),
        Some(islands.as_str()),
        "{plain}"
    );
    assert!(
        greeting_url(plain).is_none(),
        "no client script on /plain: {plain}"
    );

    // Catch-all SSR route without paths().
    let doc = driven.body("/docs/guide/intro");
    assert_eq!(driven.status("/docs/guide/intro"), 200);
    assert_eq!(css_url(doc).as_deref(), Some(css.as_str()), "{doc}");
    assert_eq!(
        greeting_url(doc).as_deref(),
        Some(greeting.as_str()),
        "{doc}"
    );

    // htmlPage: status and headers survive, the body is finished.
    let missing = driven.get("/missing");
    assert_eq!(missing["status"], 404);
    assert_eq!(header(missing, "cache-control"), Some("no-store"));
    assert_eq!(header(missing, "vary"), Some("Accept-Language"));
    assert_eq!(header(missing, "x-fixture"), Some("missing"));
    assert_eq!(
        missing["headers"]["set-cookie"],
        serde_json::json!(["first=1; Path=/", "second=2; Path=/"])
    );
    assert!(header(missing, "content-type")
        .unwrap()
        .starts_with("text/html"));
    let missing_body = missing["body"].as_str().unwrap();
    assert_eq!(
        css_url(missing_body).as_deref(),
        Some(css.as_str()),
        "{missing_body}"
    );

    // Unmanaged results are byte-for-byte untouched.
    assert_eq!(
        driven.body("/string"),
        "<!doctype html>\n<html><head><link rel=\"stylesheet\" href=\"/assets/styles.css\"></head><body>string</body></html>"
    );
    assert_eq!(
        driven.body("/feed.xml"),
        "<feed><link href=\"/assets/styles.css\" /></feed>"
    );
    assert_eq!(
        driven.body("/api/raw-html"),
        "<html><head><link rel=\"stylesheet\" href=\"/assets/styles.css\"></head><body>raw</body></html>"
    );
    assert_eq!(
        driven.body("/api/data"),
        "{\"stylesheet\":\"/assets/styles.css\"}"
    );
    assert_eq!(
        driven.get("/api/bytes")["bytes"],
        serde_json::json!([0, 255, 1, 254])
    );
    let moved = driven.get("/api/moved");
    assert_eq!(moved["status"], 302);
    assert_eq!(header(moved, "location"), Some("/live"));
    let empty = driven.get("/api/empty");
    assert_eq!(empty["status"], 204);
    assert_eq!(empty["body"], "");

    // `.html` pages stay verbatim.
    let verbatim = driven.body("/verbatim");
    assert!(
        verbatim.contains(&format!("href=\"{STABLE_CSS}\"")),
        "{verbatim}"
    );

    // Unknown routes fall through to the framework 404.
    assert_eq!(driven.status("/no-such-page"), 404);

    // HEAD never carries a body; non-GET goes straight to the router.
    assert_eq!(driven.extra["headLive"]["status"], 200);
    assert_eq!(driven.extra["headLive"]["body"], "");
    assert_eq!(driven.extra["headData"]["body"], "");
    let post = driven.extra["postLive"]["body"].as_str().unwrap();
    assert_eq!(css_url(post).as_deref(), Some(css.as_str()), "{post}");

    // Streams are passed through without being read ahead.
    assert_eq!(driven.extra["stream"]["first"], "first;");
    assert_eq!(driven.extra["stream"]["rest"], "second");

    // Lazy chunks never import the islands entry back.
    let entry_name = islands.rsplit('/').next().unwrap();
    for file in fs::read_dir(dist.join("assets")).unwrap() {
        let path = file.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with("islands-chunk-") {
            let chunk = fs::read_to_string(&path).unwrap();
            assert!(
                !chunk.contains(entry_name) && !chunk.contains("islands.js"),
                "{name} must not import the islands entry"
            );
        }
    }

    preserve(&dist, "root");
}

#[test]
fn subpath_build_resolves_base_prefixed_assets_once() {
    if !tools_available() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let project = Project::new(|root| set_base(root, "/sub/"));
    let dist = project.build();
    let driven = drive(&dist, "/sub/", Some(&["/", "/live", "/plain"]), false);

    let ssg = driven.body("/");
    let css = css_url(ssg).expect("SSG stylesheet");
    let greeting = greeting_url(ssg).expect("SSG greeting script");
    let islands = islands_url(ssg).expect("SSG islands entry");
    assert!(css.starts_with("/sub/assets/styles-"), "{css}");
    assert!(
        greeting.starts_with("/sub/assets/client/greeting-"),
        "{greeting}"
    );

    let live = driven.body("/live");
    assert_eq!(css_url(live).as_deref(), Some(css.as_str()), "{live}");
    assert_eq!(
        islands_url(live).as_deref(),
        Some(islands.as_str()),
        "{live}"
    );
    // The Layout calls clientScript() at module top level.
    assert_eq!(
        greeting_url(live).as_deref(),
        Some(greeting.as_str()),
        "{live}"
    );
    for html in [ssg, live, driven.body("/plain")] {
        assert!(!html.contains("/sub/sub/"), "base applied twice: {html}");
    }
    preserve(&dist, "subpath");
}

#[test]
fn ssr_only_catch_all_build_emits_assets_and_worker() {
    if !tools_available() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let project = Project::new(|root| {
        for rel in [
            "pages/index.tsx",
            "pages/live.tsx",
            "pages/plain.tsx",
            "pages/missing.tsx",
            "pages/string.tsx",
            "pages/feed.xml.tsx",
            "pages/api",
            "pages/verbatim.html",
        ] {
            remove(root, rel);
        }
    });
    let dist = project.build();
    assert!(
        !dist.join("index.html").exists(),
        "no SSG output in this variant"
    );
    let driven = drive(&dist, "", Some(&["/docs/a", "/docs/a/b"]), false);
    for path in ["/docs/a", "/docs/a/b"] {
        let html = driven.body(path);
        assert_eq!(driven.status(path), 200, "{html}");
        let css = css_url(html).expect("stylesheet on SSR-only page");
        assert!(dist.join(css.trim_start_matches('/')).is_file(), "{css}");
        let greeting = greeting_url(html).expect("explicit client script resolved");
        assert!(
            dist.join(greeting.trim_start_matches('/')).is_file(),
            "{greeting}"
        );
        let islands = islands_url(html).expect("islands entry on SSR-only page");
        assert!(
            dist.join(islands.trim_start_matches('/')).is_file(),
            "{islands}"
        );
    }
}

#[test]
fn asset_variants_only_add_what_was_shipped() {
    if !tools_available() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let paths: &[&str] = &["/live", "/plain"];

    // CSS only.
    let project = Project::new(|root| {
        without_islands(root);
        without_client_scripts(root);
    });
    let dist = project.build();
    let driven = drive(&dist, "", Some(paths), false);
    let plain = driven.body("/plain");
    assert!(css_url(plain).is_some(), "{plain}");
    assert!(script_srcs(plain).is_empty(), "{plain}");
    // An unknown stable reference is left alone; nothing is invented.
    let live = driven.body("/live");
    assert_eq!(
        greeting_url(live).as_deref(),
        Some(STABLE_GREETING),
        "{live}"
    );

    // Client scripts only.
    let project = Project::new(|root| {
        without_islands(root);
        without_css(root);
    });
    let dist = project.build();
    let driven = drive(&dist, "", Some(paths), false);
    let live = driven.body("/live");
    assert!(css_url(live).is_none(), "{live}");
    let greeting = greeting_url(live).expect("greeting resolved");
    assert!(
        greeting.starts_with("/assets/client/greeting-"),
        "{greeting}"
    );
    assert_eq!(script_srcs(live), vec![greeting.clone()], "{live}");
    assert!(script_srcs(driven.body("/plain")).is_empty());

    // Islands without CSS or client scripts.
    let project = Project::new(|root| {
        without_css(root);
        without_client_scripts(root);
    });
    let dist = project.build();
    let driven = drive(&dist, "", Some(paths), false);
    let plain = driven.body("/plain");
    assert!(css_url(plain).is_none(), "{plain}");
    assert!(islands_url(plain).is_some(), "{plain}");

    // No assets at all: pages are exactly what the router rendered before
    // this feature.
    let project = Project::new(|root| {
        without_islands(root);
        without_css(root);
        without_client_scripts(root);
    });
    let dist = project.build();
    let driven = drive(&dist, "", Some(paths), false);
    let plain = driven.body("/plain");
    assert!(
        css_url(plain).is_none() && script_srcs(plain).is_empty(),
        "{plain}"
    );
}

#[test]
fn rebuild_with_changed_bytes_moves_ssg_and_ssr_to_new_hashes() {
    if !tools_available() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let project = Project::new(|_| {});
    let paths: &[&str] = &["/", "/live"];
    let dist = project.build();
    let before = drive(&dist, "", Some(paths), false);
    let before_css = css_url(before.body("/")).unwrap();
    let before_greeting = greeting_url(before.body("/")).unwrap();
    let before_css_bytes = hashed_file_bytes(&dist, &before_css, "");

    project.write(
        "styles/global.css",
        "body {\n  margin: 0;\n}\n\n.shared-marker {\n  color: rgb(65, 43, 21);\n}\n",
    );
    project.write(
        "src/greeting.client.ts",
        "document.documentElement.dataset.greeting = \"ready-again\";\n",
    );
    let dist = project.build();
    let after = drive(&dist, "", Some(paths), false);
    let ssg = after.body("/");
    let live = after.body("/live");
    let css = css_url(ssg).unwrap();
    let greeting = greeting_url(ssg).unwrap();
    assert_ne!(css, before_css, "changed CSS bytes get a new hash");
    assert_ne!(
        greeting, before_greeting,
        "changed script bytes get a new hash"
    );
    assert_eq!(css_url(live).as_deref(), Some(css.as_str()), "{live}");
    assert_eq!(
        greeting_url(live).as_deref(),
        Some(greeting.as_str()),
        "{live}"
    );
    assert_ne!(
        hashed_file_bytes(&dist, &css, ""),
        before_css_bytes,
        "the new stylesheet bytes are shipped under the new hash"
    );
}
