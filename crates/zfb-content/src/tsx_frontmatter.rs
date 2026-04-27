//! TSX `export const frontmatter` static extraction.
//!
//! Walks a `.tsx` module via [`swc_core`] and statically extracts the
//! top-level
//!
//! ```ts
//! export const frontmatter = { /* literal-only object */ };
//! ```
//!
//! into a [`serde_json::Value`]. Sibling literal exports
//! `extension` and `contentType` (both string literals) are also
//! captured so the rest of the engine can consult them when deciding
//! the output filename / response type for a TSX page.
//!
//! ### Literal-only contract
//!
//! Identifiers, calls, member accesses, spreads, computed keys,
//! template strings with substitutions, regular expressions, and any
//! other "computed" value are **rejected** — they cannot be evaluated
//! without executing the module, and this engine is explicitly
//! AST-only. Each rejection points at the offending node's source
//! location (file + line:column) so the engine can surface a useful
//! diagnostic to the page author.
//!
//! Allowed value shapes:
//!
//! - String, number (including unary `+` / `-` numeric literal),
//!   boolean, `null`.
//! - Object literal (recursively).
//! - Array literal (recursively); array holes are rejected.
//! - Template strings **without** substitutions
//!   (`` `hello world` `` is allowed; `` `hi ${x}` `` is not).
//!
//! ### Filename rule (shared with Sub 6)
//!
//! TSX page filenames may carry a candidate output extension as the
//! last `.`-separated segment before `.tsx`:
//!
//! ```text
//! foo.bar.baz.tsx  → "baz"
//! page.html.tsx    → "html"
//! page.tsx         → None
//! ```
//!
//! Any earlier dots are part of the page name. See
//! [`filename_extension_candidate`].

use std::path::PathBuf;

use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap, Span, Spanned};
use swc_core::ecma::ast::{
    Decl, EsVersion, Expr, Lit, ModuleDecl, ModuleItem, ObjectLit, Pat, Prop, PropName,
    PropOrSpread, UnaryOp, VarDeclKind,
};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use thiserror::Error;

/// Output of [`extract`]. A successful extraction always produces a
/// `frontmatter` (the required export); the two sibling exports are
/// optional and absent when the source did not declare them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsxFrontmatter {
    /// Parsed value of `export const frontmatter = …` as JSON. Always
    /// present on success; missing/non-object cases surface as
    /// [`TsxFrontmatterError`].
    pub frontmatter: JsonValue,
    /// Value of the optional `export const extension = "…"` literal,
    /// if declared. The engine layers this on top of the filename rule
    /// (see [`filename_extension_candidate`]) when picking the page's
    /// output extension.
    pub extension: Option<String>,
    /// Value of the optional `export const contentType = "…"` literal,
    /// if declared. Drives `Content-Type` for non-HTML page outputs.
    pub content_type: Option<String>,
}

/// All ways extraction can fail. Every variant names the offending
/// `file_name` (forwarded by the caller) so diagnostics can be glued
/// together by the surrounding pipeline.
#[derive(Debug, Error)]
pub enum TsxFrontmatterError {
    /// The SWC parser refused the source. Surfaces parser diagnostics
    /// as a single string instead of letting them panic up the stack.
    #[error("{file}: parse error: {message}")]
    Parse { file: String, message: String },

    /// No top-level `export const frontmatter` was found. The engine
    /// requires a frontmatter export on every TSX page.
    #[error("{file}: missing required `export const frontmatter`")]
    MissingFrontmatter { file: String },

    /// The source declared `export const <name>` more than once at the
    /// top level. We refuse to silently pick one.
    #[error("{file}:{line}:{col}: duplicate top-level `export const {name}`")]
    DuplicateExport {
        file: String,
        name: String,
        line: usize,
        col: usize,
    },

    /// `export const <name>` was declared but its initializer (or some
    /// nested value inside it) is not a literal. The error names the
    /// export, the file, the offending span, and a short reason that
    /// describes which non-literal shape was encountered.
    #[error("{file}:{line}:{col}: non-literal value not allowed in `export const {export}` ({reason})")]
    ComputedValue {
        file: String,
        export: String,
        reason: String,
        line: usize,
        col: usize,
    },

    /// `export const <name>` is in the wrong shape — `frontmatter`
    /// must be an object literal, `extension`/`contentType` must be
    /// string literals, and the binding must be a plain identifier
    /// (no destructuring, no missing initializer, etc.).
    #[error("{file}:{line}:{col}: `export const {export}` {reason}")]
    WrongShape {
        file: String,
        export: String,
        reason: String,
        line: usize,
        col: usize,
    },
}

/// Parse `source` as a `.tsx` module and statically extract its
/// frontmatter export plus the optional `extension` / `contentType`
/// siblings. `file_name` is purely cosmetic — it shows up in error
/// messages so the caller can pinpoint the offending file.
///
/// This function never executes the source; it walks the SWC AST.
/// Parser failures are surfaced as
/// [`TsxFrontmatterError::Parse`] rather than panics.
pub fn extract(source: &str, file_name: &str) -> Result<TsxFrontmatter, TsxFrontmatterError> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Real(PathBuf::from(file_name)).into(),
        source.to_string(),
    );

    // SWC parser diagnostics live on the returned `Err` value below;
    // no `Handler` is wired up because this crate is a library, not a
    // CLI, and we don't want stray prints to stderr on malformed input.

    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: true,
            decorators: false,
            dts: false,
            no_early_errors: false,
            disallow_ambiguous_jsx_like: false,
        }),
        EsVersion::Es2022,
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);

    let module = parser
        .parse_module()
        .map_err(|e| TsxFrontmatterError::Parse {
            file: file_name.to_string(),
            message: format!("{e:?}"),
        })?;

    let ctx = Ctx {
        file_name,
        source_map: &cm,
    };

    let mut frontmatter: Option<JsonValue> = None;
    let mut extension: Option<String> = None;
    let mut content_type: Option<String> = None;
    // Track "have we already seen this export?" separately from the
    // value slot. We can't rely on `frontmatter.is_some()` because a
    // failed extraction (e.g. duplicate after a good one) must still
    // report against the *second* declaration; flipping these flags
    // before parsing the new initializer keeps that ordering honest.
    let mut frontmatter_seen = false;
    let mut extension_seen = false;
    let mut content_type_seen = false;

    for item in module.body.iter() {
        // Only `ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(...))`
        // matters here; everything else is a top-level statement,
        // import, default export, type-only export, etc., none of
        // which can introduce a top-level `export const NAME = …`.
        let export = match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => e,
            _ => continue,
        };

        // We ignore non-`var` declarations entirely — `export function`,
        // `export class`, `export type`, `export interface`, etc. all
        // have their own AST shapes and never declare a literal value.
        let var_decl = match &export.decl {
            Decl::Var(v) => v,
            _ => continue,
        };

        // The engine only honors `const`, not `let` / `var`. Mutable
        // bindings would invite the page author to assume the value is
        // computed at runtime — which would be untrue under static
        // extraction.
        if !matches!(var_decl.kind, VarDeclKind::Const) {
            continue;
        }

        // A single declaration can introduce multiple bindings:
        // `export const a = 1, b = 2;`. Process each independently so
        // each gets its own diagnostic on failure.
        for declarator in &var_decl.decls {
            // Only plain identifier bindings are recognized. Anything
            // exotic (`export const { a, b } = …`, `export const [a] = …`)
            // is silently ignored so the page may still declare other
            // (irrelevant) destructured exports without us complaining.
            let ident = match &declarator.name {
                Pat::Ident(bi) => bi,
                _ => continue,
            };
            let name_str = ident.id.sym.as_ref();

            // Only the three names we care about. Other top-level
            // `export const` bindings (state, helpers, …) are ignored
            // — they're none of our business.
            let target = match name_str {
                "frontmatter" => ExportTarget::Frontmatter,
                "extension" => ExportTarget::Extension,
                "contentType" => ExportTarget::ContentType,
                _ => continue,
            };

            // All three exports must have an initializer. `export const
            // frontmatter;` is a syntax error in TS already, but we
            // defend against the AST shape just in case.
            let init = match &declarator.init {
                Some(e) => e,
                None => {
                    return Err(TsxFrontmatterError::WrongShape {
                        file: file_name.to_string(),
                        export: target.name().to_string(),
                        reason: "must have an initializer".to_string(),
                        line: ctx.line(declarator.span()).0,
                        col: ctx.line(declarator.span()).1,
                    });
                }
            };

            // Detect duplicates BEFORE we parse the value — this gives
            // the author a span pointing at the *second* declaration,
            // which is the one they need to delete.
            let already_seen = match target {
                ExportTarget::Frontmatter => frontmatter_seen,
                ExportTarget::Extension => extension_seen,
                ExportTarget::ContentType => content_type_seen,
            };
            if already_seen {
                let (line, col) = ctx.line(declarator.span());
                return Err(TsxFrontmatterError::DuplicateExport {
                    file: file_name.to_string(),
                    name: target.name().to_string(),
                    line,
                    col,
                });
            }
            match target {
                ExportTarget::Frontmatter => frontmatter_seen = true,
                ExportTarget::Extension => extension_seen = true,
                ExportTarget::ContentType => content_type_seen = true,
            }

            match target {
                ExportTarget::Frontmatter => {
                    let obj = expect_object(init, target.name(), &ctx)?;
                    frontmatter = Some(object_to_json(obj, target.name(), &ctx)?);
                }
                ExportTarget::Extension => {
                    extension = Some(expect_string(init, target.name(), &ctx)?);
                }
                ExportTarget::ContentType => {
                    content_type = Some(expect_string(init, target.name(), &ctx)?);
                }
            }
        }
    }

    let frontmatter = frontmatter.ok_or_else(|| TsxFrontmatterError::MissingFrontmatter {
        file: file_name.to_string(),
    })?;

    Ok(TsxFrontmatter {
        frontmatter,
        extension,
        content_type,
    })
}

/// Filename-rule helper: extract the candidate output extension from a
/// TSX page filename, where only the last `.`-separated segment before
/// `.tsx` counts. See module docs for details.
///
/// This intentionally takes a `&str` (not a `Path`) because the rule
/// is purely lexical — directory separators, leading dots, and so on
/// are the caller's job.
pub fn filename_extension_candidate(file_name: &str) -> Option<&str> {
    // Use the basename only — leading directories must not influence
    // the result (e.g. `posts/2026.04/page.html.tsx` → `"html"`).
    let base = match file_name.rsplit_once('/') {
        Some((_, rest)) => rest,
        None => file_name,
    };
    let stem = base.strip_suffix(".tsx")?;
    // A bare `.tsx` (e.g. file_name == ".tsx") leaves an empty stem;
    // there is no candidate extension to return.
    if stem.is_empty() {
        return None;
    }
    let dot = stem.rfind('.')?;
    let candidate = &stem[dot + 1..];
    if candidate.is_empty() {
        // `foo..tsx` — trailing dot before `.tsx`; not a real candidate.
        None
    } else {
        Some(candidate)
    }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Which of the three recognized exports we're currently extracting.
/// Carrying this as an enum lets each helper render the right name in
/// diagnostics without the caller passing strings around by hand.
#[derive(Debug, Clone, Copy)]
enum ExportTarget {
    Frontmatter,
    Extension,
    ContentType,
}

impl ExportTarget {
    fn name(&self) -> &'static str {
        match self {
            ExportTarget::Frontmatter => "frontmatter",
            ExportTarget::Extension => "extension",
            ExportTarget::ContentType => "contentType",
        }
    }
}

/// Bundled "extraction context" — everything an internal helper needs
/// to render a span-aware error without each one growing a long
/// parameter list.
struct Ctx<'a> {
    file_name: &'a str,
    source_map: &'a Lrc<SourceMap>,
}

impl<'a> Ctx<'a> {
    /// Resolve a `Span` to a (line, column) pair (both 1-based). When
    /// SWC can't resolve the span (e.g. `DUMMY_SP`) we fall back to
    /// `(1, 1)` rather than panicking — the diagnostic is degraded but
    /// the extraction still completes.
    fn line(&self, span: Span) -> (usize, usize) {
        match self.source_map.try_lookup_char_pos(span.lo()) {
            Ok(loc) => (loc.line, loc.col_display + 1),
            Err(_) => (1, 1),
        }
    }

    fn computed(&self, span: Span, export: &str, reason: &str) -> TsxFrontmatterError {
        let (line, col) = self.line(span);
        TsxFrontmatterError::ComputedValue {
            file: self.file_name.to_string(),
            export: export.to_string(),
            reason: reason.to_string(),
            line,
            col,
        }
    }

    fn wrong_shape(&self, span: Span, export: &str, reason: &str) -> TsxFrontmatterError {
        let (line, col) = self.line(span);
        TsxFrontmatterError::WrongShape {
            file: self.file_name.to_string(),
            export: export.to_string(),
            reason: reason.to_string(),
            line,
            col,
        }
    }
}

/// Strip wrappers that don't change the value: parentheses around an
/// expression, and TS-specific "is this really a T" annotations like
/// `value as Frontmatter` / `value satisfies Frontmatter` /
/// `value!`. These are common in real TSX pages (the type checker
/// likes them) and removing them up front keeps the recursive
/// converter focused on actual literal shapes.
fn unwrap_ts_wrappers(expr: &Expr) -> &Expr {
    let mut cur = expr;
    loop {
        cur = match cur {
            Expr::Paren(p) => &p.expr,
            Expr::TsAs(a) => &a.expr,
            Expr::TsSatisfies(s) => &s.expr,
            Expr::TsConstAssertion(c) => &c.expr,
            Expr::TsNonNull(n) => &n.expr,
            Expr::TsTypeAssertion(t) => &t.expr,
            _ => return cur,
        };
    }
}

/// Expect an expression to be an object literal (after stripping TS
/// wrappers). Anything else is reported as a wrong-shape error
/// against the export name.
fn expect_object<'a>(
    expr: &'a Expr,
    export: &str,
    ctx: &Ctx<'_>,
) -> Result<&'a ObjectLit, TsxFrontmatterError> {
    match unwrap_ts_wrappers(expr) {
        Expr::Object(obj) => Ok(obj),
        other => Err(ctx.wrong_shape(other.span(), export, "must be an object literal")),
    }
}

/// Expect an expression to be a string literal (or a substitution-free
/// template literal). `extension` and `contentType` go through here.
fn expect_string(
    expr: &Expr,
    export: &str,
    ctx: &Ctx<'_>,
) -> Result<String, TsxFrontmatterError> {
    match unwrap_ts_wrappers(expr) {
        Expr::Lit(Lit::Str(s)) => Ok(wtf8_to_string(&s.value)),
        Expr::Tpl(tpl) if tpl.exprs.is_empty() => Ok(tpl_quasi_to_string(tpl)),
        Expr::Tpl(tpl) => Err(ctx.computed(
            tpl.span,
            export,
            "template strings with substitutions are not allowed",
        )),
        other => Err(ctx.wrong_shape(other.span(), export, "must be a string literal")),
    }
}

/// Convert an SWC `ObjectLit` to `serde_json::Value::Object`. Keys
/// must be plain identifiers, string literals, or numeric literals;
/// computed keys (`[foo]: …`), spreads (`...rest`), getters/setters,
/// and method shorthand (`foo() { … }`) are rejected.
fn object_to_json(
    obj: &ObjectLit,
    export: &str,
    ctx: &Ctx<'_>,
) -> Result<JsonValue, TsxFrontmatterError> {
    let mut map = JsonMap::with_capacity(obj.props.len());
    for prop in &obj.props {
        match prop {
            PropOrSpread::Spread(spread) => {
                return Err(ctx.computed(
                    spread.dot3_token,
                    export,
                    "object spread is not allowed",
                ));
            }
            PropOrSpread::Prop(boxed) => match &**boxed {
                Prop::KeyValue(kv) => {
                    let key = prop_name_to_string(&kv.key, export, ctx)?;
                    let value = expr_to_json(&kv.value, export, ctx)?;
                    // Preserve the *first* occurrence on duplicate
                    // keys — JS itself silently overwrites, but for
                    // diagnostics it's better to be deterministic and
                    // use insertion order. `serde_json::Map` already
                    // does that; just .insert() last-write-wins.
                    map.insert(key, value);
                }
                Prop::Shorthand(ident) => {
                    // `{ foo }` desugars to `{ foo: foo }` — that
                    // second `foo` is an identifier reference, which
                    // requires runtime evaluation. Reject.
                    return Err(ctx.computed(
                        ident.span,
                        export,
                        "shorthand property references a variable, not a literal",
                    ));
                }
                Prop::Method(m) => {
                    return Err(ctx.computed(
                        m.key.span(),
                        export,
                        "method shorthand is not a literal value",
                    ));
                }
                Prop::Getter(g) => {
                    return Err(ctx.computed(
                        g.key.span(),
                        export,
                        "getter is not a literal value",
                    ));
                }
                Prop::Setter(s) => {
                    return Err(ctx.computed(
                        s.key.span(),
                        export,
                        "setter is not a literal value",
                    ));
                }
                Prop::Assign(a) => {
                    return Err(ctx.computed(
                        a.span,
                        export,
                        "assignment property is not a literal value",
                    ));
                }
            },
        }
    }
    Ok(JsonValue::Object(map))
}

/// Convert an SWC property name into a JSON object key. Computed keys
/// (`[expr]: …`) and `BigInt` keys are rejected — both require either
/// runtime evaluation or a number type JSON cannot represent.
fn prop_name_to_string(
    name: &PropName,
    export: &str,
    ctx: &Ctx<'_>,
) -> Result<String, TsxFrontmatterError> {
    match name {
        PropName::Ident(i) => Ok(i.sym.as_str().to_owned()),
        PropName::Str(s) => Ok(wtf8_to_string(&s.value)),
        PropName::Num(n) => Ok(format_number(n.value)),
        PropName::BigInt(b) => Err(ctx.computed(
            b.span,
            export,
            "BigInt property keys are not representable in JSON",
        )),
        PropName::Computed(c) => Err(ctx.computed(
            c.span,
            export,
            "computed property keys are not allowed",
        )),
    }
}

/// Convert an arbitrary expression into a JSON value. The set of
/// recognized shapes is intentionally narrow — anything outside it is
/// a "computed" value and is rejected with a span-aware error.
fn expr_to_json(
    expr: &Expr,
    export: &str,
    ctx: &Ctx<'_>,
) -> Result<JsonValue, TsxFrontmatterError> {
    let expr = unwrap_ts_wrappers(expr);
    match expr {
        Expr::Lit(lit) => lit_to_json(lit, export, ctx),
        Expr::Object(obj) => object_to_json(obj, export, ctx),
        Expr::Array(arr) => {
            let mut out = Vec::with_capacity(arr.elems.len());
            for elem in &arr.elems {
                match elem {
                    Some(item) => {
                        if let Some(spread_span) = item.spread {
                            return Err(ctx.computed(
                                spread_span,
                                export,
                                "array spread is not allowed",
                            ));
                        }
                        out.push(expr_to_json(&item.expr, export, ctx)?);
                    }
                    None => {
                        // `[1, , 3]` — a "hole". JSON has no concept
                        // of holes, and silently coercing one into
                        // `null` would lie about what the source said.
                        return Err(ctx.computed(
                            arr.span,
                            export,
                            "array holes are not allowed",
                        ));
                    }
                }
            }
            Ok(JsonValue::Array(out))
        }
        Expr::Tpl(tpl) => {
            if !tpl.exprs.is_empty() {
                return Err(ctx.computed(
                    tpl.span,
                    export,
                    "template strings with substitutions are not allowed",
                ));
            }
            Ok(JsonValue::String(tpl_quasi_to_string(tpl)))
        }
        Expr::Unary(u) => match u.op {
            // `-1`, `+1`, `-Infinity` (still rejected later because it
            // can't be encoded as JSON), `+0`. Note that `-1` parses
            // as `Unary(Minus, Lit(Num(1)))`, not `Lit(Num(-1))`.
            UnaryOp::Minus | UnaryOp::Plus => match &*u.arg {
                Expr::Lit(Lit::Num(n)) => {
                    let value = if matches!(u.op, UnaryOp::Minus) {
                        -n.value
                    } else {
                        n.value
                    };
                    number_to_json(value).ok_or_else(|| {
                        ctx.computed(
                            u.span,
                            export,
                            "non-finite number is not representable in JSON",
                        )
                    })
                }
                _ => Err(ctx.computed(
                    u.span,
                    export,
                    "unary operator only allowed in front of a numeric literal",
                )),
            },
            _ => Err(ctx.computed(
                u.span,
                export,
                "unary operator is not a literal value",
            )),
        },
        // Everything below requires runtime evaluation or is otherwise
        // not a literal; report with a kind-specific reason so the
        // page author sees what they used.
        Expr::Ident(i) => Err(ctx.computed(
            i.span,
            export,
            "identifier reference is not a literal",
        )),
        Expr::Call(c) => Err(ctx.computed(c.span, export, "function call is not a literal")),
        Expr::New(n) => Err(ctx.computed(n.span, export, "`new` expression is not a literal")),
        Expr::Member(m) => Err(ctx.computed(
            m.span,
            export,
            "member access is not a literal",
        )),
        Expr::Bin(b) => Err(ctx.computed(
            b.span,
            export,
            "binary expression is not a literal",
        )),
        Expr::Cond(c) => Err(ctx.computed(
            c.span,
            export,
            "conditional expression is not a literal",
        )),
        Expr::Arrow(a) => Err(ctx.computed(a.span, export, "arrow function is not a literal")),
        Expr::Fn(f) => Err(ctx.computed(
            f.function.span,
            export,
            "function expression is not a literal",
        )),
        Expr::Class(c) => Err(ctx.computed(
            c.class.span,
            export,
            "class expression is not a literal",
        )),
        Expr::This(t) => Err(ctx.computed(t.span, export, "`this` is not a literal")),
        Expr::Update(u) => Err(ctx.computed(
            u.span,
            export,
            "update expression is not a literal",
        )),
        Expr::Assign(a) => Err(ctx.computed(
            a.span,
            export,
            "assignment expression is not a literal",
        )),
        Expr::Seq(s) => Err(ctx.computed(
            s.span,
            export,
            "sequence expression is not a literal",
        )),
        Expr::Yield(y) => Err(ctx.computed(y.span, export, "`yield` is not a literal")),
        Expr::Await(a) => Err(ctx.computed(a.span, export, "`await` is not a literal")),
        Expr::TaggedTpl(t) => Err(ctx.computed(
            t.span,
            export,
            "tagged template literal is not a literal",
        )),
        Expr::JSXElement(j) => Err(ctx.computed(
            j.span,
            export,
            "JSX element is not a literal value",
        )),
        Expr::JSXFragment(j) => Err(ctx.computed(
            j.span,
            export,
            "JSX fragment is not a literal value",
        )),
        // Catch-all for anything not enumerated above; we still emit a
        // span so the page author can find it.
        other => Err(ctx.computed(
            other.span(),
            export,
            "value is not a JSON-representable literal",
        )),
    }
}

/// Convert a single literal node into a JSON value. Regex / BigInt /
/// raw JSXText literals can't survive a round-trip through JSON, so
/// they're rejected.
fn lit_to_json(
    lit: &Lit,
    export: &str,
    ctx: &Ctx<'_>,
) -> Result<JsonValue, TsxFrontmatterError> {
    match lit {
        Lit::Str(s) => Ok(JsonValue::String(wtf8_to_string(&s.value))),
        Lit::Bool(b) => Ok(JsonValue::Bool(b.value)),
        Lit::Null(_) => Ok(JsonValue::Null),
        Lit::Num(n) => number_to_json(n.value).ok_or_else(|| {
            ctx.computed(
                n.span,
                export,
                "non-finite number is not representable in JSON",
            )
        }),
        Lit::BigInt(b) => Err(ctx.computed(
            b.span,
            export,
            "BigInt literal is not representable in JSON",
        )),
        Lit::Regex(r) => Err(ctx.computed(
            r.span,
            export,
            "regex literal is not representable in JSON",
        )),
        Lit::JSXText(j) => Err(ctx.computed(
            j.span,
            export,
            "JSX text is not a literal value",
        )),
    }
}

/// Render a substitution-free template literal as a plain string,
/// preferring the cooked value and falling back to raw if SWC didn't
/// produce a cooked atom (rare — only happens for explicit invalid
/// escape sequences in tagged templates, which we already reject).
fn tpl_quasi_to_string(tpl: &swc_core::ecma::ast::Tpl) -> String {
    debug_assert!(
        !tpl.quasis.is_empty(),
        "a substitution-free template literal still has at least one quasi",
    );
    let q = &tpl.quasis[0];
    match &q.cooked {
        Some(c) => wtf8_to_string(c),
        None => q.raw.as_str().to_owned(),
    }
}

/// Convert a `Wtf8Atom` value into a Rust `String`. Frontmatter strings
/// are not expected to contain unpaired surrogates, but we use the
/// lossy converter rather than panicking so a stray `\uD800` in some
/// page's metadata can't take down the whole build.
fn wtf8_to_string(atom: &swc_core::atoms::Wtf8Atom) -> String {
    match atom.as_atom() {
        Some(a) => a.as_str().to_owned(),
        None => atom.to_string_lossy().into_owned(),
    }
}

/// Render a numeric property key the same way `serde_json` would
/// render it as a Number — keeps integer keys integer-shaped and
/// avoids sticking `.0` onto whole numbers like JS' `Number.toString`
/// already does.
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e16 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Pick the most-faithful `serde_json::Number` representation for an
/// `f64` literal. `serde_json` stores numbers in three internal lanes
/// (`u64`, `i64`, `f64`) and which lane you're in determines whether
/// `Value::as_i64()` etc. returns `Some`. Using `from_f64` for whole
/// numbers would file `42` under the float lane and break the
/// "round-trip into `serde_json::Value`" expectation that `42` reads
/// back as an integer. Pick the integer lane when the value is a
/// whole number that fits, and fall back to `f64` otherwise.
fn number_to_json(value: f64) -> Option<JsonValue> {
    if value.is_finite() && value.fract() == 0.0 {
        // Use the integer lane when the value fits cleanly. `value as
        // i64` saturates on overflow, so guard with explicit range
        // checks against the `i64` and `u64` boundaries.
        if value >= 0.0 && value <= u64::MAX as f64 {
            return Some(JsonValue::Number(JsonNumber::from(value as u64)));
        }
        if value >= i64::MIN as f64 && value <= i64::MAX as f64 {
            return Some(JsonValue::Number(JsonNumber::from(value as i64)));
        }
    }
    JsonNumber::from_f64(value).map(JsonValue::Number)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_ok(src: &str) -> TsxFrontmatter {
        extract(src, "page.tsx").expect("extract should succeed")
    }

    #[test]
    fn primitives_round_trip() {
        let src = r#"
            export const frontmatter = {
                title: "Hello",
                draft: false,
                count: 42,
                ratio: -0.5,
                missing: null,
            };
        "#;
        let out = extract_ok(src);
        let fm = out.frontmatter.as_object().unwrap();
        assert_eq!(fm["title"], JsonValue::String("Hello".into()));
        assert_eq!(fm["draft"], JsonValue::Bool(false));
        assert_eq!(fm["count"].as_i64(), Some(42));
        assert!((fm["ratio"].as_f64().unwrap() + 0.5).abs() < f64::EPSILON);
        assert_eq!(fm["missing"], JsonValue::Null);
    }

    #[test]
    fn nested_object_and_array() {
        let src = r#"
            export const frontmatter = {
                tags: ["rust", "tsx", "frontmatter"],
                author: { name: "Alice", handle: "alice42" },
                matrix: [[1, 2], [3, 4]],
            };
        "#;
        let out = extract_ok(src);
        let fm = out.frontmatter.as_object().unwrap();
        let tags = fm["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0].as_str(), Some("rust"));
        assert_eq!(fm["author"]["name"].as_str(), Some("Alice"));
        assert_eq!(fm["matrix"][1][1].as_i64(), Some(4));
    }

    #[test]
    fn leading_comments_are_ignored() {
        let src = r#"
            // This page is the home page.
            /* The frontmatter follows. */
            export const frontmatter = { title: "Home" };
        "#;
        let out = extract_ok(src);
        assert_eq!(out.frontmatter["title"].as_str(), Some("Home"));
    }

    #[test]
    fn string_escapes_are_decoded() {
        // `é` is é; `\n` is a real newline; `\\` is a single
        // backslash. The extractor must hand back the *cooked* value,
        // not the raw source.
        let src = r#"
            export const frontmatter = {
                quote: "She said \"hi\"",
                accent: "café",
                multi: "line1\nline2",
                escaped: "a\\b",
            };
        "#;
        let out = extract_ok(src);
        let fm = out.frontmatter.as_object().unwrap();
        assert_eq!(fm["quote"].as_str(), Some("She said \"hi\""));
        assert_eq!(fm["accent"].as_str(), Some("café"));
        assert_eq!(fm["multi"].as_str(), Some("line1\nline2"));
        assert_eq!(fm["escaped"].as_str(), Some("a\\b"));
    }

    #[test]
    fn template_string_without_substitution_allowed() {
        let src = r#"
            export const frontmatter = {
                title: `Hello there`,
            };
        "#;
        let out = extract_ok(src);
        assert_eq!(out.frontmatter["title"].as_str(), Some("Hello there"));
    }

    #[test]
    fn ts_wrappers_are_transparent() {
        // `as const`, `satisfies`, and `as Frontmatter` should not
        // confuse the extractor — they're TypeScript-only. The literal
        // shape underneath is what matters.
        let src = r#"
            type Frontmatter = { title: string };
            export const frontmatter = ({ title: "T" } satisfies Frontmatter) as const;
            export const extension = "html" as const;
            export const contentType = "text/html" satisfies string;
        "#;
        let out = extract_ok(src);
        assert_eq!(out.frontmatter["title"].as_str(), Some("T"));
        assert_eq!(out.extension.as_deref(), Some("html"));
        assert_eq!(out.content_type.as_deref(), Some("text/html"));
    }

    #[test]
    fn extension_and_content_type_extracted() {
        let src = r#"
            export const frontmatter = { title: "X" };
            export const extension = "xml";
            export const contentType = "application/xml";
        "#;
        let out = extract_ok(src);
        assert_eq!(out.extension.as_deref(), Some("xml"));
        assert_eq!(out.content_type.as_deref(), Some("application/xml"));
    }

    #[test]
    fn missing_extension_and_content_type_are_none() {
        let src = r#"
            export const frontmatter = { title: "X" };
        "#;
        let out = extract_ok(src);
        assert!(out.extension.is_none());
        assert!(out.content_type.is_none());
    }

    #[test]
    fn computed_value_identifier_rejected_with_file_and_span() {
        // `title: NAME` — `NAME` is a runtime reference.
        let src = "const NAME = \"Hi\";\nexport const frontmatter = {\n  title: NAME,\n};\n";
        let err = extract(src, "blog/post.tsx").expect_err("must fail");
        let (file, export, line, col) = match err {
            TsxFrontmatterError::ComputedValue {
                file,
                export,
                line,
                col,
                ..
            } => (file, export, line, col),
            other => panic!("expected ComputedValue, got {other:?}"),
        };
        assert_eq!(file, "blog/post.tsx");
        assert_eq!(export, "frontmatter");
        // The identifier `NAME` lives on line 3 in the snippet above.
        assert_eq!(line, 3, "expected error at the `NAME` reference");
        assert!(col >= 1, "column should be 1-based");
    }

    #[test]
    fn computed_value_call_rejected() {
        let src = r#"
            function tag(s: string) { return s.toUpperCase(); }
            export const frontmatter = { title: tag("home") };
        "#;
        let err = extract(src, "page.tsx").expect_err("must fail");
        assert!(
            matches!(err, TsxFrontmatterError::ComputedValue { .. }),
            "expected ComputedValue, got {err:?}",
        );
    }

    #[test]
    fn computed_value_template_with_substitution_rejected() {
        let src = r#"
            const NAME = "world";
            export const frontmatter = { title: `hello ${NAME}` };
        "#;
        let err = extract(src, "page.tsx").expect_err("must fail");
        match err {
            TsxFrontmatterError::ComputedValue { reason, export, .. } => {
                assert_eq!(export, "frontmatter");
                assert!(
                    reason.contains("substitution"),
                    "reason should mention substitutions, got {reason:?}",
                );
            }
            other => panic!("expected ComputedValue, got {other:?}"),
        }
    }

    #[test]
    fn computed_value_spread_from_variable_rejected() {
        let src = r#"
            const base = { a: 1 };
            export const frontmatter = { ...base, b: 2 };
        "#;
        let err = extract(src, "page.tsx").expect_err("must fail");
        match err {
            TsxFrontmatterError::ComputedValue { reason, .. } => {
                assert!(
                    reason.contains("spread"),
                    "reason should mention spread, got {reason:?}",
                );
            }
            other => panic!("expected ComputedValue, got {other:?}"),
        }
    }

    #[test]
    fn shorthand_property_rejected() {
        // `{ title }` references the local `title`, which is a
        // runtime value; reject.
        let src = r#"
            const title = "X";
            export const frontmatter = { title };
        "#;
        let err = extract(src, "page.tsx").expect_err("must fail");
        assert!(matches!(err, TsxFrontmatterError::ComputedValue { .. }));
    }

    #[test]
    fn computed_property_key_rejected() {
        let src = r#"
            const KEY = "title";
            export const frontmatter = { [KEY]: "Hi" };
        "#;
        let err = extract(src, "page.tsx").expect_err("must fail");
        assert!(matches!(err, TsxFrontmatterError::ComputedValue { .. }));
    }

    #[test]
    fn missing_export_reports_file() {
        let src = "export const other = 1;\n";
        let err = extract(src, "no-fm.tsx").expect_err("must fail");
        match err {
            TsxFrontmatterError::MissingFrontmatter { file } => assert_eq!(file, "no-fm.tsx"),
            other => panic!("expected MissingFrontmatter, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_frontmatter_export_rejected() {
        let src = r#"
            export const frontmatter = { title: "A" };
            export const frontmatter = { title: "B" };
        "#;
        let err = extract(src, "dup.tsx").expect_err("must fail");
        match err {
            TsxFrontmatterError::DuplicateExport { name, line, .. } => {
                assert_eq!(name, "frontmatter");
                assert!(line >= 2, "second export should be on line >= 2");
            }
            other => panic!("expected DuplicateExport, got {other:?}"),
        }
    }

    #[test]
    fn multiple_unrelated_exports_are_ignored() {
        // The extractor must not blow up on perfectly normal helper
        // exports; only the three names it cares about are
        // significant.
        let src = r#"
            export const frontmatter = { title: "Hi" };
            export const helper = (x: number) => x + 1;
            export function Component() { return null; }
            export default function Page() { return null; }
        "#;
        let out = extract_ok(src);
        assert_eq!(out.frontmatter["title"].as_str(), Some("Hi"));
    }

    #[test]
    fn frontmatter_must_be_object_literal() {
        let src = r#"export const frontmatter = "not an object";"#;
        let err = extract(src, "page.tsx").expect_err("must fail");
        match err {
            TsxFrontmatterError::WrongShape { export, .. } => {
                assert_eq!(export, "frontmatter");
            }
            other => panic!("expected WrongShape, got {other:?}"),
        }
    }

    #[test]
    fn extension_must_be_string_literal() {
        let src = r#"
            export const frontmatter = { title: "X" };
            export const extension = 42;
        "#;
        let err = extract(src, "page.tsx").expect_err("must fail");
        match err {
            TsxFrontmatterError::WrongShape { export, .. } => {
                assert_eq!(export, "extension");
            }
            other => panic!("expected WrongShape, got {other:?}"),
        }
    }

    #[test]
    fn malformed_source_returns_parse_error_not_panic() {
        // Unterminated string literal — guaranteed to bork the parser.
        let src = "export const frontmatter = { title: \"unterminated;\n";
        let err = extract(src, "broken.tsx").expect_err("must fail");
        assert!(
            matches!(err, TsxFrontmatterError::Parse { .. }),
            "expected Parse, got {err:?}",
        );
    }

    #[test]
    fn jsx_inside_value_rejected() {
        // The page's JSX-returning component is fine (we don't look at
        // it), but a JSX expression *inside* `frontmatter` is not.
        let src = r#"
            export const frontmatter = { hero: <div /> };
        "#;
        let err = extract(src, "page.tsx").expect_err("must fail");
        assert!(matches!(err, TsxFrontmatterError::ComputedValue { .. }));
    }

    #[test]
    fn unary_minus_on_number_allowed() {
        let src = r#"
            export const frontmatter = { offset: -7, plus: +3 };
        "#;
        let out = extract_ok(src);
        assert_eq!(out.frontmatter["offset"].as_i64(), Some(-7));
        assert_eq!(out.frontmatter["plus"].as_i64(), Some(3));
    }

    // ----- filename_extension_candidate -----

    #[test]
    fn filename_rule_basic() {
        // The "shared with Sub 6" rule, encoded as the spec example.
        assert_eq!(filename_extension_candidate("foo.bar.baz.tsx"), Some("baz"));
        assert_eq!(filename_extension_candidate("page.html.tsx"), Some("html"));
        assert_eq!(filename_extension_candidate("page.xml.tsx"), Some("xml"));
    }

    #[test]
    fn filename_rule_no_dot_means_no_candidate() {
        assert_eq!(filename_extension_candidate("page.tsx"), None);
    }

    #[test]
    fn filename_rule_non_tsx_returns_none() {
        // The rule only fires for `.tsx`. Other extensions (or none)
        // yield `None`.
        assert_eq!(filename_extension_candidate("page.html"), None);
        assert_eq!(filename_extension_candidate("page"), None);
        assert_eq!(filename_extension_candidate(""), None);
    }

    #[test]
    fn filename_rule_uses_basename_only() {
        // Directory components must not influence the candidate.
        assert_eq!(
            filename_extension_candidate("posts/2026.04/page.html.tsx"),
            Some("html"),
        );
        assert_eq!(filename_extension_candidate("a.b/page.tsx"), None);
    }

    #[test]
    fn filename_rule_trailing_dot_yields_none() {
        // `foo..tsx` has an empty segment immediately before `.tsx`;
        // it is not a meaningful extension candidate.
        assert_eq!(filename_extension_candidate("foo..tsx"), None);
    }

    #[test]
    fn filename_rule_bare_tsx_yields_none() {
        // `.tsx` as the entire basename has no stem.
        assert_eq!(filename_extension_candidate(".tsx"), None);
    }
}
