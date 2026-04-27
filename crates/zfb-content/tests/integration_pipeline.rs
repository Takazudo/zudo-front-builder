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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use zfb_content::frontmatter::{self};
use zfb_content::pipeline::{HastNode, HastVisitor, Pipeline};
use zfb_content::plugins::{
    AdmonitionsPlugin, CodeTitlePlugin, HeadingLinksPlugin, ImageEnlargePlugin, MermaidPlugin,
    StripMdExtensionPlugin, SyntectPlugin,
};
use zfb_content::serializer::serialize;
use zfb_content::syntect_highlight::Highlighter;

/// Build the full plugin chain.
///
/// We deliberately omit `ResolveLinksPlugin` here: with an empty source
/// map it would be a no-op anyway, and `StripMdExtensionPlugin` already
/// covers the `.md` rewriting we want to assert on. Keeping it out keeps
/// the snapshots stable.
fn build_full_pipeline() -> Pipeline {
    let mut p = Pipeline::with_mdx();
    p.add_mdast_visitor(Box::new(AdmonitionsPlugin::new()));
    p.add_hast_visitor(Box::new(HeadingLinksPlugin::new()));
    p.add_hast_visitor(Box::new(CodeTitlePlugin::new()));
    p.add_hast_visitor(Box::new(ImageEnlargePlugin::new()));
    p.add_hast_visitor(Box::new(MermaidPlugin::new()));
    p.add_hast_visitor(Box::new(StripMdExtensionPlugin::new()));
    p.add_hast_visitor(Box::new(SyntectPlugin::new(Arc::new(Highlighter::new()))));
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

/// Test-only hast visitor: walks the tree and tags every `<img>` that
/// has `alt="hero"` with `width="400"` and `height="300"`.
///
/// This is the only realistic way to feed a width-bearing `<img>`
/// element into `ImageEnlargePlugin` from a markdown fixture: the
/// markdown image syntax (`![alt](src)`) doesn't accept dimensions,
/// and raw `<img>` HTML embedded in MDX rides through as
/// `HastNode::Raw`, which `ImageEnlargePlugin` (rightly) ignores.
struct InjectHeroDimensions;

impl HastVisitor for InjectHeroDimensions {
    fn visit(&mut self, node: &mut HastNode) {
        if let HastNode::Element { tag, attrs, .. } = node {
            if tag == "img" {
                let is_hero = attrs.iter().any(|(k, v)| k == "alt" && v == "hero");
                if is_hero {
                    if !attrs.iter().any(|(k, _)| k == "width") {
                        attrs.push(("width".to_string(), "400".to_string()));
                    }
                    if !attrs.iter().any(|(k, _)| k == "height") {
                        attrs.push(("height".to_string(), "300".to_string()));
                    }
                }
            }
        }
        match node {
            HastNode::Root { children } | HastNode::Element { children, .. } => {
                for c in children {
                    self.visit(c);
                }
            }
            _ => {}
        }
    }
}

/// Pipeline used by the image fixture: same as the default chain but
/// with `InjectHeroDimensions` inserted BEFORE `ImageEnlargePlugin` so
/// the latter has a width attribute to act on.
fn build_image_pipeline() -> Pipeline {
    let mut p = Pipeline::with_mdx();
    p.add_mdast_visitor(Box::new(AdmonitionsPlugin::new()));
    p.add_hast_visitor(Box::new(HeadingLinksPlugin::new()));
    p.add_hast_visitor(Box::new(CodeTitlePlugin::new()));
    // Inject width/height onto the `hero` img before ImageEnlargePlugin
    // gets to look at it.
    p.add_hast_visitor(Box::new(InjectHeroDimensions));
    p.add_hast_visitor(Box::new(ImageEnlargePlugin::new()));
    p.add_hast_visitor(Box::new(MermaidPlugin::new()));
    p.add_hast_visitor(Box::new(StripMdExtensionPlugin::new()));
    p.add_hast_visitor(Box::new(SyntectPlugin::new(Arc::new(Highlighter::new()))));
    p
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

#[test]
fn fixture_04_code_block_wraps_in_figure() {
    check_fixture("04-code-block.md", "04-code-block.html");
    let snapshot = std::fs::read_to_string(snapshots_dir().join("04-code-block.html"))
        .expect("snapshot present after first run");
    assert!(
        snapshot.contains("<figure class=\"code-figure\">"),
        "expected code-figure wrapper in snapshot:\n{snapshot}",
    );
    assert!(
        snapshot.contains("<figcaption>example.rs</figcaption>"),
        "expected figcaption with title in snapshot:\n{snapshot}",
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
        snapshot.contains("class=\"heading-anchor\""),
        "expected heading-anchor class in snapshot:\n{snapshot}",
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
    // Internal `.md` link → extension stripped.
    assert!(
        snapshot.contains("href=\"other-doc\""),
        "internal .md href should be stripped to other-doc:\n{snapshot}",
    );
    // Internal link with fragment keeps fragment.
    assert!(
        snapshot.contains("href=\"other-doc#sec\""),
        "internal .md#sec should become other-doc#sec:\n{snapshot}",
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

#[test]
fn fixture_07_image_wraps_with_image_enlarge() {
    let actual = render_fixture_with("07-image.md", build_image_pipeline());
    let snap_path = snapshots_dir().join("07-image.html");
    assert_snapshot_eq(&actual, &snap_path);
    let snapshot = std::fs::read_to_string(&snap_path).expect("snapshot present after first run");
    // The hero image with width should be wrapped in an ImageEnlarge marker.
    assert!(
        snapshot.contains("<ImageEnlarge"),
        "expected ImageEnlarge marker in snapshot:\n{snapshot}",
    );
    assert!(
        snapshot.contains("width=\"400\""),
        "expected width=400 preserved on ImageEnlarge:\n{snapshot}",
    );
    // The bare image (no width) should remain a plain <img …/>.
    assert!(
        snapshot.contains("src=\"bare.png\""),
        "bare image should be preserved as plain <img>:\n{snapshot}",
    );
    // And the bare image must NOT be wrapped (the fixture proves the
    // plugin is selective).
    let bare_count = snapshot.matches("src=\"bare.png\"").count();
    let bare_in_enlarge = snapshot.matches("<ImageEnlarge src=\"bare.png\"").count();
    assert_eq!(
        bare_in_enlarge, 0,
        "bare image must not be wrapped in ImageEnlarge: {snapshot}",
    );
    assert_eq!(bare_count, 1, "bare image must appear exactly once");
}

#[test]
fn fixture_08_mermaid_block_is_marked() {
    check_fixture("08-mermaid.md", "08-mermaid.html");
    let snapshot = std::fs::read_to_string(snapshots_dir().join("08-mermaid.html"))
        .expect("snapshot present after first run");
    // MermaidPlugin must have flagged the <pre>.
    assert!(
        snapshot.contains("data-mermaid=\"true\""),
        "expected data-mermaid attribute in snapshot:\n{snapshot}",
    );
    assert!(
        snapshot.contains("language-mermaid"),
        "expected class=language-mermaid in snapshot:\n{snapshot}",
    );
    // Mermaid body must NOT be syntect-wrapped.
    assert!(
        !snapshot.contains("<pre class=\"syntect-"),
        "mermaid block must not be syntect-highlighted:\n{snapshot}",
    );
    // Original mermaid source must survive (escaped).
    assert!(
        snapshot.contains("graph TD"),
        "mermaid source must be preserved:\n{snapshot}",
    );
}
