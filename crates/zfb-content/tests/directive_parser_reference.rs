use serde::Deserialize;
use serde_json::Value;
use zfb_content::directive_parser::{parse_directive_mdast, DirectiveMdastNode};
use zfb_content::facade::{GfmOptions, ParseDialect, ParseMdastOptions};

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    dialect: ParseDialect,
    source: String,
}

#[derive(Debug, Deserialize)]
struct Oracle {
    name: String,
    ast: Value,
}

#[test]
fn matches_pinned_remark_directive_oracle() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/directives/cases.json")).unwrap();
    let oracles: Vec<Oracle> =
        serde_json::from_str(include_str!("fixtures/directives/oracle.json")).unwrap();
    assert_eq!(cases.len(), oracles.len());

    for (case, mut oracle) in cases.into_iter().zip(oracles) {
        assert_eq!(case.name, oracle.name);
        let actual = parse_directive_mdast(
            ParseMdastOptions {
                dialect: case.dialect,
                gfm: GfmOptions::default(),
            },
            &case.source,
        )
        .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        assert_spans(&actual, &case.source, None);
        normalize_documented_base_differences(&mut oracle.ast);
        let mut actual = serde_json::to_value(actual).unwrap();
        normalize_base_container_positions(&mut actual);
        normalize_base_container_positions(&mut oracle.ast);
        assert_eq!(actual, oracle.ast, "oracle mismatch for {}", case.name);
    }
}

fn normalize_base_container_positions(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                normalize_base_container_positions(value);
            }
        }
        Value::Object(object) => {
            if object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| matches!(kind, "root" | "blockquote" | "list" | "listItem"))
            {
                object.remove("position");
            }
            for value in object.values_mut() {
                normalize_base_container_positions(value);
            }
        }
        _ => {}
    }
}

fn normalize_documented_base_differences(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                normalize_documented_base_differences(value);
            }
        }
        Value::Object(object) => {
            object.retain(|_, value| !value.is_null());
            if object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("mdxJsxAttribute"))
            {
                object.remove("position");
            }
            for value in object.values_mut() {
                normalize_documented_base_differences(value);
            }
        }
        _ => {}
    }
}

fn assert_spans(node: &DirectiveMdastNode, source: &str, parent: Option<(usize, usize)>) {
    let start = node.position.start.offset;
    let end = node.position.end.offset;
    assert!(
        source.get(start..end).is_some(),
        "{}: {start}..{end}",
        node.kind
    );
    if let Some((parent_start, parent_end)) = parent {
        assert!(
            start >= parent_start && end <= parent_end,
            "{} {start}..{end} escapes {parent_start}..{parent_end}",
            node.kind
        );
    }
    if let Some(children) = &node.children {
        for child in children {
            assert_spans(child, source, Some((start, end)));
        }
    }
}
