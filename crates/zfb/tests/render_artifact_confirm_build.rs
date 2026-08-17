//! Issue #2427 (epic #2421, Wave 3 — CONFIRM), Test 1: the central
//! real-`zfb build` integration gate for the render-artifact export
//! feature. Waves 1-2 unit-tested the marker emitter (`content.ts`,
//! `render_md_page_shell`) and the extraction/writer
//! (`render_artifact.rs`) each against the pinned sentinel format in
//! isolation; this file is the first place both sides run together
//! through a real build, closing the blind spots the epic named:
//!
//! 1. Real-build integration of markers ↔ extractor.
//! 2. The direct `pages/*.md` metadata join surviving
//!    `resolveMarkdownLinks`-driven specifier hashing (untested until a
//!    real build computes the hash).
//! 3. Asset-URL rewriting / link-base rewriting leaving sentinels intact
//!    through steps 3.5/3.6, and the shipped asset URL landing inside
//!    the captured fragment in its hashed form.
//! 4. Final page HTML byte-identity flag-on vs flag-off, in both
//!    minify states; artifact bytes identical across minify states.
//!
//! ## Fixture shape
//!
//! - `docs` collection with two sibling entries (`guide`, `reference`),
//!   each a `## heading` + a `> [!NOTE]` GitHub alert (resolved through
//!   a project-root `mdx-components.tsx`, mirroring
//!   `crates/zfb/templates/basic-blog`) containing a markdown link to
//!   the project's own stable CSS URL (`/assets/styles.css`) — the
//!   `apply_prod_asset_pipeline` boundary-replace rewrites every
//!   occurrence of that literal string anywhere in the shipped HTML
//!   (`crates/zfb-build/src/pipeline/prod.rs`'s `boundary_replace`), so
//!   this is a real, in-content asset reference that gets hash-rewritten
//!   by the SAME pass the epic's fixed pipeline position runs before.
//! - `pages/docs/[slug].tsx` — one entry per route, exactly one
//!   `<entry.Content />` region each → each gets an artifact.
//! - `pages/index.tsx` — a listing page rendering BOTH entries'
//!   `<entry.Content />` — two top-level sibling regions → the
//!   deterministic multi-region warning, no artifact.
//! - `pages/about.md` — a direct markdown page (bypasses the `Content`
//!   bridge; wrapped by `render_md_page_shell` instead) → its own
//!   artifact, proving the direct-`.md` join.
//! - `pages/skip.mdx` — a direct MDX page. Its own compiled JSX module
//!   IS the route module (no shell seam `render_md_page_shell` can hook
//!   into), so the epic documents this as NOT instrumented: no
//!   sentinels, no artifact, no warning.
//!
//! ## Level / tier
//!
//! Level 4 (real `zfb build` process e2e). Self-skip convention (no
//! `#[ignore]`) — registered in nextest's `e2e-heavy` build-only group
//! in `.config/nextest.toml` per `crates/CLAUDE.md`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use zfb_test_utils::{locate_esbuild, zfb_binary};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// `true` when the non-zero build is a known-skip (no embedded V8 / no
/// esbuild), matching the pattern used across sibling build-command tests
/// (`html_minify_build.rs`, `end_to_end_basic_blog_build.rs`).
fn is_known_skip(combined: &str) -> bool {
    combined.contains("embed_v8")
        || combined.contains("no esbuild")
        || combined.contains("no tailwind")
        || (combined.contains("tailwindcss") && combined.contains("not found"))
}

fn run_zfb_build(root: &Path, esbuild: &Path) -> std::process::Output {
    Command::new(zfb_binary!())
        .arg("build")
        .current_dir(root)
        .env("ZFB_ESBUILD_BIN", esbuild)
        .output()
        .expect("spawn `zfb build`")
}

fn combined_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}{stderr}")
}

/// Recursively collect every regular file under `dir` into a
/// `BTreeMap<relative-path-string, file-bytes>`, mirroring
/// `build_package_routes_consumer.rs`'s `collect_all_files`.
fn collect_all_files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    if !dir.is_dir() {
        return out;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p
                    .strip_prefix(dir)
                    .expect("strip_prefix")
                    .to_string_lossy()
                    .replace('\\', "/");
                let bytes = fs::read(&p).unwrap_or_default();
                out.insert(rel, bytes);
            }
        }
    }
    out
}

/// Extract the top-level JSON keys of a `serde_json::to_string_pretty`
/// document IN DECLARATION ORDER, by scanning each line's leading quoted
/// token. Mirrors the technique `render_artifact.rs`'s own unit tests use
/// to pin field order (`single_region_writes_the_pinned_artifact...`) —
/// pretty-printed HTML-bearing string values never contain a literal
/// newline (SSR output is single-line), so each key's line is never
/// confused with fragment content.
fn json_key_order(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|l| l.trim().strip_prefix('"'))
        .filter_map(|l| l.split('"').next())
        .map(str::to_string)
        .collect()
}

const PINNED_FIELD_ORDER: [&str; 8] = [
    "contractVersion",
    "route",
    "fragmentHtml",
    "headings",
    "depth",
    "text",
    "slug",
    "sourceDigest",
];

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

fn write_fixture(root: &Path, emit_render_artifacts: bool, minify_html: bool) {
    fs::write(
        root.join("zfb.config.json"),
        format!(
            r#"{{
  "framework": "preact",
  "minifyHtml": {minify_html},
  "emitRenderArtifacts": {emit_render_artifacts},
  "collections": [{{ "name": "docs", "path": "content/docs" }}],
  "markdown": {{ "features": {{ "githubAlerts": true }} }}
}}
"#
        ),
    )
    .unwrap();

    // A project-root `mdx-components.tsx` (issue #616 convention) so the
    // `> [!NOTE]` alert resolves to a real component carrying
    // `data-component="note"` — the same hook
    // `crates/zfb/templates/basic-blog/components/callout.tsx` stamps —
    // which is the proof the fragment captured POST-SSR expansion rather
    // than the opaque pre-SSR `MdxJsxFlowElement`.
    fs::write(
        root.join("mdx-components.tsx"),
        r#"export default {
  Note({ children }) {
    return <aside data-component="note">{children}</aside>;
  },
};
"#,
    )
    .unwrap();

    // A plain (non-Tailwind) global stylesheet is enough to arm the CSS
    // emitter slot — same fixture shape as `html_minify_build.rs`.
    fs::create_dir_all(root.join("styles")).unwrap();
    fs::write(root.join("styles/global.css"), "body { color: #1a2b3c; }\n").unwrap();

    fs::create_dir_all(root.join("content/docs")).unwrap();
    for (name, other) in [("guide", "reference"), ("reference", "guide")] {
        fs::write(
            root.join(format!("content/docs/{name}.md")),
            format!(
                "---\ntitle: {name} doc\n---\n\n\
                 ## {name} usage\n\n\
                 > [!NOTE]\n\
                 > See the [stylesheet](/assets/styles.css) and the {other} doc.\n\n\
                 Body text for {name}.\n"
            ),
        )
        .unwrap();
    }

    fs::create_dir_all(root.join("pages/docs")).unwrap();
    fs::write(
        root.join("pages/docs/[slug].tsx"),
        r#"export async function paths() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const entries = await getCollection("docs");
  return entries.map((entry) => ({ params: { slug: entry.slug }, props: { entry } }));
}

export default function DocPage({ entry }) {
  return (
    <html lang="en">
      <head>
        <title>{entry.data.title}</title>
      </head>
      <body>
        <main>
          <entry.Content />
        </main>
      </body>
    </html>
  );
}
"#,
    )
    .unwrap();

    // Listing page: renders BOTH entries' `<entry.Content />` — two
    // top-level sibling regions on one route.
    fs::write(
        root.join("pages/index.tsx"),
        r#"export async function getStaticProps() {
  const { getCollection } = await import("@takazudo/zfb/content");
  const entries = await getCollection("docs");
  return { props: { entries } };
}

export default function Home({ entries }) {
  return (
    <html lang="en">
      <head>
        <title>Docs listing</title>
      </head>
      <body>
        <main>
          {entries.map((entry) => (
            <article key={entry.slug}>
              <entry.Content />
            </article>
          ))}
        </main>
      </body>
    </html>
  );
}
"#,
    )
    .unwrap();

    // Direct `pages/*.md` route — bypasses the `Content` bridge, wrapped
    // by `render_md_page_shell` instead (epic #2421).
    fs::write(
        root.join("pages/about.md"),
        "---\ntitle: About\n---\n\n## About heading\n\nDirect markdown page body.\n",
    )
    .unwrap();

    // Direct `pages/*.mdx` route — the documented exclusion: its own
    // compiled JSX module IS the page, so no wrapping seam exists.
    fs::write(
        root.join("pages/skip.mdx"),
        "## Skip heading\n\nA direct MDX page route; no render-artifact instrumentation applies here.\n",
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Test A — contract shape, region rule, sentinel stripping, exclusion
// ---------------------------------------------------------------------------

#[test]
fn render_artifact_contract_and_region_rules_on_real_build() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[render_artifact_confirm] no esbuild; skipping.");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    // minify OFF here so the assertions below (heading id substrings,
    // fragment text) don't have to account for whitespace collapsing —
    // the minify x flag matrix is Test B's job.
    write_fixture(root, true, false);

    let output = run_zfb_build(root, &esbuild);
    let combined = combined_output(&output);
    if !output.status.success() {
        if is_known_skip(&combined) {
            eprintln!("[render_artifact_confirm] known-skip indicator; skipping.\n{combined}");
            return;
        }
        panic!(
            "zfb build failed unexpectedly.\nstatus={:?}\n{combined}",
            output.status
        );
    }

    let dist = root.join("dist");
    let dist_files = collect_all_files(&dist);

    // --- per-entry artifacts (guide, reference) ---
    for name in ["guide", "reference"] {
        let artifact_path = dist.join(format!("__zfb/render/docs/{name}/index.json"));
        let raw = fs::read_to_string(&artifact_path).unwrap_or_else(|e| {
            panic!("read artifact for `{name}` at {artifact_path:?}: {e}\ndist: {dist_files:#?}")
        });
        assert!(
            raw.ends_with('\n'),
            "`{name}` artifact must end with a newline"
        );

        assert_eq!(
            json_key_order(&raw),
            PINNED_FIELD_ORDER,
            "`{name}` artifact must carry the exact pinned field order; raw:\n{raw}"
        );

        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(json["contractVersion"], 1);
        assert_eq!(json["route"], format!("/docs/{name}"));

        let fragment = json["fragmentHtml"].as_str().unwrap();
        assert!(
            fragment.contains("data-component=\"note\""),
            "`{name}` fragment must contain the expanded alert component markup \
             (proof of post-SSR capture, not the opaque pre-SSR JSX node): {fragment}"
        );
        assert!(
            !fragment.contains("data-zfb-render-region"),
            "`{name}` fragment must contain no sentinel: {fragment}"
        );
        assert!(
            fragment.contains("/assets/styles-") && !fragment.contains("/assets/styles.css"),
            "`{name}` fragment's in-content asset reference must be the hashed shipped form: {fragment}"
        );

        let source_digest = json["sourceDigest"].as_str().unwrap();
        assert!(
            source_digest.starts_with("sha256:") && source_digest.len() == "sha256:".len() + 64,
            "`{name}` sourceDigest must be `sha256:<64-hex>`: {source_digest}"
        );

        // headings[].slug must match the id the SAME build's compiler
        // stamped onto the rendered heading in the shipped page.
        let slug = json["headings"][0]["slug"].as_str().unwrap();
        let page = fs::read_to_string(dist.join(format!("docs/{name}/index.html"))).unwrap();
        assert!(
            page.contains(&format!("id=\"{slug}\"")),
            "`{name}` artifact heading slug `{slug}` must match a rendered heading id in the shipped page:\n{page}"
        );
        assert!(
            !page.contains("data-zfb-render-region"),
            "`{name}` shipped page must contain no sentinel:\n{page}"
        );
    }

    // --- direct pages/*.md route: about ---
    let about_artifact = dist.join("__zfb/render/about/index.json");
    let about_raw = fs::read_to_string(&about_artifact)
        .unwrap_or_else(|e| panic!("read about artifact: {e}\ndist: {dist_files:#?}"));
    let about_json: serde_json::Value = serde_json::from_str(&about_raw).unwrap();
    assert_eq!(about_json["route"], "/about");
    let about_fragment = about_json["fragmentHtml"].as_str().unwrap();
    assert!(
        about_fragment.contains("About heading"),
        "direct pages/*.md artifact must carry the page's own content: {about_fragment}"
    );
    let about_slug = about_json["headings"][0]["slug"].as_str().unwrap();
    let about_page = fs::read_to_string(dist.join("about/index.html")).unwrap();
    assert!(
        about_page.contains(&format!("id=\"{about_slug}\"")),
        "direct pages/*.md heading slug must match the rendered heading id: slug={about_slug}\n{about_page}"
    );

    // --- multi-region listing page: no artifact, deterministic warning ---
    assert!(
        !dist.join("__zfb/render/index.json").exists(),
        "listing page with 2 sibling regions must get no artifact"
    );
    let expected_warning = "render artifacts: / renders 2 top-level content regions; \
         an artifact is written only for a route with exactly one \
         (see `emitRenderArtifacts`)";
    assert_eq!(
        combined.matches(expected_warning).count(),
        1,
        "expected exactly one deterministic multi-region warning naming the route; got:\n{combined}"
    );

    // --- direct pages/*.mdx: documented exclusion, no artifact, no noise ---
    assert!(
        !dist.join("__zfb/render/skip/index.json").exists(),
        "direct pages/*.mdx must never get an artifact"
    );
    assert!(
        !combined.contains("/skip") && !combined.contains("skip.mdx"),
        "direct pages/*.mdx must produce no render-artifact warning noise at all:\n{combined}"
    );
    let skip_page = fs::read_to_string(dist.join("skip/index.html")).unwrap();
    assert!(
        !skip_page.contains("data-zfb-render-region"),
        "direct pages/*.mdx page must never carry a sentinel (none is ever emitted for it):\n{skip_page}"
    );

    // --- no sentinel survives anywhere in the shipped dist tree ---
    for (rel, bytes) in &dist_files {
        let text = String::from_utf8_lossy(bytes);
        assert!(
            !text.contains("data-zfb-render-region"),
            "sentinel leaked into shipped output at `{rel}`"
        );
    }
}

// ---------------------------------------------------------------------------
// Test B — flag on/off byte identity, in both minify states
// ---------------------------------------------------------------------------

#[test]
fn render_artifact_flag_on_vs_off_byte_identity_across_minify_states() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[render_artifact_confirm] no esbuild; skipping byte-identity test.");
        return;
    };

    let mut trees: BTreeMap<(bool, bool), BTreeMap<String, Vec<u8>>> = BTreeMap::new();
    for emit in [false, true] {
        for minify in [false, true] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path();
            write_fixture(root, emit, minify);

            let output = run_zfb_build(root, &esbuild);
            let combined = combined_output(&output);
            if !output.status.success() {
                if is_known_skip(&combined) {
                    eprintln!(
                        "[render_artifact_confirm] known-skip for emit={emit} minify={minify}; \
                         skipping the whole byte-identity test.\n{combined}"
                    );
                    return;
                }
                panic!(
                    "zfb build failed for emit={emit} minify={minify}.\nstatus={:?}\n{combined}",
                    output.status
                );
            }

            let dist = root.join("dist");
            let render_dir = dist.join("__zfb/render");
            if emit {
                assert!(
                    render_dir.is_dir(),
                    "flag ON (minify={minify}) must write dist/__zfb/render/"
                );
            } else {
                assert!(
                    !render_dir.exists(),
                    "flag OFF (minify={minify}) must never write dist/__zfb/render/"
                );
            }
            trees.insert((emit, minify), collect_all_files(&dist));
        }
    }

    // Page HTML byte-identical flag ON vs OFF, at each minify state.
    for minify in [false, true] {
        let on = &trees[&(true, minify)];
        let off = &trees[&(false, minify)];
        let on_pages: BTreeMap<&String, &Vec<u8>> = on
            .iter()
            .filter(|(k, _)| !k.starts_with("__zfb/render/"))
            .collect();
        let off_pages: BTreeMap<&String, &Vec<u8>> = off.iter().collect();

        let on_keys: Vec<&&String> = on_pages.keys().collect();
        let off_keys: Vec<&&String> = off_pages.keys().collect();
        assert_eq!(
            on_keys, off_keys,
            "flag on/off page sets must match at minify={minify}"
        );

        let mismatches: Vec<&String> = on_pages
            .iter()
            .filter(|(rel, bytes)| off_pages.get(**rel) != Some(bytes))
            .map(|(rel, _)| *rel)
            .collect();
        assert!(
            mismatches.is_empty(),
            "flag on/off page HTML must be byte-identical at minify={minify}; mismatches: {mismatches:#?}"
        );
    }

    // Artifact bytes identical across minify states (extraction always
    // runs BEFORE minification, so the minify flag cannot perturb it).
    let render_on: BTreeMap<&String, &Vec<u8>> = trees[&(true, true)]
        .iter()
        .filter(|(k, _)| k.starts_with("__zfb/render/"))
        .collect();
    let render_off: BTreeMap<&String, &Vec<u8>> = trees[&(true, false)]
        .iter()
        .filter(|(k, _)| k.starts_with("__zfb/render/"))
        .collect();
    let on_keys: Vec<&&String> = render_on.keys().collect();
    let off_keys: Vec<&&String> = render_off.keys().collect();
    assert_eq!(
        on_keys, off_keys,
        "render-artifact file sets must match across minify states"
    );
    let mismatches: Vec<&String> = render_on
        .iter()
        .filter(|(rel, bytes)| render_off.get(**rel) != Some(bytes))
        .map(|(rel, _)| *rel)
        .collect();
    assert!(
        mismatches.is_empty(),
        "render-artifact bytes must be identical across minify states; mismatches: {mismatches:#?}"
    );

    eprintln!(
        "[render_artifact_confirm] byte-identity holds across {} artifact file(s) and both minify states.",
        render_on.len()
    );
}
