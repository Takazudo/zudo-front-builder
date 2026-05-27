//! Minimal JSON-Schema subset used by zfb to (a) generate precise
//! TypeScript types for `.zfb/types.d.ts` and (b) validate frontmatter
//! at `zfb check` time.
//!
//! The dialect is intentionally tiny — the goal is "good enough for
//! Astro-style frontmatter schemas" rather than full JSON Schema 2020-12
//! conformance. Recognised keywords:
//!
//! - `type`: one of `"string"`, `"number"`, `"integer"`, `"boolean"`,
//!   `"array"`, `"object"`, `"null"`. Also accepts an array of types
//!   for unions (e.g. `["string", "null"]`).
//! - `items`: schema for array elements.
//! - `properties`: object keyed by field name → field schema.
//! - `required`: array of property names that must be present.
//! - `enum`: array of literal values (strings / numbers / booleans).
//!
//! Unknown / missing combinations fall back to `unknown` (TS) and
//! "anything is valid" (validator). That keeps the implementation
//! permissive enough that user schemas which use keywords this dialect
//! does not understand still produce a sane build rather than a hard
//! error.

use serde_json::Value as JsonValue;

// -----------------------------------------------------------------------------
// TS type generation
// -----------------------------------------------------------------------------

/// Render a JSON Schema as a TypeScript type expression.
///
/// `indent` is the number of spaces to use for the OUTERMOST level of
/// nested object braces; nested objects bump the indent by 2 each level.
/// Pass `0` for top-level use.
pub fn json_schema_to_ts(schema: &JsonValue, indent: usize) -> String {
    schema_to_ts_inner(schema, indent)
}

fn schema_to_ts_inner(schema: &JsonValue, indent: usize) -> String {
    let obj = match schema {
        JsonValue::Object(m) => m,
        // A `true` or `null` schema is "anything goes".
        _ => return "unknown".to_string(),
    };

    // `enum` short-circuits everything else — the union of literals is
    // the most precise type we can emit.
    if let Some(JsonValue::Array(values)) = obj.get("enum") {
        if !values.is_empty() {
            let parts: Vec<String> = values.iter().map(literal_to_ts).collect();
            return parts.join(" | ");
        }
    }

    let ts_type = match obj.get("type") {
        Some(JsonValue::String(s)) => single_type_to_ts(s, obj, indent),
        Some(JsonValue::Array(types)) => {
            let parts: Vec<String> = types
                .iter()
                .filter_map(|t| t.as_str().map(|s| single_type_to_ts(s, obj, indent)))
                .collect();
            if parts.is_empty() {
                "unknown".to_string()
            } else {
                parts.join(" | ")
            }
        }
        _ => "unknown".to_string(),
    };
    ts_type
}

fn single_type_to_ts(
    ty: &str,
    obj: &serde_json::Map<String, JsonValue>,
    indent: usize,
) -> String {
    match ty {
        "string" => "string".to_string(),
        "number" | "integer" => "number".to_string(),
        "boolean" => "boolean".to_string(),
        "null" => "null".to_string(),
        "array" => {
            let item = obj
                .get("items")
                .map(|s| schema_to_ts_inner(s, indent))
                .unwrap_or_else(|| "unknown".to_string());
            // Wrap unions / multi-token element types in parentheses so
            // the `[]` binds correctly.
            if needs_paren(&item) {
                format!("({item})[]")
            } else {
                format!("{item}[]")
            }
        }
        "object" => object_to_ts(obj, indent),
        _ => "unknown".to_string(),
    }
}

fn object_to_ts(obj: &serde_json::Map<String, JsonValue>, indent: usize) -> String {
    let props = obj.get("properties").and_then(JsonValue::as_object);
    let required: std::collections::HashSet<&str> = obj
        .get("required")
        .and_then(JsonValue::as_array)
        .map(|arr| arr.iter().filter_map(JsonValue::as_str).collect())
        .unwrap_or_default();

    let Some(props) = props else {
        return "Record<string, unknown>".to_string();
    };
    if props.is_empty() {
        return "{}".to_string();
    }

    let inner_indent = indent + 2;
    let pad = " ".repeat(inner_indent);
    let close_pad = " ".repeat(indent);

    let mut out = String::from("{\n");
    // Explicit alphabetical sort so the emitted `.d.ts` is byte-stable
    // regardless of whether serde_json's `preserve_order` feature is
    // enabled (transitively or directly). Previously the order relied on
    // `Map` being a `BTreeMap`, but `preserve_order` swaps that for an
    // `IndexMap` (insertion order). Sorting explicitly here makes the
    // contract dep-tree-independent.
    let mut sorted_props: Vec<(&String, &JsonValue)> = props.iter().collect();
    sorted_props.sort_by_key(|(k, _)| k.as_str());
    for (key, sub) in sorted_props {
        let optional = !required.contains(key.as_str());
        let q = if optional { "?" } else { "" };
        let ts = schema_to_ts_inner(sub, inner_indent);
        out.push_str(&format!("{pad}{}{q}: {ts};\n", ts_safe_key(key)));
    }
    out.push_str(&format!("{close_pad}}}"));
    out
}

fn literal_to_ts(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => format!("\"{}\"", escape_ts_string(s)),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Null => "null".to_string(),
        // Compound literals are unusual in `enum`; render as JSON for
        // best-effort.
        other => other.to_string(),
    }
}

fn escape_ts_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn ts_safe_key(key: &str) -> String {
    // Identifier-safe keys can be emitted bare; everything else gets
    // wrapped in quotes. The JS / TS rule for unquoted property names is
    // a (small) superset of /^[A-Za-z_$][A-Za-z0-9_$]*$/ — we use the
    // ASCII-only form which is what JSON Schema property names will
    // realistically use.
    let is_ident = !key.is_empty()
        && key.chars().enumerate().all(|(i, c)| {
            if i == 0 {
                c.is_ascii_alphabetic() || c == '_' || c == '$'
            } else {
                c.is_ascii_alphanumeric() || c == '_' || c == '$'
            }
        });
    if is_ident {
        key.to_string()
    } else {
        format!("\"{}\"", escape_ts_string(key))
    }
}

fn needs_paren(ts: &str) -> bool {
    // Cheap approximation: only union types (containing ` | `) need
    // parens. Object types render `{ ... }` and bind tightly enough.
    ts.contains(" | ")
}

// -----------------------------------------------------------------------------
// Validation
// -----------------------------------------------------------------------------

/// One validation problem. Path is a JSON-pointer-ish dotted path into
/// the value (e.g. `tags[0]`, `author.name`). Empty path means "at the
/// root of the frontmatter value".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaIssue {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for SchemaIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// Validate `value` against `schema`. Returns the list of issues —
/// empty Vec means "valid".
pub fn validate(value: &JsonValue, schema: &JsonValue) -> Vec<SchemaIssue> {
    let mut out = Vec::new();
    validate_inner(value, schema, "", &mut out);
    out
}

fn validate_inner(
    value: &JsonValue,
    schema: &JsonValue,
    path: &str,
    out: &mut Vec<SchemaIssue>,
) {
    let JsonValue::Object(schema_obj) = schema else {
        // Non-object schemas (`true`, missing, etc.) accept anything.
        return;
    };

    // `enum` short-circuits.
    if let Some(JsonValue::Array(values)) = schema_obj.get("enum") {
        if !values.iter().any(|v| values_equal(v, value)) {
            let parts: Vec<String> = values.iter().map(literal_to_ts).collect();
            out.push(SchemaIssue {
                path: path.to_string(),
                message: format!("expected one of {}, got {}", parts.join(" | "), describe(value)),
            });
        }
        return;
    }

    let types: Vec<&str> = match schema_obj.get("type") {
        Some(JsonValue::String(s)) => vec![s.as_str()],
        Some(JsonValue::Array(arr)) => arr.iter().filter_map(JsonValue::as_str).collect(),
        _ => return, // No type constraint → accept.
    };

    if !types.iter().any(|t| value_matches_type(value, t)) {
        let expected = if types.len() == 1 {
            types[0].to_string()
        } else {
            types.join(" | ")
        };
        out.push(SchemaIssue {
            path: path.to_string(),
            message: format!("expected {}, got {}", expected, describe(value)),
        });
        return;
    }

    // Recurse for compound types we matched.
    if value.is_object() && types.contains(&"object") {
        validate_object(value.as_object().unwrap(), schema_obj, path, out);
    }
    if value.is_array() && types.contains(&"array") {
        if let Some(items) = schema_obj.get("items") {
            for (i, item) in value.as_array().unwrap().iter().enumerate() {
                let child_path = format!("{path}[{i}]");
                validate_inner(item, items, &child_path, out);
            }
        }
    }
}

fn validate_object(
    obj: &serde_json::Map<String, JsonValue>,
    schema_obj: &serde_json::Map<String, JsonValue>,
    path: &str,
    out: &mut Vec<SchemaIssue>,
) {
    if let Some(JsonValue::Array(req)) = schema_obj.get("required") {
        for r in req {
            if let Some(name) = r.as_str() {
                if !obj.contains_key(name) {
                    let child_path = join_path(path, name);
                    out.push(SchemaIssue {
                        path: child_path,
                        message: "missing required field".to_string(),
                    });
                }
            }
        }
    }
    if let Some(JsonValue::Object(props)) = schema_obj.get("properties") {
        for (key, val) in obj {
            if let Some(sub_schema) = props.get(key) {
                let child_path = join_path(path, key);
                validate_inner(val, sub_schema, &child_path, out);
            }
        }
    }
}

fn value_matches_type(value: &JsonValue, ty: &str) -> bool {
    match ty {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some()
            || value.as_u64().is_some()
            // Also accept JSON `1.0` which is technically integral, but
            // only when it actually fits in an integer lane — otherwise
            // `1e30.fract() == 0.0` would silently classify oversized
            // floats as integers.
            || value
                .as_f64()
                .map(|f| {
                    f.is_finite()
                        && f.fract() == 0.0
                        && f >= i64::MIN as f64
                        && f <= u64::MAX as f64
                })
                .unwrap_or(false),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    }
}

fn values_equal(a: &JsonValue, b: &JsonValue) -> bool {
    a == b
}

fn describe(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::String(_) => "string",
        JsonValue::Number(_) => "number",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Null => "null",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

fn join_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}.{child}")
    }
}

// -----------------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ts_primitive_types() {
        assert_eq!(json_schema_to_ts(&json!({ "type": "string" }), 0), "string");
        assert_eq!(json_schema_to_ts(&json!({ "type": "number" }), 0), "number");
        assert_eq!(
            json_schema_to_ts(&json!({ "type": "integer" }), 0),
            "number"
        );
        assert_eq!(
            json_schema_to_ts(&json!({ "type": "boolean" }), 0),
            "boolean"
        );
        assert_eq!(json_schema_to_ts(&json!({ "type": "null" }), 0), "null");
    }

    #[test]
    fn ts_array_of_strings() {
        let s = json_schema_to_ts(
            &json!({ "type": "array", "items": { "type": "string" } }),
            0,
        );
        assert_eq!(s, "string[]");
    }

    #[test]
    fn ts_array_of_unions_uses_parens() {
        let s = json_schema_to_ts(
            &json!({
                "type": "array",
                "items": { "enum": ["a", "b", "c"] }
            }),
            0,
        );
        assert_eq!(s, "(\"a\" | \"b\" | \"c\")[]");
    }

    #[test]
    fn ts_object_with_required_and_optional_fields() {
        let schema = json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "sidebar_position": { "type": "number" },
                "draft": { "type": "boolean" }
            },
            "required": ["title"]
        });
        let s = json_schema_to_ts(&schema, 0);
        assert!(s.starts_with("{\n"));
        assert!(s.contains("title: string;"));
        assert!(s.contains("sidebar_position?: number;"));
        assert!(s.contains("draft?: boolean;"));
    }

    #[test]
    fn ts_enum_of_string_literals() {
        let s = json_schema_to_ts(&json!({ "enum": ["draft", "published"] }), 0);
        assert_eq!(s, "\"draft\" | \"published\"");
    }

    #[test]
    fn ts_nested_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "author": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "url": { "type": "string" }
                    },
                    "required": ["name"]
                }
            },
            "required": ["author"]
        });
        let s = json_schema_to_ts(&schema, 0);
        assert!(s.contains("author: {"));
        assert!(s.contains("name: string;"));
        assert!(s.contains("url?: string;"));
    }

    #[test]
    fn ts_quoted_property_for_non_ident() {
        let schema = json!({
            "type": "object",
            "properties": {
                "kebab-cased": { "type": "string" }
            },
            "required": ["kebab-cased"]
        });
        let s = json_schema_to_ts(&schema, 0);
        assert!(s.contains("\"kebab-cased\": string;"));
    }

    #[test]
    fn ts_unknown_for_empty_or_non_object() {
        assert_eq!(json_schema_to_ts(&json!({}), 0), "unknown");
        assert_eq!(json_schema_to_ts(&json!(true), 0), "unknown");
        assert_eq!(json_schema_to_ts(&json!(null), 0), "unknown");
    }

    #[test]
    fn ts_object_without_properties_renders_record() {
        assert_eq!(
            json_schema_to_ts(&json!({ "type": "object" }), 0),
            "Record<string, unknown>"
        );
    }

    // ---- validator ----

    #[test]
    fn validate_accepts_matching_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "draft": { "type": "boolean" }
            },
            "required": ["title"]
        });
        let value = json!({ "title": "hi", "draft": true });
        assert!(validate(&value, &schema).is_empty());
    }

    #[test]
    fn validate_reports_missing_required_field() {
        let schema = json!({
            "type": "object",
            "properties": { "title": { "type": "string" } },
            "required": ["title"]
        });
        let value = json!({});
        let issues = validate(&value, &schema);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "title");
        assert!(issues[0].message.contains("missing"));
    }

    #[test]
    fn validate_reports_type_mismatch_with_path() {
        let schema = json!({
            "type": "object",
            "properties": {
                "sidebar_position": { "type": "number" }
            }
        });
        let value = json!({ "sidebar_position": "not-a-number" });
        let issues = validate(&value, &schema);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "sidebar_position");
        assert!(issues[0].message.contains("expected number"));
        assert!(issues[0].message.contains("got string"));
    }

    #[test]
    fn validate_recurses_into_arrays() {
        let schema = json!({
            "type": "object",
            "properties": {
                "tags": { "type": "array", "items": { "type": "string" } }
            }
        });
        let value = json!({ "tags": ["ok", 7, "also-ok"] });
        let issues = validate(&value, &schema);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "tags[1]");
    }

    #[test]
    fn validate_enum_constraint() {
        let schema = json!({
            "type": "object",
            "properties": {
                "status": { "enum": ["draft", "published"] }
            }
        });
        let value = json!({ "status": "wip" });
        let issues = validate(&value, &schema);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "status");
        assert!(issues[0].message.contains("expected one of"));
    }

    #[test]
    fn validate_no_constraint_accepts_anything() {
        let schema = json!({});
        assert!(validate(&json!(42), &schema).is_empty());
        assert!(validate(&json!("hi"), &schema).is_empty());
    }
}
