//! Native (rlib) integration tests for the `parseToAst` export (zfb#1857,
//! epic zfb#1854).
//!
//! Test plan (declared per project testing discipline):
//! - **Level**: 1 (unit/logic). `parse_to_ast` is a pure `&str → String`
//!   transform; every assertion is a JSON comparison against in-memory
//!   output on the host triple — same rationale as `tests/api.rs`.
//! - **Why level 1**: the position-shift/UTF-16-conversion arithmetic and
//!   the raw-mdast contract shape are pure logic. The wasm boundary itself
//!   is exercised at level 3 by the npm package's `test/parse-to-ast.test.ts`
//!   against the built `dist/` artifact (including the remark-parity proof,
//!   which needs a real JS remark to run against).
//! - **What's covered**: (a) the review-mandated RECURSIVE WHOLE-TREE
//!   position proof — every node in the returned AST carries `position`,
//!   with lines shifted by the frontmatter line count and offsets/columns
//!   in the export's real units (ASCII fixture: UTF-16 == bytes, so the
//!   byte-arithmetic proof below doubles as a UTF-16 proof; the dedicated
//!   non-ASCII fixture below proves the UTF-16 conversion itself), verified
//!   node-by-node against an independently parsed unshifted body tree AND
//!   by slice-equality against the original source bytes; (b) contract-shape
//!   fixtures: MDX (JSX elements + expressions + ESM), directive-style
//!   `:::note` text (stays RAW — no zfb visitor runs), custom-node-ish
//!   GitHub-alert blockquote (stays a plain blockquote — pre-visitor
//!   contract), a `<CustomComponent>` MDX-JSX element surviving with
//!   type/name/attributes intact (requirement 3 of zfb#1828); (c)
//!   `_markdownRsStops` absolute BYTE offsets shifted in lock-step with
//!   positions (never UTF-16-converted, by design); (d) no-frontmatter
//!   passthrough (zero shift); (e) parse errors surface as `"markdown"`
//!   diagnostics with frontmatter-shifted lines, same arithmetic as
//!   `renderHtml`; (f) the UTF-16 code-unit pin on a CJK+emoji source
//!   (surrogate pairs discriminate UTF-16 units from code points from
//!   bytes).
//! - **Blind spots**: executing the compiled wasm artifact (npm package
//!   scope); remark-parity (needs real JS remark — npm package scope);
//!   benchmark characteristics (the bench harness under `npm/test/bench/`
//!   measures, it does not assert).

#![cfg(feature = "pipeline")]

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

fn parse(result: String) -> Value {
    serde_json::from_str(&result).expect("API output is always a JSON document")
}

/// The non-frozen fixture corpus shared with the npm package's
/// `test/parse-to-ast.test.ts` (`highlight-parity/`'s manifest pattern —
/// unlike the frozen `tests/fixtures/parity/` corpus, this one has no
/// committed "expected" oracle: each fixture is asserted structurally, not
/// byte-compared against another renderer).
#[derive(Debug, Deserialize)]
struct FixtureManifest {
    fixtures: Vec<FixtureEntry>,
}

#[derive(Debug, Deserialize)]
struct FixtureEntry {
    slug: String,
    source: String,
    options: Value,
}

fn load_fixture(slug: &str) -> FixtureEntry {
    let manifest_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parse-to-ast/manifest.json");
    let text = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", manifest_path.display()));
    let manifest: FixtureManifest = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parsing {}: {e}", manifest_path.display()));
    manifest
        .fixtures
        .into_iter()
        .find(|f| f.slug == slug)
        .unwrap_or_else(|| panic!("no fixture named `{slug}` in {}", manifest_path.display()))
}

/// Frontmatter'd fixture exercising a wide construct spread: headings,
/// emphasis/strong/inline code, list, GFM table + strikethrough, fenced
/// code, blockquote, link/image, JSX flow element with an expression
/// attribute, MDX flow + text expressions.
const POSITION_FIXTURE: &str = "---\n\
title: Position proof\n\
tags:\n\
\x20 - ast\n\
---\n\
\n\
# Heading one\n\
\n\
Some *emphasis*, **strong**, `inline()` and ~~gone~~ text.\n\
\n\
- first item\n\
- second item with [a link](https://example.com/)\n\
\n\
| col a | col b |\n\
| ----- | ----- |\n\
| 1     | 2     |\n\
\n\
> quoted line\n\
\n\
```rust\n\
fn main() {}\n\
```\n\
\n\
<Card label=\"pinned\" count={6 * 7}>\n\
\x20 inner {1 + 2} text\n\
</Card>\n\
\n\
{2 + 3}\n\
\n\
![alt text](https://example.com/img.png)\n";

const MDX_OPTIONS: &str = r#"{"filename":"post.mdx"}"#;

/// The known position-less object shapes in markdown-rs's serde output:
/// JSX attributes and their expression values serialize with a `type` tag
/// but carry no `position` (markdown-rs does not model attribute
/// positions — recorded as contract data for the Wave-2 decision).
const POSITIONLESS_TYPES: &[&str] = &[
    "mdxJsxAttribute",
    "mdxJsxAttributeValueExpression",
    "mdxJsxExpressionAttribute",
];

/// Recursive whole-tree proof (review-corrected: spot checks are
/// insufficient). Walks the shifted AST and the independently parsed
/// unshifted body AST in parallel and asserts, for EVERY node:
/// - the shifted node carries `position` whenever the raw node does (and
///   every real mdast node does — checked separately below);
/// - `line = raw.line + prefix_lines`, `offset = raw.offset + body_offset`,
///   `column` unchanged, for both `start` and `end`;
/// - slice-equality: `source[start.offset..end.offset]` equals
///   `body[raw.start.offset..raw.end.offset]` — an independent anchor into
///   the original source bytes, not just delta arithmetic;
/// - `_markdownRsStops` second elements shifted by `body_offset`;
/// - everything else is byte-identical between the two trees.
fn assert_shifted_tree(
    shifted: &Value,
    raw: &Value,
    source: &str,
    body: &str,
    prefix_lines: u64,
    body_offset: u64,
    path: &str,
) {
    match (shifted, raw) {
        (Value::Object(s), Value::Object(r)) => {
            let s_keys: Vec<_> = s.keys().collect();
            let r_keys: Vec<_> = r.keys().collect();
            assert_eq!(s_keys, r_keys, "key sets diverge at {path}");
            for (key, s_val) in s {
                let r_val = &r[key];
                match key.as_str() {
                    "position" => {
                        assert_shifted_position(
                            s_val,
                            r_val,
                            source,
                            body,
                            prefix_lines,
                            body_offset,
                            path,
                        );
                    }
                    "_markdownRsStops" => {
                        let s_stops = s_val.as_array().expect("stops are an array");
                        let r_stops = r_val.as_array().expect("stops are an array");
                        assert_eq!(s_stops.len(), r_stops.len(), "stop counts at {path}");
                        for (s_stop, r_stop) in s_stops.iter().zip(r_stops) {
                            assert_eq!(
                                s_stop[0], r_stop[0],
                                "stop value-relative index unchanged at {path}"
                            );
                            assert_eq!(
                                s_stop[1].as_u64().unwrap(),
                                r_stop[1].as_u64().unwrap() + body_offset,
                                "stop absolute offset shifted by body offset at {path}"
                            );
                        }
                    }
                    _ => assert_shifted_tree(
                        s_val,
                        r_val,
                        source,
                        body,
                        prefix_lines,
                        body_offset,
                        &format!("{path}.{key}"),
                    ),
                }
            }
        }
        (Value::Array(s), Value::Array(r)) => {
            assert_eq!(s.len(), r.len(), "array lengths diverge at {path}");
            for (i, (s_val, r_val)) in s.iter().zip(r).enumerate() {
                assert_shifted_tree(
                    s_val,
                    r_val,
                    source,
                    body,
                    prefix_lines,
                    body_offset,
                    &format!("{path}[{i}]"),
                );
            }
        }
        _ => assert_eq!(shifted, raw, "non-position values diverge at {path}"),
    }
}

fn assert_shifted_position(
    shifted: &Value,
    raw: &Value,
    source: &str,
    body: &str,
    prefix_lines: u64,
    body_offset: u64,
    path: &str,
) {
    for point in ["start", "end"] {
        let s = &shifted[point];
        let r = &raw[point];
        assert_eq!(
            s["line"].as_u64().unwrap(),
            r["line"].as_u64().unwrap() + prefix_lines,
            "{path}.position.{point}.line shifted by frontmatter line count"
        );
        assert_eq!(
            s["offset"].as_u64().unwrap(),
            r["offset"].as_u64().unwrap() + body_offset,
            "{path}.position.{point}.offset shifted by body byte offset"
        );
        assert_eq!(
            s["column"], r["column"],
            "{path}.position.{point}.column unchanged"
        );
    }
    let s_start = shifted["start"]["offset"].as_u64().unwrap() as usize;
    let s_end = shifted["end"]["offset"].as_u64().unwrap() as usize;
    let r_start = raw["start"]["offset"].as_u64().unwrap() as usize;
    let r_end = raw["end"]["offset"].as_u64().unwrap() as usize;
    assert_eq!(
        &source[s_start..s_end],
        &body[r_start..r_end],
        "{path}: shifted offsets must slice the original source to the same bytes"
    );
}

/// Every real mdast node (any object with a `type` outside the known
/// position-less attribute shapes) must carry `position`.
fn assert_every_node_has_position(value: &Value, path: &str) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(node_type)) = map.get("type") {
                if !POSITIONLESS_TYPES.contains(&node_type.as_str()) {
                    assert!(
                        map.get("position").is_some_and(|p| p.is_object()),
                        "node `{node_type}` at {path} carries no position"
                    );
                }
            }
            for (key, nested) in map {
                assert_every_node_has_position(nested, &format!("{path}.{key}"));
            }
        }
        Value::Array(items) => {
            for (i, nested) in items.iter().enumerate() {
                assert_every_node_has_position(nested, &format!("{path}[{i}]"));
            }
        }
        _ => {}
    }
}

fn count_nodes_of_type(value: &Value, node_type: &str) -> usize {
    match value {
        Value::Object(map) => {
            let own = usize::from(map.get("type").and_then(Value::as_str) == Some(node_type));
            own + map
                .values()
                .map(|v| count_nodes_of_type(v, node_type))
                .sum::<usize>()
        }
        Value::Array(items) => items
            .iter()
            .map(|v| count_nodes_of_type(v, node_type))
            .sum(),
        _ => 0,
    }
}

#[test]
fn whole_tree_positions_are_shifted_on_frontmatter_fixture() {
    let out = parse(zfb_md_wasm::parse_to_ast(POSITION_FIXTURE, MDX_OPTIONS));
    assert_eq!(
        out["diagnostics"].as_array().unwrap().len(),
        0,
        "fixture parses clean: {:?}",
        out["diagnostics"]
    );
    assert_eq!(out["frontmatter"]["title"], "Position proof");
    let ast = &out["ast"];
    assert_eq!(ast["type"], "root");

    // Independent unshifted twin: strip the frontmatter the same way the
    // export does, then raw-parse the body directly through the facade.
    let extracted = zfb_content::frontmatter::extract_from_filename("post.mdx", POSITION_FIXTURE)
        .expect("fixture frontmatter extracts");
    let body = extracted.body.expect("mdx always yields a body");
    let body_offset = extracted
        .body_offset
        .expect("mdx always yields a body offset") as u64;
    let prefix_lines = POSITION_FIXTURE[..body_offset as usize]
        .matches('\n')
        .count() as u64;
    assert!(prefix_lines > 0, "fixture must actually have frontmatter");
    let raw =
        zfb_content::facade::parse_mdast(&zfb_content::facade::PipelineOptions::default(), &body)
            .expect("body parses");
    let raw_json = serde_json::to_value(&raw).expect("mdast serializes");

    // Guard the fixture breadth: the construct spread must actually be in
    // the tree, or the whole-tree proof proves less than it claims.
    for required in [
        "heading",
        "emphasis",
        "strong",
        "inlineCode",
        "delete",
        "list",
        "link",
        "table",
        "blockquote",
        "code",
        "mdxJsxFlowElement",
        "mdxFlowExpression",
        "mdxTextExpression",
        "image",
        "text",
    ] {
        assert!(
            count_nodes_of_type(ast, required) > 0,
            "position fixture must contain a `{required}` node"
        );
    }

    assert_every_node_has_position(ast, "ast");
    assert_shifted_tree(
        ast,
        &raw_json,
        POSITION_FIXTURE,
        &body,
        prefix_lines,
        body_offset,
        "ast",
    );
}

#[test]
fn no_frontmatter_source_is_returned_unshifted() {
    let source = "# Plain\n\nNo frontmatter here.\n";
    let out = parse(zfb_md_wasm::parse_to_ast(source, MDX_OPTIONS));
    assert_eq!(out["frontmatter"], Value::Null);
    let raw =
        zfb_content::facade::parse_mdast(&zfb_content::facade::PipelineOptions::default(), source)
            .expect("source parses");
    let raw_json = serde_json::to_value(&raw).expect("mdast serializes");
    assert_eq!(
        out["ast"], raw_json,
        "zero frontmatter means zero shift — byte-identical trees"
    );
}

#[test]
fn contract_shape_mdx_nodes_and_stops_are_shifted() {
    let source = "---\ntitle: Stops\n---\n\nBefore.\n\n{40 + 2}\n";
    let out = parse(zfb_md_wasm::parse_to_ast(source, MDX_OPTIONS));
    let ast = &out["ast"];
    let expression = &ast["children"][1];
    assert_eq!(expression["type"], "mdxFlowExpression");
    assert_eq!(expression["value"], "40 + 2");
    let brace_offset = source.find('{').unwrap() as u64;
    assert_eq!(
        expression["position"]["start"]["offset"].as_u64().unwrap(),
        brace_offset,
        "expression start offset points at the `{{` in the ORIGINAL source"
    );
    let stops = expression["_markdownRsStops"].as_array().unwrap();
    assert!(!stops.is_empty(), "flow expression carries stops");
    // Stop = (index_in_value, absolute_source_offset): index 0 of the value
    // ("40 + 2") sits one byte past the `{` in the original source.
    assert_eq!(stops[0][0].as_u64().unwrap(), 0);
    assert_eq!(stops[0][1].as_u64().unwrap(), brace_offset + 1);
}

#[test]
fn contract_shape_directive_and_alert_text_stays_raw() {
    // `:::note` directives and `> [!NOTE]` alerts are zfb VISITOR features.
    // The locked contract is RAW parser mdast (pre-visitors), so both must
    // come back as plain markdown shapes — no custom/synthesized nodes.
    let source = ":::note\ndirective body\n:::\n\n> [!NOTE]\n> alert body\n";
    let out = parse(zfb_md_wasm::parse_to_ast(source, MDX_OPTIONS));
    assert_eq!(out["diagnostics"].as_array().unwrap().len(), 0);
    let ast = &out["ast"];
    let children = ast["children"].as_array().unwrap();
    assert_eq!(
        children[0]["type"], "paragraph",
        "`:::note` stays a raw paragraph pre-visitors"
    );
    assert_eq!(
        children[1]["type"], "blockquote",
        "`> [!NOTE]` stays a raw blockquote pre-visitors"
    );
    let quoted_text = &children[1]["children"][0]["children"][0];
    assert_eq!(quoted_text["type"], "text");
    assert!(
        quoted_text["value"]
            .as_str()
            .unwrap()
            .starts_with("[!NOTE]"),
        "alert marker text survives verbatim: {quoted_text:?}"
    );
}

#[test]
fn gfm_toggles_flow_through_options() {
    let source = "a ~~strike~~ b\n";
    let on = parse(zfb_md_wasm::parse_to_ast(source, MDX_OPTIONS));
    assert!(count_nodes_of_type(&on["ast"], "delete") > 0);
    let off = parse(zfb_md_wasm::parse_to_ast(
        source,
        r#"{"filename":"post.mdx","pipeline":{"gfm":{"strikethrough":false}}}"#,
    ));
    assert_eq!(count_nodes_of_type(&off["ast"], "delete"), 0);
}

#[test]
fn parse_error_diagnostic_line_is_frontmatter_shifted() {
    // 3 frontmatter lines before the body. An OPENED-but-never-closed JSX
    // element makes markdown-rs fail at end of file (body line 3 → original
    // line 6). Note a merely INCOMPLETE tag (`<Card` with no `>`) is NOT a
    // parse error at the raw-mdast stage — markdown-rs degrades it to text;
    // same lenience as `to_mdast` everywhere else.
    let source = "---\ntitle: Err\n---\n\n<Card>\n";
    let out = parse(zfb_md_wasm::parse_to_ast(source, MDX_OPTIONS));
    assert_eq!(out["ast"], Value::Null);
    assert_eq!(out["frontmatter"]["title"], "Err");
    let diag = &out["diagnostics"][0];
    assert_eq!(diag["source"], "markdown");
    assert!(
        diag["message"]
            .as_str()
            .is_some_and(|m| m.contains("closing tag")),
        "unclosed-element parse error expected: {diag:?}"
    );
    assert_eq!(
        diag["line"].as_u64(),
        Some(6),
        "parse-error line points into the ORIGINAL source (eof after line 5's newline): {diag:?}"
    );
}

#[test]
fn options_document_is_shared_with_the_other_tiers() {
    // jsxRuntime/development are compile-tier knobs; parseToAst accepts and
    // ignores them so one options document serves every tier (same contract
    // as renderHtml). Unknown fields still fail fast.
    let out = parse(zfb_md_wasm::parse_to_ast(
        "# ok\n",
        r#"{"filename":"a.mdx","jsxRuntime":"react","development":true}"#,
    ));
    assert_eq!(out["ast"]["type"], "root");
    let bad = parse(zfb_md_wasm::parse_to_ast("# ok\n", r#"{"nope":1}"#));
    assert_eq!(bad["ast"], Value::Null);
    assert_eq!(bad["diagnostics"][0]["source"], "options");
}

#[test]
fn eof_terminated_frontmatter_shifts_first_line_columns_too() {
    // Self-review finding (P3): with the closing `---` at EOF (no trailing
    // newline) the extracted EMPTY body starts right after the delimiter on
    // the SAME line — so first-body-line columns must shift by the
    // body-start column, or the returned column disagrees with the offset
    // beside it. `---\ntitle: x\n---` → body "" at offset 16 = line 3,
    // column 4 (1-based, bytes).
    let source = "---\ntitle: x\n---";
    let out = parse(zfb_md_wasm::parse_to_ast(source, MDX_OPTIONS));
    assert_eq!(out["diagnostics"].as_array().unwrap().len(), 0);
    assert_eq!(out["frontmatter"]["title"], "x");
    let root_pos = &out["ast"]["position"];
    for point in ["start", "end"] {
        assert_eq!(root_pos[point]["line"].as_u64(), Some(3), "{point}");
        assert_eq!(root_pos[point]["column"].as_u64(), Some(4), "{point}");
        assert_eq!(root_pos[point]["offset"].as_u64(), Some(16), "{point}");
    }
}

#[test]
fn utf16_code_unit_semantics_are_pinned_on_non_ascii() {
    // Replaces the Wave-1 byte-offset pin (decision-sub zfb#1856 LOCKED this
    // as the production contract): `position.offset`/`position.column` are
    // UTF-16 code units, matching remark/unist and JS `String` indexing.
    // `日本語` alone (3 chars, all in the Basic Multilingual Plane, 1 UTF-16
    // unit each) would NOT discriminate UTF-16 units from Unicode scalar
    // values/code points — `😀` (U+1F600) is required: it needs a SURROGATE
    // PAIR (2 UTF-16 units, 1 scalar value, 4 UTF-8 bytes), so only a UTF-16
    // implementation reports the numbers pinned below.
    let source = "# 日本語 😀\n\nnext\n";
    let out = parse(zfb_md_wasm::parse_to_ast(source, MDX_OPTIONS));
    assert_eq!(out["diagnostics"].as_array().unwrap().len(), 0);
    let heading_end = &out["ast"]["children"][0]["position"]["end"];
    // Independent oracle: std's `encode_utf16` on the heading's own source
    // line, not the production `char::len_utf16`-based prefix map.
    let heading_line = source.split('\n').next().unwrap();
    let expected_utf16_units = heading_line.encode_utf16().count() as u64;
    // "# 日本語 😀" = 2 + 3 + 1 + 2 = 8 UTF-16 units (2 for "# ", 3 for
    // "日本語", 1 for the space, 2 for the surrogate-pair emoji). If this
    // ever starts asserting 16 (bytes) or 7 (code points), the contract
    // regressed — update lib.rs/types.ts's docs before touching this pin.
    assert_eq!(expected_utf16_units, 8);
    assert_eq!(heading_end["offset"].as_u64(), Some(expected_utf16_units));
    assert_eq!(
        heading_end["column"].as_u64(),
        Some(expected_utf16_units + 1)
    );
}

#[test]
fn utf16_positions_are_shifted_on_non_ascii_fixture() {
    // The recursive whole-tree proof above uses an ASCII fixture, where
    // UTF-16 units == bytes (the fast path never even builds the prefix
    // map) — it cannot, by itself, prove the UTF-16 conversion runs
    // correctly. This test re-proves the same whole-tree contract on the
    // shared CJK+emoji fixture, computing the expected UTF-16 line/column/
    // offset independently (via `str::encode_utf16`, not the production
    // `char::len_utf16`-based prefix map) rather than re-deriving the
    // production arithmetic.
    let fixture = load_fixture("non-ascii-cjk-emoji");
    let options_json = serde_json::to_string(&fixture.options).unwrap();
    let out = parse(zfb_md_wasm::parse_to_ast(&fixture.source, &options_json));
    assert_eq!(
        out["diagnostics"].as_array().unwrap().len(),
        0,
        "fixture parses clean: {:?}",
        out["diagnostics"]
    );
    assert_eq!(
        out["frontmatter"],
        Value::Null,
        "fixture has no frontmatter"
    );

    let raw = zfb_content::facade::parse_mdast(
        &zfb_content::facade::PipelineOptions::default(),
        &fixture.source,
    )
    .expect("fixture body parses");
    let raw_json = serde_json::to_value(&raw).expect("mdast serializes");

    for required in ["heading", "text", "emphasis", "list", "listItem"] {
        assert!(
            count_nodes_of_type(&out["ast"], required) > 0,
            "non-ASCII fixture must contain a `{required}` node"
        );
    }

    assert_every_node_has_position(&out["ast"], "ast");
    assert_utf16_shifted_tree(&out["ast"], &raw_json, &fixture.source, "ast");
}

/// Independent UTF-16 point oracle (test-only, deliberately NOT the
/// production `char::len_utf16`-based prefix map): same algorithm class as
/// markdown-rs's own `Location::to_point`, but counting UTF-16 code units
/// via `str::encode_utf16` instead of bytes. `byte_offset` is a byte offset
/// into `source` (no frontmatter shift needed here — the fixture this
/// backs has none, so raw markdown-rs offsets already index `source`
/// directly).
fn independent_utf16_point(source: &str, byte_offset: usize) -> (u64, u64, u64) {
    let prefix = &source[..byte_offset];
    let line = prefix.matches('\n').count() as u64 + 1;
    let line_start_byte = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let column = source[line_start_byte..byte_offset].encode_utf16().count() as u64 + 1;
    let offset = source[..byte_offset].encode_utf16().count() as u64;
    (line, column, offset)
}

/// Recursive whole-tree UTF-16 proof, mirroring `assert_shifted_tree`'s
/// byte-based sibling above but for the frontmatter-free non-ASCII fixture
/// (no body-offset/prefix-lines shift needed — `raw` already parses the
/// exact same `source`), and asserting UTF-16 units via the independent
/// oracle above instead of raw-plus-delta arithmetic.
fn assert_utf16_shifted_tree(shifted: &Value, raw: &Value, source: &str, path: &str) {
    match (shifted, raw) {
        (Value::Object(s), Value::Object(r)) => {
            let s_keys: Vec<_> = s.keys().collect();
            let r_keys: Vec<_> = r.keys().collect();
            assert_eq!(s_keys, r_keys, "key sets diverge at {path}");
            for (key, s_val) in s {
                let r_val = &r[key];
                match key.as_str() {
                    "position" => {
                        for point in ["start", "end"] {
                            let raw_byte_offset = r_val[point]["offset"].as_u64().unwrap() as usize;
                            let (exp_line, exp_column, exp_offset) =
                                independent_utf16_point(source, raw_byte_offset);
                            assert_eq!(
                                s_val[point]["line"].as_u64(),
                                Some(exp_line),
                                "{path}.position.{point}.line"
                            );
                            assert_eq!(
                                s_val[point]["column"].as_u64(),
                                Some(exp_column),
                                "{path}.position.{point}.column (UTF-16 units)"
                            );
                            assert_eq!(
                                s_val[point]["offset"].as_u64(),
                                Some(exp_offset),
                                "{path}.position.{point}.offset (UTF-16 units)"
                            );
                        }
                    }
                    "_markdownRsStops" => {
                        // No frontmatter on this fixture, so stops are
                        // unshifted AND (by contract) never UTF-16-converted.
                        assert_eq!(
                            s_val, r_val,
                            "_markdownRsStops stay byte-based and unshifted at {path}"
                        );
                    }
                    _ => assert_utf16_shifted_tree(s_val, r_val, source, &format!("{path}.{key}")),
                }
            }
        }
        (Value::Array(s), Value::Array(r)) => {
            assert_eq!(s.len(), r.len(), "array lengths diverge at {path}");
            for (i, (s_val, r_val)) in s.iter().zip(r).enumerate() {
                assert_utf16_shifted_tree(s_val, r_val, source, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(shifted, raw, "non-position values diverge at {path}"),
    }
}

#[test]
fn mdx_custom_component_survives_as_typed_node_with_attributes() {
    // Requirement 3 of zfb#1828: unrecognized/custom constructs (MDX JSX
    // elements) survive serialization as TYPED nodes, not dropped or
    // collapsed to plain text -- proven here on the shared fixture corpus's
    // `<CustomComponent>` element (name + a literal attribute + an
    // expression attribute + nested phrasing content).
    let fixture = load_fixture("mdx-custom-component");
    let options_json = serde_json::to_string(&fixture.options).unwrap();
    let out = parse(zfb_md_wasm::parse_to_ast(&fixture.source, &options_json));
    assert_eq!(
        out["diagnostics"].as_array().unwrap().len(),
        0,
        "fixture parses clean: {:?}",
        out["diagnostics"]
    );
    let ast = &out["ast"];
    let element = ast["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["type"] == "mdxJsxFlowElement")
        .expect("fixture contains an mdxJsxFlowElement");

    assert_eq!(element["name"], "CustomComponent");
    assert!(
        element["position"].is_object(),
        "the element itself carries a position"
    );

    let attributes = element["attributes"].as_array().unwrap();
    let prop = attributes
        .iter()
        .find(|a| a["name"] == "prop")
        .expect("literal `prop` attribute survives");
    assert_eq!(prop["type"], "mdxJsxAttribute");
    assert_eq!(prop["value"], "x");

    let count = attributes
        .iter()
        .find(|a| a["name"] == "count")
        .expect("expression `count` attribute survives");
    assert_eq!(count["type"], "mdxJsxAttribute");
    assert_eq!(count["value"]["type"], "mdxJsxAttributeValueExpression");
    assert_eq!(count["value"]["value"], "2 + 3");

    // Nested content: the element's paragraph child carries a text node and
    // an emphasis node, exactly as markdown-rs parsed them -- no zfb
    // visitor rewrote or dropped anything.
    let paragraph = element["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "paragraph")
        .expect("nested content forms a paragraph child");
    let inline = paragraph["children"].as_array().unwrap();
    assert!(
        inline.iter().any(|c| c["type"] == "text"),
        "nested plain text survives: {inline:?}"
    );
    assert!(
        inline.iter().any(|c| c["type"] == "emphasis"),
        "nested emphasis survives: {inline:?}"
    );
}
