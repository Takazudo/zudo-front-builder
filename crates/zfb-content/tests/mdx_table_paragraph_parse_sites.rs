//! Explicit MDX table normalisation (zfb#4232) at every parse site.
//!
//! `mdx_jsx_emit_hast.rs` / `mdx_jsx_emit.rs` cover the MDX compile parse
//! site. The same `table > p` shape reaches the HTML render path through
//! `Pipeline::run` / `run_with_context`, and a transcluded file through
//! `TranscludePlugin`'s own secondary parse — each `to_mdast` call site owns
//! its output, so each needs the pass. The collapsed-directive re-parse is
//! covered at the mdast level in `plugins::directives`'s own tests.

use std::collections::HashMap;

use zfb_content::pipeline::{Pipeline, ResolvedGfmConstructs};
use zfb_md_ast::{BuildContext, DirectiveSpec, MarkdownFeaturesConfig, TranscludeConfig};

#[path = "support/table_tags.rs"]
mod table_tags;
use table_tags::{assert_no_structure_paragraph, table_tag_sequence};

const TABLE_REPRO: &str =
    "<table>\n<thead><tr><th>A</th></tr></thead>\n<tbody>\n<tr><td>B</td></tr>\n</tbody>\n</table>\n";

fn pipeline_with_transclude() -> Pipeline {
    let mut directives = HashMap::new();
    directives.insert("note".to_string(), DirectiveSpec::Short("Note".to_string()));
    let features = MarkdownFeaturesConfig {
        transclude: Some(TranscludeConfig::default()),
        directives: Some(directives),
        ..Default::default()
    };
    Pipeline::with_defaults_and_full_config(
        None,
        ResolvedGfmConstructs::CONSERVATIVE,
        None,
        false,
        false,
        Some(&features),
    )
    .expect("pipeline builds")
}

fn tmpdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("mdx_table_paragraph")
        .tempdir()
        .expect("tempdir")
}

fn include_snippet(dir: &std::path::Path, body: &str) -> String {
    std::fs::write(dir.join("snippet.md"), body).expect("write snippet");
    r#":::include{file="./snippet.md"}"#.to_string()
}

fn render_with_context(p: &mut Pipeline, dir: &std::path::Path, input: &str) -> String {
    let mut ctx = BuildContext {
        source_path: Some(dir.join("page.md")),
        project_root: dir.to_path_buf(),
        public_dir: dir.join("public"),
        heading_registry: None,
        diagnostics: None,
        cross_file_links: None,
    };
    let hast = p.run_with_context(input, &mut ctx).expect("render ok");
    zfb_content::serializer::serialize(&hast)
}

/// The HTML path reconstructs the JSX as markup, so the repro's sections
/// must come out directly nested, with no `<p>` anywhere.
fn assert_html_table_unwrapped(html: &str) {
    assert!(
        html.contains("<thead><tr><th>A</th></tr></thead>"),
        "thead must hold its row directly:\n{html}"
    );
    assert!(
        html.contains("<tbody><tr><td>B</td></tr></tbody>"),
        "tbody must hold its row directly:\n{html}"
    );
    assert!(
        !html.contains("<p>"),
        "no paragraph may wrap table structure:\n{html}"
    );
}

#[test]
fn html_run_path_unwraps_explicit_table_paragraphs() {
    let hast = Pipeline::new().run(TABLE_REPRO).expect("render ok");
    assert_html_table_unwrapped(&zfb_content::serializer::serialize(&hast));
}

#[test]
fn html_run_with_context_path_unwraps_explicit_table_paragraphs() {
    let dir = tmpdir();
    let html = render_with_context(&mut Pipeline::new(), dir.path(), TABLE_REPRO);
    assert_html_table_unwrapped(&html);
}

#[test]
fn transcluded_explicit_table_is_unwrapped_on_the_html_path() {
    let dir = tmpdir();
    let include = include_snippet(dir.path(), TABLE_REPRO);
    let html = render_with_context(&mut pipeline_with_transclude(), dir.path(), &include);
    assert_html_table_unwrapped(&html);
}

#[test]
fn transcluded_explicit_table_is_unwrapped_on_the_jsx_path() {
    let dir = tmpdir();
    let include = include_snippet(dir.path(), TABLE_REPRO);
    let mut p = pipeline_with_transclude();
    p.set_build_context_roots(dir.path().to_path_buf(), dir.path().join("public"));
    let opts = zfb_content::MdxJsxOptions::default()
        .with_filename("page.mdx".to_string())
        .with_source_path(dir.path().join("page.mdx"));
    let out =
        zfb_content::mdx_to_jsx_module_with_pipeline(&include, opts, &mut p).expect("compile ok");

    let seq = table_tag_sequence(&out);
    assert_no_structure_paragraph(&seq, &out);
    assert_eq!(
        seq,
        ["table", "thead", "tr", "th", "tbody", "tr", "td"],
        "{out}"
    );
}
