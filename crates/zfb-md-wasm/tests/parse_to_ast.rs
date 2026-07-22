//! Native (rlib) integration tests for the PROTOTYPE `parseToAst` export
//! (zfb#1855, epic zfb#1854 — go/no-go spike; pruned on a Wave-2 no-go).
//!
//! Test plan (declared per project testing discipline):
//! - **Level**: 1 (unit/logic). `parse_to_ast` is a pure `&str → String`
//!   transform; every assertion is a JSON comparison against in-memory
//!   output on the host triple — same rationale as `tests/api.rs`.
//! - **Why level 1**: the position-shift arithmetic and the raw-mdast
//!   contract shape are pure logic. The wasm boundary itself is exercised
//!   at level 3 by the npm package's `test/parse-to-ast.test.ts` against
//!   the built `dist/` artifact.
//! - **What's covered**: (a) the review-mandated RECURSIVE WHOLE-TREE
//!   position proof — every node in the returned AST carries `position`,
//!   with lines shifted by the frontmatter line count AND offsets shifted
//!   by the body byte offset (columns unchanged), verified node-by-node
//!   against an independently parsed unshifted body tree AND by
//!   slice-equality against the original source bytes; (b) contract-shape
//!   fixtures: MDX (JSX elements + expressions + ESM), directive-style
//!   `:::note` text (stays RAW — no zfb visitor runs), custom-node-ish
//!   GitHub-alert blockquote (stays a plain blockquote — pre-visitor
//!   contract); (c) `_markdownRsStops` absolute offsets shifted in
//!   lock-step with positions; (d) no-frontmatter passthrough (zero
//!   shift); (e) parse errors surface as `"markdown"` diagnostics with
//!   frontmatter-shifted lines, same arithmetic as `renderHtml`.
//! - **Blind spots**: executing the compiled wasm artifact (npm package
//!   scope); benchmark characteristics (the bench harness under
//!   `npm/test/bench/` measures, it does not assert).

#![cfg(feature = "pipeline")]

use serde_json::Value;

fn parse(result: String) -> Value {
    serde_json::from_str(&result).expect("API output is always a JSON document")
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
fn byte_offset_semantics_are_pinned_on_non_ascii() {
    // Self-review finding (P1), PINNED as a KNOWN CONTRACT GAP — decision-sub
    // input (zfb#1856), not an endorsement: positions are markdown-rs's
    // native UTF-8 BYTE units, so on non-ASCII sources the serialized
    // offsets diverge from the UTF-16 code-unit indices remark/unist
    // consumers expect. `日本語` is 9 bytes / 3 UTF-16 code units; a full
    // implementation must convert (re-benchmarking the added cost) or
    // explicitly declare byte semantics. This test exists so the gap is
    // explicit, not silently shipped.
    let source = "# 日本語\n\nnext\n";
    let out = parse(zfb_md_wasm::parse_to_ast(source, MDX_OPTIONS));
    assert_eq!(out["diagnostics"].as_array().unwrap().len(), 0);
    let heading_end = &out["ast"]["children"][0]["position"]["end"];
    // "# 日本語" = 2 + 9 BYTES (5 code points, 5 UTF-16 units). If this
    // assertion ever starts failing with 7, the contract switched to
    // UTF-16/code-point units — update the docs in lib.rs/types.ts with it.
    assert_eq!(heading_end["offset"].as_u64(), Some(11));
    assert_eq!(heading_end["column"].as_u64(), Some(12));
}
