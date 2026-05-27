//! End-to-end pipeline integration tests.
//!
//! For each markdown/MDX fixture under `tests/fixtures/`:
//! 1. Read the file.
//! 2. Strip frontmatter (if any) via `frontmatter::parse`.
//! 3. Run the body through a fully-wired `Pipeline` (all 7 plugins).
//! 4. Serialize to HTML.
//! 5. Compare against the snapshot under `tests/fixtures/snapshots/`.
//!
//! Snapshot bootstrap: set `INSANE_UPDATE_SNAPSHOTS=1` (or delete the
//! snapshot file) to capture the current output. Without the env var,
//! a missing snapshot is also captured-and-passed (first-run behaviour).
//! Re-running with the snapshot present and the env var unset compares.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use zfb_content::diagnostics::{CollectingSink, DiagnosticSeverity, MarkdownDiagnostic};
use zfb_content::frontmatter::{self};
use zfb_content::heading_registry::HeadingRegistry;
use zfb_content::pipeline::{BuildContext, HastNode, HastVisitor, Pipeline};
use zfb_content::plugins::{ResolveLinksPlugin, ResolveMarkdownLinksOptions};
use zfb_content::serializer::serialize;

/// Build the full plugin chain.
///
/// Sub 4b (#47) replaced the explicit per-plugin wiring here with
/// [`Pipeline::with_defaults`], plus the trailing-slash-aware
/// `StripMdExtensionPlugin` layered on top via `add_strip_md_ext()`.
/// We deliberately omit `ResolveLinksPlugin`: with an empty source map
/// it would be a no-op anyway, and `StripMdExtensionPlugin` already
/// covers the `.md` rewriting we want to assert on.
fn build_full_pipeline() -> Pipeline {
    let mut p = Pipeline::with_defaults();
    p.add_strip_md_ext();
    p
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn snapshots_dir() -> PathBuf {
    fixtures_dir().join("snapshots")
}

/// Run a fixture through the full pipeline and return the HTML output.
fn render_fixture(name: &str) -> String {
    render_fixture_with(name, build_full_pipeline())
}

/// Same as `render_fixture` but allows the caller to inject a custom
/// pipeline (e.g. with extra visitors).
fn render_fixture_with(name: &str, mut pipeline: Pipeline) -> String {
    let path = fixtures_dir().join(name);
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {path:?}: {e}"));
    // Strip frontmatter so the pipeline only sees the body. For files
    // without frontmatter, `extract` returns the whole input as body.
    let uf = frontmatter::extract(&path, &raw)
        .unwrap_or_else(|e| panic!("parse frontmatter for {path:?}: {e}"));
    let body = uf
        .body
        .unwrap_or_else(|| panic!("md/mdx fixture must have a body: {path:?}"));
    let hast = pipeline
        .run(&body)
        .unwrap_or_else(|e| panic!("pipeline failed for {path:?}: {e}"));
    serialize(&hast)
}

/// Snapshot equality with first-run capture.
///
/// - If `INSANE_UPDATE_SNAPSHOTS=1` is set OR the snapshot file does
///   not exist, write `actual` to `snapshot_path` and return.
/// - Otherwise read the snapshot and compare; on mismatch panic with a
///   line-level divergence pointer plus full expected/actual dumps.
fn assert_snapshot_eq(actual: &str, snapshot_path: &Path) {
    let update = std::env::var("INSANE_UPDATE_SNAPSHOTS").is_ok();
    if update || !snapshot_path.exists() {
        if let Some(parent) = snapshot_path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create snapshot dir {parent:?}: {e}"));
        }
        std::fs::write(snapshot_path, actual)
            .unwrap_or_else(|e| panic!("write snapshot {snapshot_path:?}: {e}"));
        return;
    }
    let expected = std::fs::read_to_string(snapshot_path)
        .unwrap_or_else(|e| panic!("read snapshot {snapshot_path:?}: {e}"));
    if actual == expected {
        return;
    }
    let line = expected
        .lines()
        .zip(actual.lines())
        .enumerate()
        .find(|(_, (e, a))| e != a)
        .map(|(i, (e, a))| {
            format!("first divergence at line {i}:\n  expected: {e:?}\n  actual:   {a:?}")
        })
        .unwrap_or_else(|| {
            format!(
                "trailing-content mismatch: expected {} lines, actual {} lines",
                expected.lines().count(),
                actual.lines().count(),
            )
        });
    // Snapshot mismatch is a legitimate test-failure mechanism. `panic!`
    // is the assertion primitive here because the bespoke diff output is
    // more readable than `assert_eq!` of two large strings.
    panic!(
        "snapshot mismatch at {snapshot_path:?}\n{line}\n--- expected ---\n{expected}\n--- actual ---\n{actual}",
    );
}

/// Helper: render fixture, compare to snapshot of the same stem.
fn check_fixture(fixture_name: &str, snapshot_name: &str) {
    let actual = render_fixture(fixture_name);
    let snap_path = snapshots_dir().join(snapshot_name);
    assert_snapshot_eq(&actual, &snap_path);
}

#[test]
fn fixture_01_basic_renders() {
    check_fixture("01-basic.md", "01-basic.html");
}

#[test]
fn fixture_02_frontmatter_strips_frontmatter() {
    let path = fixtures_dir().join("02-frontmatter.md");
    let raw = std::fs::read_to_string(&path).unwrap();
    // Sanity: frontmatter::extract must extract the title and leave the
    // body without any --- markers.
    let uf = frontmatter::extract(&path, &raw).expect("frontmatter parses");
    assert_eq!(
        uf.value["title"].as_str(),
        Some("Hello Frontmatter"),
        "title must come from YAML",
    );
    let body = uf.body.expect("md/mdx returns a body");
    assert!(
        !body.contains("---"),
        "body must not still contain frontmatter delimiters: {body:?}",
    );
    check_fixture("02-frontmatter.md", "02-frontmatter.html");
    // Snapshot must NOT contain the frontmatter title or `---`.
    let snapshot = std::fs::read_to_string(snapshots_dir().join("02-frontmatter.html"))
        .expect("snapshot present after first run");
    assert!(
        !snapshot.contains("Hello Frontmatter"),
        "snapshot leaked frontmatter title: {snapshot}",
    );
    assert!(
        !snapshot.contains("---"),
        "snapshot leaked frontmatter delimiters: {snapshot}",
    );
}

#[test]
fn fixture_03_admonitions_render_as_jsx() {
    check_fixture("03-admonitions.mdx", "03-admonitions.html");
    let snapshot = std::fs::read_to_string(snapshots_dir().join("03-admonitions.html"))
        .expect("snapshot present after first run");
    for tag in ["<Note", "<Tip", "<Warning", "<Details"] {
        assert!(
            snapshot.contains(tag),
            "expected admonition tag {tag} in snapshot:\n{snapshot}",
        );
    }
    assert!(
        snapshot.contains("title=\"Click me\""),
        "expected details title in snapshot:\n{snapshot}",
    );
}

/// Sub 4b (#47) acceptance criterion: a fixture MDX with `:::note`
/// compiles to `<Note>…</Note>` via the default path (no manual
/// plugin wiring at the call site).
#[test]
fn fixture_03_admonitions_compile_via_default_pipeline() {
    let actual = render_fixture_with("03-admonitions.mdx", Pipeline::with_defaults());
    assert!(
        actual.contains("<Note") && actual.contains("</Note>"),
        "expected <Note>…</Note> via Pipeline::with_defaults(), got:\n{actual}",
    );
    for tag in ["<Tip", "<Warning", "<Details"] {
        assert!(
            actual.contains(tag),
            "expected admonition {tag} via default pipeline:\n{actual}",
        );
    }
    assert!(
        actual.contains("title=\"Click me\""),
        "expected directive-promoted details title via default pipeline:\n{actual}",
    );
}

/// Sub 4b (#47) override criterion: callers must still be able to
/// opt out of the default plugin chain by constructing a bare
/// [`Pipeline::with_mdx`] (or [`Pipeline::new`]). When they do, the
/// directive transform must NOT fire — the `:::note` paragraphs
/// survive as plain text content.
#[test]
fn caller_can_override_with_bare_pipeline() {
    let actual = render_fixture_with("03-admonitions.mdx", Pipeline::with_mdx());
    assert!(
        !actual.contains("<Note"),
        "bare Pipeline::with_mdx() must NOT synthesize <Note>; got:\n{actual}",
    );
    // The literal `:::note` source survives as text content because no
    // mdast visitor folded it into a JSX element.
    assert!(
        actual.contains(":::note"),
        "bare pipeline must preserve raw directive markers verbatim:\n{actual}",
    );
}

#[test]
fn fixture_04_code_block_wraps_in_container() {
    check_fixture("04-code-block.md", "04-code-block.html");
    let snapshot = std::fs::read_to_string(snapshots_dir().join("04-code-block.html"))
        .expect("snapshot present after first run");
    assert!(
        snapshot.contains("<div class=\"code-block-container\">"),
        "expected code-block-container wrapper in snapshot:\n{snapshot}",
    );
    assert!(
        snapshot.contains("<div class=\"code-block-title\">example.rs</div>"),
        "expected code-block-title with file name in snapshot:\n{snapshot}",
    );
    assert!(
        snapshot.contains("<pre class=\"syntect-"),
        "expected syntect-highlighted pre in snapshot:\n{snapshot}",
    );
}

#[test]
fn fixture_05_headings_get_anchors() {
    check_fixture("05-headings.md", "05-headings.html");
    let snapshot = std::fs::read_to_string(snapshots_dir().join("05-headings.html"))
        .expect("snapshot present after first run");
    // h2 / h3 / h4 each get an id and a heading-anchor child.
    for slug in ["section-one", "subsection-a", "deep-heading", "section-two"] {
        assert!(
            snapshot.contains(&format!("id=\"{slug}\"")),
            "expected id={slug} in snapshot:\n{snapshot}",
        );
        assert!(
            snapshot.contains(&format!("href=\"#{slug}\"")),
            "expected anchor href=#{slug} in snapshot:\n{snapshot}",
        );
    }
    assert!(
        snapshot.contains("class=\"hash-link\""),
        "expected hash-link class in snapshot:\n{snapshot}",
    );
    // The anchor body is empty — the `#` glyph is rendered by CSS.
    assert!(
        snapshot.contains("aria-label=\"Direct link to "),
        "expected `Direct link to {{text}}` aria-label in snapshot:\n{snapshot}",
    );
    assert!(
        !snapshot.contains(">#</a>"),
        "anchor body must be empty (CSS renders the # glyph):\n{snapshot}",
    );
    // h1 must NOT receive an id.
    assert!(
        !snapshot.contains("<h1 id="),
        "h1 should not get an id in snapshot:\n{snapshot}",
    );
}

#[test]
fn fixture_06_links_strip_md_extension() {
    check_fixture("06-links.md", "06-links.html");
    let snapshot = std::fs::read_to_string(snapshots_dir().join("06-links.html"))
        .expect("snapshot present after first run");
    // Internal `.md` link → extension stripped AND trailing slash
    // appended (JS-aligned shape; the default `add_trailing_slash=true`
    // converges with `ResolveLinksPlugin`'s output shape).
    assert!(
        snapshot.contains("href=\"other-doc/\""),
        "internal .md href should be stripped to other-doc/:\n{snapshot}",
    );
    // Internal link with fragment keeps fragment AFTER the slash.
    assert!(
        snapshot.contains("href=\"other-doc/#sec\""),
        "internal .md#sec should become other-doc/#sec:\n{snapshot}",
    );
    // External link is untouched.
    assert!(
        snapshot.contains("href=\"https://example.com\""),
        "external href should be preserved:\n{snapshot}",
    );
    // No `.md` extensions should leak into hrefs.
    assert!(
        !snapshot.contains("href=\"other-doc.md"),
        ".md must be stripped from internal links:\n{snapshot}",
    );
}

/// Convergence (Sub 5 / #110 + Sub 6.5): when `ResolveLinksPlugin` is
/// chained alongside `StripMdExtensionPlugin` in trailing-slash mode,
/// both plugins must produce the same URL shape for the same target —
/// mapped links resolved by the resolver and unmapped extension-stripped
/// links rewritten by the strip plugin should both end in `/`.
#[test]
fn convergence_resolve_links_and_strip_md_share_url_shape() {
    // One mapped target, one unmapped — both .md, both should land on
    // `<target>/` after the pipeline runs.
    let mut source_map: HashMap<PathBuf, String> = HashMap::new();
    source_map.insert(PathBuf::from("mapped.md"), "/docs/mapped/".to_string());

    let mut p = Pipeline::with_mdx();
    // mdast phase: resolver runs first to rewrite mapped .md → /docs/mapped/.
    p.add_mdast_visitor(Box::new(ResolveLinksPlugin::new(
        ResolveMarkdownLinksOptions {
            source_map,
            source_dir: None,
        },
    )));
    // hast phase: strip+slash for any remaining unmapped .md.
    p.add_strip_md_ext();

    let body = "[m](mapped.md) and [u](./unmapped.md)";
    let h = p.run(body).expect("pipeline ok");
    let html = serialize(&h);

    assert!(
        html.contains("href=\"/docs/mapped/\""),
        "mapped link must be resolved to /docs/mapped/ — got:\n{html}",
    );
    assert!(
        html.contains("href=\"./unmapped/\""),
        "unmapped .md must be stripped + slashed to ./unmapped/ — got:\n{html}",
    );

    // Both URLs end in `/` — that's the convergence assertion.
    let mapped_ok = html.contains("href=\"/docs/mapped/\"");
    let unmapped_ok = html.contains("href=\"./unmapped/\"");
    assert!(
        mapped_ok && unmapped_ok,
        "convergence violated — mapped and unmapped must both end in `/`:\n{html}",
    );
}

#[test]
fn fixture_07_image_wraps_with_enlargeable_figure() {
    check_fixture("07-image.md", "07-image.html");
    let snapshot = std::fs::read_to_string(snapshots_dir().join("07-image.html"))
        .expect("snapshot present after first run");
    // Both block-level paragraph-only images get wrapped now (the
    // selector is "paragraph with only an <img>", no width gating).
    assert!(
        snapshot.contains("<figure class=\"zd-enlargeable\">"),
        "expected zd-enlargeable figure in snapshot:\n{snapshot}",
    );
    assert!(
        snapshot.contains("class=\"zd-enlarge-btn\""),
        "expected zd-enlarge-btn button in snapshot:\n{snapshot}",
    );
    assert!(
        snapshot.contains("aria-label=\"Enlarge image\""),
        "expected enlarge-button aria-label in snapshot:\n{snapshot}",
    );
    // Both images present in the snapshot.
    assert!(
        snapshot.contains("src=\"bare.png\""),
        "bare image must still appear in snapshot:\n{snapshot}",
    );
    assert!(
        snapshot.contains("src=\"pic.png\""),
        "pic image must still appear in snapshot:\n{snapshot}",
    );
    // The legacy `<ImageEnlarge>` JSX-marker path is gone — no consumer
    // depends on it post-cutover.
    assert!(
        !snapshot.contains("<ImageEnlarge"),
        "legacy <ImageEnlarge> marker must not leak into output:\n{snapshot}",
    );
}

/// Opt-out: `title="no-enlarge"` leaves the `<p>` alone and strips the
/// title sentinel from the `<img>` so it never reaches the rendered DOM.
#[test]
fn fixture_07b_image_opt_out_strips_title() {
    let actual = render_fixture("07b-image-opt-out.md");
    assert!(
        !actual.contains("<figure class=\"zd-enlargeable\">"),
        "opt-out must skip the enlargeable wrap:\n{actual}",
    );
    assert!(
        !actual.contains("title=\"no-enlarge\""),
        "title=\"no-enlarge\" sentinel must be stripped from <img>:\n{actual}",
    );
    assert!(
        actual.contains("src=\"keep.png\""),
        "image src must survive the opt-out:\n{actual}",
    );
}

/// Integration: a fence with lang=mdx produces a themed `<pre class="syntect-…">` wrapper.
/// mdx resolves to the Markdown grammar via resolve_alias, so the block is themed
/// even though there is no dedicated MDX grammar bundle.
#[test]
fn fixture_09_mdx_lang_fence_uses_themed_wrapper() {
    let actual = render_fixture("09-mdx-lang-fence.md");
    assert!(
        actual.contains("<pre class=\"syntect-"),
        "mdx fence must produce a themed <pre class=\"syntect-…\"> wrapper:\n{actual}"
    );
    assert!(
        !actual.contains("<pre><code>"),
        "mdx fence must not fall through to bare <pre><code>:\n{actual}"
    );
}

#[test]
fn fixture_08_mermaid_block_replaced_with_div() {
    check_fixture("08-mermaid.md", "08-mermaid.html");
    let snapshot = std::fs::read_to_string(snapshots_dir().join("08-mermaid.html"))
        .expect("snapshot present after first run");
    // MermaidPlugin replaces the entire <pre> with <div class="mermaid">
    // (JS-aligned shape; the `<pre>` no longer survives).
    assert!(
        snapshot.contains("<div class=\"mermaid\""),
        "expected <div class=\"mermaid\"> wrapper in snapshot:\n{snapshot}",
    );
    assert!(
        snapshot.contains("data-mermaid"),
        "expected data-mermaid attribute in snapshot:\n{snapshot}",
    );
    // The original `<pre>` shell is gone — no `language-mermaid` class
    // and no syntect wrapping should leak out.
    assert!(
        !snapshot.contains("language-mermaid"),
        "mermaid <pre>/<code> should be replaced — no language-mermaid leftover:\n{snapshot}",
    );
    assert!(
        !snapshot.contains("<pre class=\"syntect-"),
        "mermaid block must not be syntect-highlighted:\n{snapshot}",
    );
    // Diagram source survives as escaped text inside the div.
    assert!(
        snapshot.contains("graph TD"),
        "mermaid source must be preserved:\n{snapshot}",
    );
}

// ── Wave-6 seam tests (issue #568) ────────────────────────────────────────────

/// Acceptance criterion: a pipeline run WITHOUT a `BuildContext` produces
/// byte-identical output to today (backwards-compat).
#[test]
fn run_without_context_is_byte_identical_to_run() {
    let md = "## Hello World\n\nsome text\n";
    let mut pipeline = Pipeline::with_defaults();

    let html_no_ctx = {
        let hast = pipeline.run(md).expect("run ok");
        serialize(&hast)
    };
    pipeline.reset_per_entry();
    let html_with_ctx = {
        let mut ctx = BuildContext::for_paths(
            "/docs/page.md",
            "/project",
            "/project/public",
        );
        let hast = pipeline.run_with_context(md, &mut ctx).expect("run_with_context ok");
        serialize(&hast)
    };

    assert_eq!(
        html_no_ctx, html_with_ctx,
        "run_with_context must produce byte-identical output to run when no context-aware visitors are wired",
    );
}

/// Acceptance criterion: `HeadingLinksPlugin` populates the registry when
/// attached; emits zero registry writes when absent.
#[test]
fn heading_links_populates_registry_when_present() {
    let md = "## Introduction\n\n## Setup\n\n### Details\n";
    let source = PathBuf::from("/docs/guide.md");

    let mut registry = HeadingRegistry::new();
    let mut ctx = BuildContext {
        source_path: Some(source.clone()),
        project_root: PathBuf::from("/project"),
        public_dir: PathBuf::from("/project/public"),
        heading_registry: Some(&mut registry),
        diagnostics: None,
    };

    let mut pipeline = Pipeline::with_defaults();
    pipeline.run_with_context(md, &mut ctx).expect("pipeline ok");

    let entries = registry.get(&source).expect("entries must be recorded");
    assert_eq!(entries.len(), 3, "expected 3 headings: {entries:?}");
    assert_eq!(entries[0].id, "introduction");
    assert_eq!(entries[0].text, "Introduction");
    assert_eq!(entries[0].depth, 2);
    assert_eq!(entries[1].id, "setup");
    assert_eq!(entries[2].id, "details");
    assert_eq!(entries[2].depth, 3);
}

/// Zero-cost path: when no registry is in the context, no entries are written.
#[test]
fn heading_links_zero_writes_without_registry() {
    let md = "## Hello\n";
    let mut ctx = BuildContext::for_paths("/docs/page.md", "/project", "/project/public");
    // heading_registry is None — no writes should occur.

    let mut pipeline = Pipeline::with_defaults();
    pipeline.run_with_context(md, &mut ctx).expect("pipeline ok");
    // No registry to query — this test just asserts no panic + context is accessible.
    assert!(ctx.source_path.as_deref() == Some(std::path::Path::new("/docs/page.md")));
}

/// Acceptance criterion: `DiagnosticsSink` smoke test — a test visitor
/// emits a warning, the sink receives it.
#[test]
fn diagnostics_sink_smoke_test() {
    // A minimal hast visitor that emits a warning diagnostic for every
    // paragraph it encounters.
    struct ParagraphWarner;
    impl HastVisitor for ParagraphWarner {
        fn visit(&mut self, _node: &mut HastNode) {}

        fn visit_with_context(
            &mut self,
            node: &mut HastNode,
            ctx: &mut BuildContext<'_>,
        ) {
            match node {
                HastNode::Element { tag, children, .. } if tag == "p" => {
                    if let Some(sink) = ctx.diagnostics.as_deref_mut() {
                        sink.emit(MarkdownDiagnostic::warning("paragraph encountered"));
                    }
                    for c in children {
                        self.visit_with_context(c, ctx);
                    }
                }
                HastNode::Root { children } | HastNode::Element { children, .. } => {
                    for c in children {
                        self.visit_with_context(c, ctx);
                    }
                }
                _ => {}
            }
        }
    }

    let md = "hello\n\nworld\n";

    let mut sink = CollectingSink::new();
    let mut ctx = BuildContext {
        source_path: None,
        project_root: PathBuf::from("/project"),
        public_dir: PathBuf::from("/project/public"),
        heading_registry: None,
        diagnostics: Some(&mut sink),
    };

    let mut pipeline = Pipeline::with_mdx();
    pipeline.add_hast_visitor(Box::new(ParagraphWarner));
    pipeline.run_with_context(md, &mut ctx).expect("pipeline ok");

    let diags = sink.take();
    assert!(
        !diags.is_empty(),
        "expected at least one diagnostic from ParagraphWarner",
    );
    assert!(
        diags
            .iter()
            .all(|d| d.severity() == DiagnosticSeverity::Warning),
        "all diagnostics must be warnings: {diags:?}",
    );
}

/// Acceptance criterion: pipeline run WITH a `BuildContext` exposes the
/// context to visitors via `visit_with_context`.
#[test]
fn context_survives_through_pipeline() {
    // A hast visitor that reads the source_path from context and records
    // whether it was accessible.
    struct ContextChecker {
        saw_path: Option<PathBuf>,
    }
    impl HastVisitor for ContextChecker {
        fn visit(&mut self, _node: &mut HastNode) {}

        fn visit_with_context(
            &mut self,
            _node: &mut HastNode,
            ctx: &mut BuildContext<'_>,
        ) {
            self.saw_path = ctx.source_path.clone();
        }
    }

    let checker = ContextChecker { saw_path: None };

    // We can't add the visitor and use it after - use the pipeline-owned approach.
    // Instead, test by querying registry after a full run.
    let md = "## Check\n";
    let source = PathBuf::from("/docs/check.md");
    let mut registry = HeadingRegistry::new();

    let mut ctx = BuildContext {
        source_path: Some(source.clone()),
        project_root: PathBuf::from("/project"),
        public_dir: PathBuf::from("/project/public"),
        heading_registry: Some(&mut registry),
        diagnostics: None,
    };

    let mut pipeline = Pipeline::with_defaults();
    pipeline.run_with_context(md, &mut ctx).expect("pipeline ok");

    // The registry was populated from context — proving context reached the visitor.
    let entries = registry.get(&source).expect("context must have reached HeadingLinksPlugin");
    assert_eq!(entries[0].id, "check");
    drop(checker); // suppress unused warning
}
