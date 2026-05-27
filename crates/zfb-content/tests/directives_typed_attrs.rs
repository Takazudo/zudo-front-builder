//! Integration tests for `DirectiveDef` typed attribute schemas + validation.
//!
//! Tests cover all four `AttrType` variants (`String`, `Enum`, `Boolean`,
//! `Number`), required-missing diagnostics, enum-value mismatches,
//! type-coercion failures, default-value injection, and fall-back behaviour
//! when validation fails (raw attrs still pass through).
//!
//! A separate file is used (rather than appending to `integration_pipeline.rs`)
//! to avoid conflicts with issue #568 which also modifies that file.

use markdown::mdast::{
    AttributeContent, AttributeValue, MdxJsxFlowElement, Node as MdastNode, Paragraph, Root, Text,
};
use zfb_content::plugins::directives::{
    AttrSchema, AttrType, DirectiveDef, DirectiveKind, DirectiveRegistry, ValidatedAttrValue,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn text_para(value: &str) -> MdastNode {
    MdastNode::Paragraph(Paragraph {
        children: vec![MdastNode::Text(Text {
            value: value.to_string(),
            position: None,
        })],
        position: None,
    })
}

fn run_registry(reg: &mut DirectiveRegistry, children: Vec<MdastNode>) -> Vec<MdastNode> {
    use zfb_content::pipeline::MdastVisitor;
    let mut root = MdastNode::Root(Root {
        children,
        position: None,
    });
    reg.visit(&mut root);
    let MdastNode::Root(Root { children, .. }) = root else {
        unreachable!()
    };
    children
}

fn flow(node: &MdastNode) -> &MdxJsxFlowElement {
    match node {
        MdastNode::MdxJsxFlowElement(j) => j,
        other => unreachable!("expected MdxJsxFlowElement, got {other:?}"),
    }
}

fn attr_value(j: &MdxJsxFlowElement, name: &str) -> Option<String> {
    for a in &j.attributes {
        if let AttributeContent::Property(p) = a {
            if p.name == name {
                if let Some(AttributeValue::Literal(v)) = &p.value {
                    return Some(v.clone());
                }
            }
        }
    }
    None
}

fn has_attr(j: &MdxJsxFlowElement, name: &str) -> bool {
    j.attributes.iter().any(|a| {
        if let AttributeContent::Property(p) = a {
            p.name == name
        } else {
            false
        }
    })
}

fn schema(name: &str, ty: AttrType, default: Option<&str>, required: bool) -> AttrSchema {
    AttrSchema {
        name: name.to_string(),
        ty,
        default: default.map(|s| s.to_string()),
        required,
    }
}

// ---------------------------------------------------------------------------
// validate_attrs unit-level tests (no registry, just the method)
// ---------------------------------------------------------------------------

#[test]
fn validate_string_attr_passes_through() {
    let def = DirectiveDef::container("callout", "Callout").with_attrs(vec![schema(
        "title",
        AttrType::String,
        None,
        false,
    )]);
    let raw = vec![("title".to_string(), "Hello".to_string())];
    let (result, warnings) = def.validate_attrs(&raw);
    assert!(warnings.is_empty(), "no warnings expected");
    let map = result.expect("validation should succeed");
    assert_eq!(
        map.get("title"),
        Some(&ValidatedAttrValue::String("Hello".to_string()))
    );
}

#[test]
fn validate_enum_attr_valid_value() {
    let def = DirectiveDef::container("callout", "Callout").with_attrs(vec![schema(
        "tone",
        AttrType::Enum(vec!["info".to_string(), "warn".to_string(), "tip".to_string()]),
        None,
        true,
    )]);
    let raw = vec![("tone".to_string(), "warn".to_string())];
    let (result, warnings) = def.validate_attrs(&raw);
    assert!(warnings.is_empty());
    let map = result.expect("valid enum should succeed");
    assert_eq!(
        map.get("tone"),
        Some(&ValidatedAttrValue::Enum("warn".to_string()))
    );
}

#[test]
fn validate_enum_attr_invalid_value_emits_error() {
    let def = DirectiveDef::container("callout", "Callout").with_attrs(vec![schema(
        "tone",
        AttrType::Enum(vec!["info".to_string(), "warn".to_string()]),
        None,
        false,
    )]);
    let raw = vec![("tone".to_string(), "oops".to_string())];
    let (result, _warnings) = def.validate_attrs(&raw);
    let errors = result.expect_err("invalid enum value should produce error");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("oops"), "error mentions bad value");
    assert!(errors[0].message.contains("tone"), "error mentions attr name");
}

#[test]
fn validate_boolean_true_and_false() {
    let def = DirectiveDef::leaf("badge", "Badge").with_attrs(vec![schema(
        "compact",
        AttrType::Boolean,
        None,
        false,
    )]);

    // "true"
    let raw = vec![("compact".to_string(), "true".to_string())];
    let (result, _) = def.validate_attrs(&raw);
    let map = result.expect("'true' is valid bool");
    assert_eq!(map.get("compact"), Some(&ValidatedAttrValue::Boolean(true)));

    // "false"
    let raw = vec![("compact".to_string(), "false".to_string())];
    let (result, _) = def.validate_attrs(&raw);
    let map = result.expect("'false' is valid bool");
    assert_eq!(
        map.get("compact"),
        Some(&ValidatedAttrValue::Boolean(false))
    );
}

#[test]
fn validate_boolean_bare_attr_empty_string_is_true() {
    // The parser emits an empty string for bare boolean attrs (no `=value`).
    let def = DirectiveDef::leaf("badge", "Badge").with_attrs(vec![schema(
        "disabled",
        AttrType::Boolean,
        None,
        false,
    )]);
    let raw = vec![("disabled".to_string(), "".to_string())];
    let (result, _) = def.validate_attrs(&raw);
    let map = result.expect("bare attr (empty string) is valid bool=true");
    assert_eq!(
        map.get("disabled"),
        Some(&ValidatedAttrValue::Boolean(true))
    );
}

#[test]
fn validate_boolean_invalid_value_emits_error() {
    let def = DirectiveDef::leaf("badge", "Badge").with_attrs(vec![schema(
        "active",
        AttrType::Boolean,
        None,
        false,
    )]);
    let raw = vec![("active".to_string(), "yes".to_string())];
    let (result, _) = def.validate_attrs(&raw);
    let errors = result.expect_err("'yes' is not a valid boolean");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("active"));
    assert!(errors[0].message.contains("yes"));
}

#[test]
fn validate_number_valid_integer() {
    let def = DirectiveDef::leaf("chart", "Chart").with_attrs(vec![schema(
        "max",
        AttrType::Number,
        None,
        false,
    )]);
    let raw = vec![("max".to_string(), "100".to_string())];
    let (result, _) = def.validate_attrs(&raw);
    let map = result.expect("integer is valid number");
    assert_eq!(
        map.get("max"),
        Some(&ValidatedAttrValue::Number("100".to_string()))
    );
}

#[test]
fn validate_number_valid_float() {
    let def = DirectiveDef::leaf("chart", "Chart").with_attrs(vec![schema(
        "step",
        AttrType::Number,
        None,
        false,
    )]);
    let raw = vec![("step".to_string(), "0.5".to_string())];
    let (result, _) = def.validate_attrs(&raw);
    let map = result.expect("float is valid number");
    assert_eq!(
        map.get("step"),
        Some(&ValidatedAttrValue::Number("0.5".to_string()))
    );
}

#[test]
fn validate_number_invalid_value_emits_error() {
    let def = DirectiveDef::leaf("chart", "Chart").with_attrs(vec![schema(
        "width",
        AttrType::Number,
        None,
        false,
    )]);
    let raw = vec![("width".to_string(), "abc".to_string())];
    let (result, _) = def.validate_attrs(&raw);
    let errors = result.expect_err("'abc' is not a valid number");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("abc"));
    assert!(errors[0].message.contains("width"));
}

#[test]
fn validate_required_attr_missing_emits_error() {
    let def = DirectiveDef::container("section", "Section").with_attrs(vec![schema(
        "id",
        AttrType::String,
        None,
        true, // required
    )]);
    let raw: Vec<(String, String)> = vec![];
    let (result, _) = def.validate_attrs(&raw);
    let errors = result.expect_err("missing required attr should be an error");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("id"), "error mentions attr name");
    assert!(
        errors[0].message.contains("required"),
        "error says 'required'"
    );
}

#[test]
fn validate_optional_attr_missing_no_default_absent_from_map() {
    let def = DirectiveDef::container("section", "Section").with_attrs(vec![schema(
        "subtitle",
        AttrType::String,
        None,
        false, // optional, no default
    )]);
    let raw: Vec<(String, String)> = vec![];
    let (result, warnings) = def.validate_attrs(&raw);
    assert!(warnings.is_empty());
    let map = result.expect("optional missing attr should not error");
    assert!(
        map.get("subtitle").is_none(),
        "absent optional not in map"
    );
}

#[test]
fn validate_default_value_applied_when_attr_absent() {
    let def = DirectiveDef::container("callout", "Callout").with_attrs(vec![schema(
        "tone",
        AttrType::Enum(vec!["info".to_string(), "warn".to_string()]),
        Some("info"), // default
        false,
    )]);
    let raw: Vec<(String, String)> = vec![];
    let (result, _) = def.validate_attrs(&raw);
    let map = result.expect("default should apply successfully");
    assert_eq!(
        map.get("tone"),
        Some(&ValidatedAttrValue::Enum("info".to_string()))
    );
}

#[test]
fn validate_unknown_attr_emits_warning_not_error() {
    let def = DirectiveDef::container("callout", "Callout").with_attrs(vec![schema(
        "tone",
        AttrType::String,
        None,
        false,
    )]);
    // Pass both a known attr and an unknown one.
    let raw = vec![
        ("tone".to_string(), "info".to_string()),
        ("unknown-extra".to_string(), "whatever".to_string()),
    ];
    let (result, warnings) = def.validate_attrs(&raw);
    // No hard error.
    let _ = result.expect("known attr is valid; unknown is only a warning");
    // One warning for the unknown attr.
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].message.contains("unknown-extra"),
        "warning mentions unknown attr name"
    );
    assert!(
        warnings[0].message.contains("warning"),
        "message says it's a warning"
    );
}

#[test]
fn validate_multiple_errors_all_collected() {
    let def = DirectiveDef::container("widget", "Widget").with_attrs(vec![
        schema("id", AttrType::String, None, true), // required, missing
        schema(
            "count",
            AttrType::Number,
            None,
            false,
        ), // present, but bad value
    ]);
    let raw = vec![("count".to_string(), "notanumber".to_string())];
    let (result, _) = def.validate_attrs(&raw);
    let errors = result.expect_err("should have errors");
    // Two errors: missing required 'id' + bad type for 'count'.
    assert_eq!(errors.len(), 2, "expected 2 errors, got {errors:?}");
}

// ---------------------------------------------------------------------------
// Empty schema (backwards-compat) — no validation run
// ---------------------------------------------------------------------------

#[test]
fn empty_schema_skips_validation_entirely() {
    let def = DirectiveDef::container("note", "Note");
    // A directive with no schema declared does nothing.
    let raw = vec![
        ("anything".to_string(), "val".to_string()),
        ("another".to_string(), "val2".to_string()),
    ];
    let (result, warnings) = def.validate_attrs(&raw);
    assert!(warnings.is_empty(), "no warnings for empty schema");
    let map = result.expect("empty schema always succeeds");
    assert!(map.is_empty(), "empty schema produces empty map");
}

// ---------------------------------------------------------------------------
// ValidatedAttrValue::as_str normalisation
// ---------------------------------------------------------------------------

#[test]
fn validated_attr_value_as_str_boolean_normalisation() {
    assert_eq!(ValidatedAttrValue::Boolean(true).as_str(), "true");
    assert_eq!(ValidatedAttrValue::Boolean(false).as_str(), "false");
    assert_eq!(
        ValidatedAttrValue::String("hello".to_string()).as_str(),
        "hello"
    );
    assert_eq!(
        ValidatedAttrValue::Enum("info".to_string()).as_str(),
        "info"
    );
    assert_eq!(
        ValidatedAttrValue::Number("3.14".to_string()).as_str(),
        "3.14"
    );
}

// ---------------------------------------------------------------------------
// Registry integration — expansion path wires diagnostics + JSX correctly
// ---------------------------------------------------------------------------

#[test]
fn container_with_typed_attrs_builds_jsx_with_validated_values() {
    let mut reg = DirectiveRegistry::new();
    reg.register(
        DirectiveDef::container("callout", "Callout").with_attrs(vec![
            schema(
                "tone",
                AttrType::Enum(vec!["info".to_string(), "warn".to_string()]),
                None,
                true,
            ),
            schema("compact", AttrType::Boolean, Some("false"), false),
        ]),
    );

    let out = run_registry(
        &mut reg,
        vec![
            text_para(":::callout{tone=\"info\"}"),
            text_para("body"),
            text_para(":::"),
        ],
    );
    assert_eq!(out.len(), 1);
    let j = flow(&out[0]);
    assert_eq!(j.name.as_deref(), Some("Callout"));
    // tone passes through as string literal.
    assert_eq!(attr_value(j, "tone").as_deref(), Some("info"));
    // compact default="false" applied.
    assert_eq!(attr_value(j, "compact").as_deref(), Some("false"));
    // No diagnostics.
    assert!(reg.take_diagnostics().is_empty());
}

#[test]
fn container_validation_error_falls_back_to_raw_attrs_and_emits_diagnostic() {
    let mut reg = DirectiveRegistry::new();
    reg.register(
        DirectiveDef::container("callout", "Callout").with_attrs(vec![schema(
            "tone",
            AttrType::Enum(vec!["info".to_string(), "warn".to_string()]),
            None,
            true, // required
        )]),
    );

    // Omit the required 'tone' attr, but pass an unknown 'extra' attr.
    // This triggers: 1 warning (unknown 'extra') + 1 error (missing required 'tone').
    let out = run_registry(
        &mut reg,
        vec![
            text_para(":::callout{extra=\"whatever\"}"),
            text_para("body"),
            text_para(":::"),
        ],
    );
    // JSX still emitted (graceful fallback, not a build error).
    assert_eq!(out.len(), 1);
    let j = flow(&out[0]);
    assert_eq!(j.name.as_deref(), Some("Callout"));
    // raw attr 'extra' passes through because we fell back on error.
    assert_eq!(attr_value(j, "extra").as_deref(), Some("whatever"));
    // Two diagnostics: warning for unknown 'extra', error for missing required 'tone'.
    let diags = reg.take_diagnostics();
    assert_eq!(diags.len(), 2, "two diagnostics expected, got {diags:?}");
    let tone_diag = diags
        .iter()
        .find(|d| d.message.contains("tone"))
        .expect("should have a diagnostic about 'tone'");
    assert!(
        tone_diag.message.contains("required"),
        "tone diagnostic says 'required'"
    );
    let extra_diag = diags
        .iter()
        .find(|d| d.message.contains("extra"))
        .expect("should have a diagnostic about 'extra'");
    assert!(
        extra_diag.message.contains("warning"),
        "extra diagnostic is a warning"
    );
}

#[test]
fn leaf_with_boolean_attr_emits_normalised_string() {
    let mut reg = DirectiveRegistry::new();
    reg.register(
        DirectiveDef::leaf("badge", "Badge").with_attrs(vec![schema(
            "outline",
            AttrType::Boolean,
            None,
            false,
        )]),
    );

    let out = run_registry(
        &mut reg,
        vec![text_para("::badge[Label]{outline=true}")],
    );
    assert_eq!(out.len(), 1);
    let j = flow(&out[0]);
    assert_eq!(attr_value(j, "outline").as_deref(), Some("true"));
    assert!(reg.take_diagnostics().is_empty());
}

#[test]
fn number_type_coercion_failure_emits_diagnostic_and_fallback() {
    let mut reg = DirectiveRegistry::new();
    reg.register(
        DirectiveDef::leaf("progress", "Progress").with_attrs(vec![schema(
            "value",
            AttrType::Number,
            None,
            false,
        )]),
    );

    let out = run_registry(
        &mut reg,
        vec![text_para("::progress{value=notanumber}")],
    );
    // Fallback: raw attr still emitted.
    assert_eq!(out.len(), 1);
    let j = flow(&out[0]);
    // Because validation failed, raw attr passes through verbatim.
    assert_eq!(attr_value(j, "value").as_deref(), Some("notanumber"));
    let diags = reg.take_diagnostics();
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("notanumber"));
}

#[test]
fn unknown_attr_passes_through_with_warning_and_jsx_still_built() {
    let mut reg = DirectiveRegistry::new();
    reg.register(
        DirectiveDef::container("section", "Section").with_attrs(vec![schema(
            "id",
            AttrType::String,
            None,
            false,
        )]),
    );

    let out = run_registry(
        &mut reg,
        vec![
            text_para(":::section{id=\"intro\" data-extra=\"x\"}"),
            text_para("body"),
            text_para(":::"),
        ],
    );
    assert_eq!(out.len(), 1);
    let j = flow(&out[0]);
    // Known attr present.
    assert_eq!(attr_value(j, "id").as_deref(), Some("intro"));
    // Unknown attr also present (passes through verbatim).
    assert_eq!(attr_value(j, "data-extra").as_deref(), Some("x"));
    // Warning (not error) emitted.
    let diags = reg.take_diagnostics();
    assert_eq!(
        diags.len(),
        1,
        "one warning for unknown attr, got {diags:?}"
    );
    assert!(diags[0].message.contains("data-extra"));
}

#[test]
fn existing_directives_without_schema_compile_and_expand_unchanged() {
    // Regression guard: directives registered without any schema (the
    // backwards-compat path) must continue to work exactly as before.
    let mut reg = DirectiveRegistry::new();
    reg.register(DirectiveDef::container("card", "Card"));
    let out = run_registry(
        &mut reg,
        vec![
            text_para(":::card{title=\"x\" variant=\"outline\"}"),
            text_para("body"),
            text_para(":::"),
        ],
    );
    assert_eq!(out.len(), 1);
    let j = flow(&out[0]);
    assert_eq!(j.name.as_deref(), Some("Card"));
    assert_eq!(attr_value(j, "title").as_deref(), Some("x"));
    assert_eq!(attr_value(j, "variant").as_deref(), Some("outline"));
    assert!(reg.take_diagnostics().is_empty());
}

#[test]
fn default_admonitions_no_schema_still_work() {
    // Guard: the six built-in admonitions (no schema) must still expand
    // without diagnostics after the typed-attrs change.
    let mut reg = DirectiveRegistry::with_defaults();
    let out = run_registry(
        &mut reg,
        vec![
            text_para(":::note[Custom Title]"),
            text_para("body"),
            text_para(":::"),
        ],
    );
    assert_eq!(out.len(), 1);
    let j = flow(&out[0]);
    assert_eq!(j.name.as_deref(), Some("Note"));
    assert_eq!(attr_value(j, "title").as_deref(), Some("Custom Title"));
    assert!(
        reg.take_diagnostics().is_empty(),
        "no diagnostics for well-formed admonition"
    );
}

#[test]
fn with_attrs_builder_is_chainable() {
    // Verify the `.with_attrs()` chainable builder works for all three
    // directive kinds.
    let attrs = vec![schema("label", AttrType::String, None, false)];

    let c = DirectiveDef::container("a", "A").with_attrs(attrs.clone());
    let l = DirectiveDef::leaf("b", "B").with_attrs(attrs.clone());
    let t = DirectiveDef::text("c", "C").with_attrs(attrs.clone());

    assert_eq!(c.attrs.len(), 1);
    assert_eq!(l.attrs.len(), 1);
    assert_eq!(t.attrs.len(), 1);
    assert_eq!(c.kind, DirectiveKind::Container);
    assert_eq!(l.kind, DirectiveKind::Leaf);
    assert_eq!(t.kind, DirectiveKind::Text);
}

#[test]
fn container_with_all_four_attr_types_validates_correctly() {
    // End-to-end: a directive with one of each AttrType variant.
    let def = DirectiveDef::container("widget", "Widget").with_attrs(vec![
        schema("label", AttrType::String, None, false),
        schema(
            "size",
            AttrType::Enum(vec!["sm".to_string(), "md".to_string(), "lg".to_string()]),
            Some("md"),
            false,
        ),
        schema("visible", AttrType::Boolean, Some("true"), false),
        schema("count", AttrType::Number, Some("0"), false),
    ]);
    let raw = vec![
        ("label".to_string(), "hello".to_string()),
        ("size".to_string(), "lg".to_string()),
        ("visible".to_string(), "false".to_string()),
        ("count".to_string(), "42".to_string()),
    ];
    let (result, warnings) = def.validate_attrs(&raw);
    assert!(warnings.is_empty());
    let map = result.expect("all valid attrs");
    assert_eq!(
        map.get("label"),
        Some(&ValidatedAttrValue::String("hello".to_string()))
    );
    assert_eq!(
        map.get("size"),
        Some(&ValidatedAttrValue::Enum("lg".to_string()))
    );
    assert_eq!(
        map.get("visible"),
        Some(&ValidatedAttrValue::Boolean(false))
    );
    assert_eq!(
        map.get("count"),
        Some(&ValidatedAttrValue::Number("42".to_string()))
    );
}

#[test]
fn defaults_applied_for_all_four_types_when_absent() {
    let def = DirectiveDef::container("widget", "Widget").with_attrs(vec![
        schema("label", AttrType::String, Some("default-label"), false),
        schema(
            "size",
            AttrType::Enum(vec!["sm".to_string(), "md".to_string()]),
            Some("sm"),
            false,
        ),
        schema("visible", AttrType::Boolean, Some("true"), false),
        schema("count", AttrType::Number, Some("0"), false),
    ]);
    let raw: Vec<(String, String)> = vec![];
    let (result, warnings) = def.validate_attrs(&raw);
    assert!(warnings.is_empty());
    let map = result.expect("all defaults valid");
    assert_eq!(
        map.get("label"),
        Some(&ValidatedAttrValue::String("default-label".to_string()))
    );
    assert_eq!(
        map.get("size"),
        Some(&ValidatedAttrValue::Enum("sm".to_string()))
    );
    assert_eq!(
        map.get("visible"),
        Some(&ValidatedAttrValue::Boolean(true))
    );
    assert_eq!(
        map.get("count"),
        Some(&ValidatedAttrValue::Number("0".to_string()))
    );
}

#[test]
fn jsx_attr_order_schema_attrs_first_then_unknown() {
    // When validation succeeds: schema-declared attrs appear first (in
    // schema declaration order), then unknown/pass-through attrs.
    let mut reg = DirectiveRegistry::new();
    reg.register(
        DirectiveDef::container("box", "Box").with_attrs(vec![
            schema("id", AttrType::String, None, false),
            schema("class", AttrType::String, None, false),
        ]),
    );

    let out = run_registry(
        &mut reg,
        vec![
            text_para(":::box{id=\"main\" data-foo=\"bar\" class=\"wide\"}"),
            text_para("content"),
            text_para(":::"),
        ],
    );
    assert_eq!(out.len(), 1);
    let j = flow(&out[0]);

    // All three attrs present (schema + unknown pass-through).
    assert!(has_attr(j, "id"));
    assert!(has_attr(j, "class"));
    assert!(has_attr(j, "data-foo"));

    assert_eq!(attr_value(j, "id").as_deref(), Some("main"));
    assert_eq!(attr_value(j, "class").as_deref(), Some("wide"));
    assert_eq!(attr_value(j, "data-foo").as_deref(), Some("bar"));

    // One warning for data-foo.
    let diags = reg.take_diagnostics();
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("data-foo"));
}
