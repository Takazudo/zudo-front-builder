//! Directive definition types shared between `zfb-content` and `zfb-md-extras`.
//!
//! Lives in `zfb-md-ast` so `zfb-md-extras` (which cannot depend on
//! `zfb-content` without creating a cycle) can produce
//! `Vec<DirectiveDef>` for the presets it exports.
//!
//! `DirectiveRegistry` and the full MDX expansion engine remain in
//! `zfb-content::plugins::directives` — they implement `MdastVisitor`
//! which is only meaningful in that crate's build context.
//! `zfb-content` re-exports all types below from its own
//! `plugins::directives` module so existing import paths continue to
//! compile unchanged.

use std::collections::HashMap;

/// Which CommonMark-Directives shape this directive responds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveKind {
    /// `:::name … :::` (block, with body).
    Container,
    /// `::name[label]{attrs}` (block, no body).
    Leaf,
    /// `:name[label]{attrs}` (inline).
    Text,
}

/// The expected type for a directive attribute value.
///
/// Used in [`AttrSchema`] to validate raw string attrs during expansion.
/// Type coercion is one-way: the raw string from the MDX source is parsed
/// into the declared type. On failure a [`DirectiveDiagnostic`] is emitted
/// (warning, not error — expansion falls back to raw attr pass-through).
#[derive(Debug, Clone)]
pub enum AttrType {
    /// Any string value is accepted verbatim.
    String,
    /// Value must be one of the declared enum variants (case-sensitive).
    ///
    /// The `Vec<String>` holds the allowed values; if the author supplies
    /// a value not in this list a diagnostic is emitted.
    Enum(Vec<std::string::String>),
    /// Value must be `"true"` or `"false"` (case-insensitive). A bare
    /// boolean attribute (no `=value`, empty string from parser) also
    /// counts as `true`. Emits as the string `"true"` / `"false"` in JSX
    /// (v1 only supports string-literal attributes).
    Boolean,
    /// Value must parse as a 64-bit float (`f64::from_str`). The
    /// validated [`ValidatedAttrValue`] stores the **original string**,
    /// not the numeric value, so the JSX emitter can pass it through
    /// unchanged. Diagnostic is emitted if the string is not parseable.
    Number,
}

/// Schema entry for one attribute on a [`DirectiveDef`].
///
/// Declare one per accepted attribute. Attributes not listed in the
/// schema are **warnings** (not errors) — unknown attrs still pass
/// through to the JSX element unchanged.
///
/// Unknown-attr policy: **warning** (preserves existing leniency).
/// Rationale: a typo in an attribute name should surface as a diagnostic,
/// not break a page build. The author can correct it without having to
/// unblock the entire pipeline.
#[derive(Debug, Clone)]
pub struct AttrSchema {
    /// Attribute name as written in the MDX source (e.g. `"tone"`, `"data-foo"`).
    pub name: std::string::String,
    /// Expected type; controls how the raw string is validated.
    pub ty: AttrType,
    /// Default value applied when the attr is absent and `required` is
    /// `false`. `None` means no default.
    pub default: Option<std::string::String>,
    /// If `true`, a missing attr (no default) triggers a diagnostic.
    pub required: bool,
}

/// Fully-validated attr value produced by [`DirectiveDef::validate_attrs`].
///
/// v1 note: even after validation, the JSX emitter represents every
/// attribute as a string literal ([`AttributeValue::Literal`]). The
/// typed enum is available for callers that want to inspect the
/// semantically-correct value (e.g. to pass a boolean `true` to the
/// `hProperties` map without wrapping in quotes). Boolean normalises to
/// `"true"`/`"false"`; Number stores the original source string (already
/// validated as parseable).
#[derive(Debug, Clone, PartialEq)]
pub enum ValidatedAttrValue {
    /// Validated string attribute.
    String(std::string::String),
    /// Validated enum attribute (value is one of the declared variants).
    Enum(std::string::String),
    /// Validated boolean; normalised to `true` / `false`.
    Boolean(bool),
    /// Validated number; original source string stored (parseable as f64).
    Number(std::string::String),
}

impl ValidatedAttrValue {
    /// Convert to the string form used in JSX `AttributeValue::Literal`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            ValidatedAttrValue::String(s)
            | ValidatedAttrValue::Enum(s)
            | ValidatedAttrValue::Number(s) => s.as_str(),
            ValidatedAttrValue::Boolean(b) => {
                if *b {
                    "true"
                } else {
                    "false"
                }
            }
        }
    }
}

/// A non-fatal diagnostic produced while expanding directives.
///
/// File path is intentionally not stored here: the registry visitor sees
/// only an mdast tree and has no idea which file it came from. The
/// orchestrator pairs the diagnostic with the file it just processed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectiveDiagnostic {
    /// Human-readable message.
    pub message: String,
    /// 1-based line of the offending construct, if known.
    pub line: Option<usize>,
    /// 1-based column of the offending construct, if known.
    pub column: Option<usize>,
}

/// Return type of [`DirectiveDef::validate_attrs`].
///
/// Tuple of:
/// - `Result<map, errors>` — `Ok` with validated attr map on success;
///   `Err` with hard-error diagnostics on required-missing or type-coercion
///   failure.
/// - `Vec<DirectiveDiagnostic>` — warning-only diagnostics (unknown attrs)
///   emitted regardless of `Ok`/`Err`.
pub type AttrValidationResult = (
    Result<HashMap<String, ValidatedAttrValue>, Vec<DirectiveDiagnostic>>,
    Vec<DirectiveDiagnostic>,
);

/// A registered directive: its source-side `name` and the JSX component
/// it expands to.
#[derive(Debug, Clone)]
pub struct DirectiveDef {
    /// Lowercase source-side name (`note`, `card`, …).
    pub name: String,
    /// Container/leaf/text shape.
    pub kind: DirectiveKind,
    /// JSX component identifier (`Note`, `Card`, …).
    pub component_name: String,
    /// If true, the bracketed `[label]` is promoted to a `title="…"`
    /// attribute on the emitted JSX (and is NOT also emitted as a
    /// child). If false, the `[label]` becomes a single Text child of
    /// the element.
    pub title_from_label: bool,
    /// Typed attribute schema. Empty by default — all existing call sites
    /// that use the `container`/`leaf`/`text` constructors or struct
    /// literals without this field compile unchanged.
    ///
    /// When non-empty, [`DirectiveDef::validate_attrs`] validates raw
    /// attrs from the MDX source against this schema during expansion.
    pub attrs: Vec<AttrSchema>,
}

impl DirectiveDef {
    /// Convenience: container with `title_from_label=false` and no attr schema.
    #[must_use]
    pub fn container(name: impl Into<String>, component_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: DirectiveKind::Container,
            component_name: component_name.into(),
            title_from_label: false,
            attrs: Vec::new(),
        }
    }

    /// Convenience: leaf with `title_from_label=false` and no attr schema.
    #[must_use]
    pub fn leaf(name: impl Into<String>, component_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: DirectiveKind::Leaf,
            component_name: component_name.into(),
            title_from_label: false,
            attrs: Vec::new(),
        }
    }

    /// Convenience: text directive with `title_from_label=false` and no attr schema.
    #[must_use]
    pub fn text(name: impl Into<String>, component_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: DirectiveKind::Text,
            component_name: component_name.into(),
            title_from_label: false,
            attrs: Vec::new(),
        }
    }

    /// Chainable builder: attach a typed attr schema to any directive.
    #[must_use]
    pub fn with_attrs(mut self, attrs: Vec<AttrSchema>) -> Self {
        self.attrs = attrs;
        self
    }

    /// Validate raw `(key, value)` attr pairs against `self.attrs` schema.
    ///
    /// Returns a tuple:
    /// - `Ok(map)` — fully-validated attr map with defaults applied, OR
    /// - `Err(error_diags)` — one or more hard errors (missing required attr,
    ///   type-coercion failure). The map is empty on `Err`.
    /// - `warn_diags` — warning-only diagnostics (unknown attrs) emitted
    ///   regardless of `Ok`/`Err`.
    ///
    /// **Unknown attrs** (not in schema) are gathered as warnings only —
    /// they do NOT cause `Err`. See [`AttrSchema`] doc comment for policy.
    pub fn validate_attrs(&self, raw: &[(String, String)]) -> AttrValidationResult {
        // If no schema is declared, skip validation entirely.
        if self.attrs.is_empty() {
            return (Ok(HashMap::new()), Vec::new());
        }

        let mut validated: HashMap<String, ValidatedAttrValue> = HashMap::new();
        let mut errors: Vec<DirectiveDiagnostic> = Vec::new();
        let mut warnings: Vec<DirectiveDiagnostic> = Vec::new();

        let raw_map: HashMap<&str, &str> =
            raw.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

        let schema_names: std::collections::HashSet<&str> =
            self.attrs.iter().map(|s| s.name.as_str()).collect();
        for (raw_key, _) in raw {
            if !schema_names.contains(raw_key.as_str()) {
                warnings.push(DirectiveDiagnostic {
                    message: format!(
                        "directive `{}`: unknown attribute `{raw_key}` (warning only — \
                         attr passes through unchanged)",
                        self.name
                    ),
                    line: None,
                    column: None,
                });
            }
        }

        for schema in &self.attrs {
            let raw_val: Option<&str> = raw_map.get(schema.name.as_str()).copied();

            match raw_val {
                None => {
                    if let Some(ref default) = schema.default {
                        match coerce_value(&schema.ty, default.as_str()) {
                            Ok(v) => {
                                validated.insert(schema.name.clone(), v);
                            }
                            Err(msg) => {
                                errors.push(DirectiveDiagnostic {
                                    message: format!(
                                        "directive `{}`: attr `{}` default value `{default}` \
                                         is not valid for type {:?}: {msg}",
                                        self.name, schema.name, schema.ty,
                                    ),
                                    line: None,
                                    column: None,
                                });
                            }
                        }
                    } else if schema.required {
                        errors.push(DirectiveDiagnostic {
                            message: format!(
                                "directive `{}`: required attribute `{}` is missing",
                                self.name, schema.name
                            ),
                            line: None,
                            column: None,
                        });
                    }
                }
                Some(val) => match coerce_value(&schema.ty, val) {
                    Ok(v) => {
                        validated.insert(schema.name.clone(), v);
                    }
                    Err(msg) => {
                        errors.push(DirectiveDiagnostic {
                            message: format!(
                                "directive `{}`: attr `{}` value `{val}` is not valid \
                                     for type {:?}: {msg}",
                                self.name, schema.name, schema.ty,
                            ),
                            line: None,
                            column: None,
                        });
                    }
                },
            }
        }

        let result = if errors.is_empty() {
            Ok(validated)
        } else {
            Err(errors)
        };
        (result, warnings)
    }
}

/// Coerce a raw string value to the declared `AttrType`.
fn coerce_value(ty: &AttrType, raw: &str) -> Result<ValidatedAttrValue, String> {
    match ty {
        AttrType::String => Ok(ValidatedAttrValue::String(raw.to_string())),
        AttrType::Enum(variants) => {
            if variants.iter().any(|v| v == raw) {
                Ok(ValidatedAttrValue::Enum(raw.to_string()))
            } else {
                Err(format!(
                    "expected one of [{}], got `{raw}`",
                    variants.join(", ")
                ))
            }
        }
        AttrType::Boolean => match raw.to_lowercase().as_str() {
            "true" | "" => Ok(ValidatedAttrValue::Boolean(true)),
            "false" => Ok(ValidatedAttrValue::Boolean(false)),
            _ => Err(format!("expected `true` or `false`, got `{raw}`")),
        },
        AttrType::Number => {
            if raw.parse::<f64>().is_ok() {
                Ok(ValidatedAttrValue::Number(raw.to_string()))
            } else {
                Err(format!("expected a number, got `{raw}`"))
            }
        }
    }
}
