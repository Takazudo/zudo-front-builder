//! Static `paths()` export extractor.
//!
//! Walks a `.tsx` page module via [`swc_core`] and tries to statically
//! extract the array literal returned by its top-level `paths` export so
//! the build can enumerate dynamic-route URLs without spinning up a JS
//! runtime.
//!
//! ## Recognized shapes
//!
//! ```ts
//! export function paths() { return [/* literal */]; }
//! export async function paths() { return [/* literal */]; }
//! export const paths = () => [/* literal */];
//! export const paths = () => { return [/* literal */]; };
//! export const paths = function () { return [/* literal */]; };
//! export const paths = async () => [/* literal */];
//! ```
//!
//! ## Result
//!
//! - [`PathsExtraction::Literal`] — the export exists and the return value
//!   was a literal array (or other JSON-representable literal). The JSON
//!   blob is handed to [`crate::paths::resolve_paths`] for URL expansion.
//! - [`PathsExtraction::NonLiteral`] — the export exists but the return
//!   value isn't statically resolvable (e.g. it `await`s an `import`,
//!   reads a content collection, calls a helper, or has more than one
//!   return statement). The caller should fall back to deferred /
//!   runtime evaluation.
//! - [`PathsExtraction::Missing`] — no top-level `paths` export was
//!   found. For a dynamic route this is a build-time error the caller
//!   surfaces verbatim; the extractor itself only reports it.
//!
//! ## Why static-only
//!
//! Running `paths()` for real requires booting a JS runtime against the
//! page bundle (the "Option B / Option C" design in the ssg-render epic
//! notes). That work is queued for a follow-up: today the bundler
//! doesn't produce a Worker-shaped bundle yet, so even the extracted
//! literal case can't be end-to-end rendered. The static extractor is
//! still useful right now because:
//!
//! 1. It unblocks unit tests of the route-enumeration plumbing without
//!    needing the embedded V8 host.
//! 2. It mirrors the literal-only extraction pattern already used by
//!    [`zfb_content::tsx_frontmatter`] for `frontmatter` / `prerender`,
//!    so page authors already understand the constraint.
//! 3. Real `paths()` exports that happen to be literal (the simplest
//!    case in the docs / templates) get a free production-quality build
//!    pass without any runtime overhead.

use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap};
use swc_core::ecma::ast::{
    BlockStmtOrExpr, Decl, EsVersion, Expr, FnExpr, Lit, ModuleDecl, ModuleItem, ObjectLit, Pat,
    Prop, PropName, PropOrSpread, ReturnStmt, Stmt, UnaryOp, VarDeclKind,
};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use thiserror::Error;

/// Outcome of [`extract_paths`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathsExtraction {
    /// Return value was a JSON-literal expression. Hand to
    /// [`crate::paths::resolve_paths`] for URL expansion.
    Literal(JsonValue),
    /// `paths` export exists but the return value can't be statically
    /// resolved. `reason` describes which non-literal shape was hit so
    /// the caller can put it in a warning.
    NonLiteral { reason: String },
    /// No top-level `paths` export found at all.
    Missing,
}

/// Errors produced while parsing the source. Anything beyond a hard
/// parse failure is reported as a [`PathsExtraction`] variant rather
/// than an error.
#[derive(Debug, Error)]
pub enum PathsExtractError {
    /// The SWC parser refused the source. Surfaces parser diagnostics
    /// as a single string instead of letting them panic up the stack.
    #[error("{file}: parse error: {message}")]
    Parse { file: String, message: String },
}

/// Try to extract a literal `paths()` return value from a TSX page.
///
/// `file_name` is purely cosmetic — it shows up in diagnostics so the
/// caller can pinpoint the offending file.
///
/// Never executes the source. Walks the SWC AST. Parser failures are
/// surfaced as [`PathsExtractError::Parse`] rather than panics.
pub fn extract_paths(source: &str, file_name: &str) -> Result<PathsExtraction, PathsExtractError> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Real(file_name.into()).into(),
        source.to_string(),
    );

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
        .map_err(|e| PathsExtractError::Parse {
            file: file_name.to_string(),
            message: format!("{e:?}"),
        })?;

    // Walk top-level items looking for either:
    //   `export function paths() { … }`
    //   `export async function paths() { … }`
    //   `export const paths = <expr>;`
    for item in &module.body {
        let export_decl = match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => e,
            _ => continue,
        };
        match &export_decl.decl {
            // export function paths() { return [...] }
            Decl::Fn(fn_decl) if fn_decl.ident.sym.as_ref() == "paths" => {
                return Ok(extract_from_block_stmt(
                    fn_decl.function.body.as_ref(),
                ));
            }
            // export const paths = <init>;
            Decl::Var(var_decl) if matches!(var_decl.kind, VarDeclKind::Const) => {
                for declarator in &var_decl.decls {
                    let ident = match &declarator.name {
                        Pat::Ident(bi) => bi,
                        _ => continue,
                    };
                    if ident.id.sym.as_ref() != "paths" {
                        continue;
                    }
                    let init = match &declarator.init {
                        Some(e) => e,
                        None => {
                            return Ok(PathsExtraction::NonLiteral {
                                reason: "`export const paths` has no initializer".to_string(),
                            });
                        }
                    };
                    return Ok(extract_from_paths_initializer(init));
                }
            }
            _ => continue,
        }
    }

    Ok(PathsExtraction::Missing)
}

/// Walk the initializer of `export const paths = <init>` and pull a
/// literal return value out of it. Recognizes arrow functions and named
/// function expressions; everything else is reported as non-literal.
fn extract_from_paths_initializer(init: &Expr) -> PathsExtraction {
    let init = unwrap_ts_wrappers(init);
    match init {
        Expr::Arrow(arrow) => match &*arrow.body {
            // `() => <literal>` — body is a bare expression.
            BlockStmtOrExpr::Expr(expr) => to_literal(expr, "paths arrow body"),
            // `() => { return <literal>; }` — body is a block.
            BlockStmtOrExpr::BlockStmt(block) => extract_from_block_stmt(Some(block)),
        },
        Expr::Fn(FnExpr { function, .. }) => extract_from_block_stmt(function.body.as_ref()),
        other => PathsExtraction::NonLiteral {
            reason: format!(
                "`paths` initializer is {} — must be a function or arrow",
                expr_kind(other)
            ),
        },
    }
}

/// Walk a function body looking for a single literal return statement at
/// the top level. Multiple returns / branches / loops are intentionally
/// NOT supported — if the page author needs that, they need the runtime
/// path.
fn extract_from_block_stmt(body: Option<&swc_core::ecma::ast::BlockStmt>) -> PathsExtraction {
    let Some(block) = body else {
        return PathsExtraction::NonLiteral {
            reason: "`paths` function has no body".to_string(),
        };
    };

    let mut return_arg: Option<&Expr> = None;
    let mut return_count = 0_usize;
    for stmt in &block.stmts {
        match stmt {
            Stmt::Return(ReturnStmt { arg, .. }) => {
                return_count += 1;
                return_arg = arg.as_deref();
            }
            // Top-level statements other than `return` are allowed only
            // if they don't shadow the return value. We accept empty
            // statements and bare `;` for robustness; everything else
            // (declarations, calls, control flow) bails out — the
            // author probably meant for those side effects to happen at
            // runtime.
            Stmt::Empty(_) => {}
            other => {
                return PathsExtraction::NonLiteral {
                    reason: format!(
                        "`paths` body contains a non-return statement ({}); only a single literal `return` is supported by static extraction",
                        stmt_kind(other),
                    ),
                };
            }
        }
    }

    if return_count == 0 {
        return PathsExtraction::NonLiteral {
            reason: "`paths` body has no return statement".to_string(),
        };
    }
    if return_count > 1 {
        return PathsExtraction::NonLiteral {
            reason: format!(
                "`paths` body has {return_count} return statements; only one literal return is supported"
            ),
        };
    }

    let arg = match return_arg {
        Some(a) => a,
        None => {
            return PathsExtraction::NonLiteral {
                reason: "`paths` returns nothing".to_string(),
            }
        }
    };
    to_literal(arg, "paths return value")
}

/// Convert a single expression into [`PathsExtraction::Literal`] if it's
/// a JSON-representable literal, else into [`PathsExtraction::NonLiteral`]
/// with a short reason naming the offending shape. `context` is folded
/// into the reason so the warning points back at the `paths` export.
fn to_literal(expr: &Expr, context: &str) -> PathsExtraction {
    match expr_to_json(unwrap_ts_wrappers(expr)) {
        Ok(value) => PathsExtraction::Literal(value),
        Err(reason) => PathsExtraction::NonLiteral {
            reason: format!("{context}: {reason}"),
        },
    }
}

// ---------------------------------------------------------------------------
// Internals — JSON literal conversion
// ---------------------------------------------------------------------------

/// Convert an SWC expression into a JSON value, returning a short
/// description of the offending shape on failure. Only literal shapes
/// are accepted; identifiers, calls, member accesses, JSX, etc. all
/// fail with a one-liner the caller bubbles into a build warning.
fn expr_to_json(expr: &Expr) -> Result<JsonValue, String> {
    let expr = unwrap_ts_wrappers(expr);
    match expr {
        Expr::Lit(lit) => lit_to_json(lit),
        Expr::Object(obj) => object_to_json(obj),
        Expr::Array(arr) => {
            let mut out = Vec::with_capacity(arr.elems.len());
            for elem in &arr.elems {
                match elem {
                    Some(item) => {
                        if item.spread.is_some() {
                            return Err("array spread is not allowed".to_string());
                        }
                        out.push(expr_to_json(&item.expr)?);
                    }
                    None => return Err("array holes are not allowed".to_string()),
                }
            }
            Ok(JsonValue::Array(out))
        }
        Expr::Tpl(tpl) => {
            if !tpl.exprs.is_empty() {
                return Err(
                    "template strings with substitutions are not statically resolvable"
                        .to_string(),
                );
            }
            Ok(JsonValue::String(tpl_quasi_to_string(tpl)))
        }
        Expr::Unary(u) => match u.op {
            UnaryOp::Minus | UnaryOp::Plus => match &*u.arg {
                Expr::Lit(Lit::Num(n)) => {
                    let value = if matches!(u.op, UnaryOp::Minus) {
                        -n.value
                    } else {
                        n.value
                    };
                    number_to_json(value).ok_or_else(|| {
                        "non-finite number is not representable in JSON".to_string()
                    })
                }
                _ => Err("unary operator only allowed in front of a numeric literal".to_string()),
            },
            _ => Err("unary operator is not a literal value".to_string()),
        },
        other => Err(format!("{} is not a literal value", expr_kind(other))),
    }
}

fn lit_to_json(lit: &Lit) -> Result<JsonValue, String> {
    match lit {
        Lit::Str(s) => Ok(JsonValue::String(wtf8_to_string(&s.value))),
        Lit::Bool(b) => Ok(JsonValue::Bool(b.value)),
        Lit::Null(_) => Ok(JsonValue::Null),
        Lit::Num(n) => number_to_json(n.value)
            .ok_or_else(|| "non-finite number is not representable in JSON".to_string()),
        Lit::BigInt(_) => Err("BigInt literal is not representable in JSON".to_string()),
        Lit::Regex(_) => Err("regex literal is not representable in JSON".to_string()),
        Lit::JSXText(_) => Err("JSX text is not a literal value".to_string()),
    }
}

fn object_to_json(obj: &ObjectLit) -> Result<JsonValue, String> {
    let mut map = JsonMap::with_capacity(obj.props.len());
    for prop in &obj.props {
        match prop {
            PropOrSpread::Spread(_) => return Err("object spread is not allowed".to_string()),
            PropOrSpread::Prop(boxed) => match &**boxed {
                Prop::KeyValue(kv) => {
                    let key = prop_name_to_string(&kv.key)?;
                    let value = expr_to_json(&kv.value)?;
                    map.insert(key, value);
                }
                Prop::Shorthand(_) => {
                    return Err(
                        "shorthand property references a variable, not a literal".to_string(),
                    );
                }
                Prop::Method(_) => {
                    return Err("method shorthand is not a literal value".to_string());
                }
                Prop::Getter(_) => return Err("getter is not a literal value".to_string()),
                Prop::Setter(_) => return Err("setter is not a literal value".to_string()),
                Prop::Assign(_) => {
                    return Err("assignment property is not a literal value".to_string());
                }
            },
        }
    }
    Ok(JsonValue::Object(map))
}

fn prop_name_to_string(name: &PropName) -> Result<String, String> {
    match name {
        PropName::Ident(i) => Ok(i.sym.as_str().to_owned()),
        PropName::Str(s) => Ok(wtf8_to_string(&s.value)),
        PropName::Num(n) => Ok(format_number(n.value)),
        PropName::BigInt(_) => {
            Err("BigInt property keys are not representable in JSON".to_string())
        }
        PropName::Computed(_) => Err("computed property keys are not allowed".to_string()),
    }
}

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

fn wtf8_to_string(atom: &swc_core::atoms::Wtf8Atom) -> String {
    match atom.as_atom() {
        Some(a) => a.as_str().to_owned(),
        None => atom.to_string_lossy().into_owned(),
    }
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e16 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn number_to_json(value: f64) -> Option<JsonValue> {
    if value.is_finite() && value.fract() == 0.0 {
        if value >= 0.0 && value <= u64::MAX as f64 {
            return Some(JsonValue::Number(JsonNumber::from(value as u64)));
        }
        if value >= i64::MIN as f64 && value <= i64::MAX as f64 {
            return Some(JsonValue::Number(JsonNumber::from(value as i64)));
        }
    }
    JsonNumber::from_f64(value).map(JsonValue::Number)
}

/// Short human label for an expression kind, used in non-literal
/// reasons. The list is intentionally narrow — anything not enumerated
/// gets a generic "expression" so we don't have to keep up with new
/// AST variants in error messages.
fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::This(_) => "`this`",
        Expr::Array(_) => "array literal",
        Expr::Object(_) => "object literal",
        Expr::Fn(_) => "function expression",
        Expr::Unary(_) => "unary expression",
        Expr::Update(_) => "update expression",
        Expr::Bin(_) => "binary expression",
        Expr::Assign(_) => "assignment expression",
        Expr::Member(_) => "member access",
        Expr::Cond(_) => "conditional expression",
        Expr::Call(_) => "function call",
        Expr::New(_) => "`new` expression",
        Expr::Seq(_) => "sequence expression",
        Expr::Ident(_) => "identifier reference",
        Expr::Lit(_) => "literal",
        Expr::Tpl(_) => "template literal",
        Expr::TaggedTpl(_) => "tagged template literal",
        Expr::Arrow(_) => "arrow function",
        Expr::Class(_) => "class expression",
        Expr::Yield(_) => "`yield` expression",
        Expr::MetaProp(_) => "meta-property",
        Expr::Await(_) => "`await` expression",
        Expr::Paren(_) => "parenthesized expression",
        Expr::JSXElement(_) | Expr::JSXFragment(_) | Expr::JSXEmpty(_) | Expr::JSXMember(_)
        | Expr::JSXNamespacedName(_) => "JSX expression",
        _ => "expression",
    }
}

fn stmt_kind(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::Block(_) => "block statement",
        Stmt::Empty(_) => "empty statement",
        Stmt::Debugger(_) => "`debugger` statement",
        Stmt::With(_) => "`with` statement",
        Stmt::Return(_) => "return statement",
        Stmt::Labeled(_) => "labeled statement",
        Stmt::Break(_) => "`break` statement",
        Stmt::Continue(_) => "`continue` statement",
        Stmt::If(_) => "`if` statement",
        Stmt::Switch(_) => "`switch` statement",
        Stmt::Throw(_) => "`throw` statement",
        Stmt::Try(_) => "`try` statement",
        Stmt::While(_) => "`while` loop",
        Stmt::DoWhile(_) => "`do-while` loop",
        Stmt::For(_) => "`for` loop",
        Stmt::ForIn(_) => "`for-in` loop",
        Stmt::ForOf(_) => "`for-of` loop",
        Stmt::Decl(_) => "declaration",
        Stmt::Expr(_) => "expression statement",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn extract_ok(src: &str) -> PathsExtraction {
        extract_paths(src, "page.tsx").expect("extract should succeed")
    }

    #[test]
    fn function_with_literal_array_extracted() {
        let src = r#"
            export function paths() {
                return [
                    { params: { slug: "hello" } },
                    { params: { slug: "world" }, props: { i: 2 } },
                ];
            }
        "#;
        let out = extract_ok(src);
        match out {
            PathsExtraction::Literal(v) => {
                assert_eq!(
                    v,
                    json!([
                        { "params": { "slug": "hello" } },
                        { "params": { "slug": "world" }, "props": { "i": 2 } },
                    ]),
                );
            }
            other => unreachable!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn async_function_with_literal_array_extracted() {
        let src = r#"
            export async function paths() {
                return [{ params: { slug: "a" } }];
            }
        "#;
        match extract_ok(src) {
            PathsExtraction::Literal(v) => {
                assert_eq!(v, json!([{ "params": { "slug": "a" } }]));
            }
            other => unreachable!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn arrow_with_literal_body_extracted() {
        let src = r#"
            export const paths = () => [{ params: { slug: "x" } }];
        "#;
        match extract_ok(src) {
            PathsExtraction::Literal(v) => {
                assert_eq!(v, json!([{ "params": { "slug": "x" } }]));
            }
            other => unreachable!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn arrow_with_block_body_extracted() {
        let src = r#"
            export const paths = () => {
                return [{ params: { slug: "y" } }];
            };
        "#;
        match extract_ok(src) {
            PathsExtraction::Literal(v) => {
                assert_eq!(v, json!([{ "params": { "slug": "y" } }]));
            }
            other => unreachable!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn const_function_expression_extracted() {
        let src = r#"
            export const paths = function () {
                return [{ params: { slug: "z" } }];
            };
        "#;
        match extract_ok(src) {
            PathsExtraction::Literal(v) => {
                assert_eq!(v, json!([{ "params": { "slug": "z" } }]));
            }
            other => unreachable!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn ts_wrappers_around_initializer_transparent() {
        let src = r#"
            export const paths = (() => [{ params: { slug: "x" } }]) as const;
        "#;
        match extract_ok(src) {
            PathsExtraction::Literal(v) => {
                assert_eq!(v, json!([{ "params": { "slug": "x" } }]));
            }
            other => unreachable!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn empty_array_literal_is_literal_extraction() {
        let src = r#"
            export function paths() { return []; }
        "#;
        match extract_ok(src) {
            PathsExtraction::Literal(v) => assert_eq!(v, json!([])),
            other => unreachable!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn missing_export_reported_as_missing() {
        let src = r#"
            export default function Page() { return null; }
        "#;
        assert!(matches!(extract_ok(src), PathsExtraction::Missing));
    }

    #[test]
    fn await_import_reported_as_non_literal() {
        // Mirrors the real basic-blog usage. We can't statically follow
        // `await import(...)`; this should report NonLiteral so the
        // caller defers to runtime evaluation.
        let src = r#"
            export async function paths() {
                const { getCollection } = await import("zfb/content");
                const posts = await getCollection("blog");
                return posts.map((post) => ({ params: { slug: post.slug } }));
            }
        "#;
        match extract_ok(src) {
            PathsExtraction::NonLiteral { reason } => {
                assert!(
                    !reason.is_empty(),
                    "non-literal reason should not be empty",
                );
            }
            other => unreachable!("expected NonLiteral, got {other:?}"),
        }
    }

    #[test]
    fn function_call_in_return_reported_as_non_literal() {
        let src = r#"
            function helper() { return [{ params: { slug: "x" } }]; }
            export function paths() { return helper(); }
        "#;
        match extract_ok(src) {
            PathsExtraction::NonLiteral { reason } => {
                assert!(reason.contains("function call"), "{reason}");
            }
            other => unreachable!("expected NonLiteral, got {other:?}"),
        }
    }

    #[test]
    fn identifier_in_return_reported_as_non_literal() {
        let src = r#"
            const data = [{ params: { slug: "x" } }];
            export function paths() { return data; }
        "#;
        match extract_ok(src) {
            PathsExtraction::NonLiteral { reason } => {
                assert!(reason.contains("identifier"), "{reason}");
            }
            other => unreachable!("expected NonLiteral, got {other:?}"),
        }
    }

    #[test]
    fn non_function_initializer_reported_as_non_literal() {
        // `paths` is the wrong shape entirely — array, not a function.
        // We could pretend to support this (it's literally the JSON
        // we want), but the type contract on the page side is that
        // `paths` is callable. Reject for clarity.
        let src = r#"
            export const paths = [{ params: { slug: "x" } }];
        "#;
        match extract_ok(src) {
            PathsExtraction::NonLiteral { reason } => {
                assert!(reason.contains("must be a function"), "{reason}");
            }
            other => unreachable!("expected NonLiteral, got {other:?}"),
        }
    }

    #[test]
    fn multiple_returns_reported_as_non_literal() {
        let src = r#"
            export function paths() {
                if (typeof globalThis !== "undefined") {
                    return [{ params: { slug: "a" } }];
                }
                return [{ params: { slug: "b" } }];
            }
        "#;
        // The `if` is a non-return statement — extraction bails before
        // it gets to count returns. Either way: NonLiteral.
        assert!(matches!(extract_ok(src), PathsExtraction::NonLiteral { .. }));
    }

    #[test]
    fn empty_body_reported_as_non_literal() {
        let src = r#"
            export function paths() {}
        "#;
        match extract_ok(src) {
            PathsExtraction::NonLiteral { reason } => {
                assert!(reason.contains("no return"), "{reason}");
            }
            other => unreachable!("expected NonLiteral, got {other:?}"),
        }
    }

    #[test]
    fn return_without_argument_reported_as_non_literal() {
        let src = r#"
            export function paths() { return; }
        "#;
        match extract_ok(src) {
            PathsExtraction::NonLiteral { reason } => {
                assert!(reason.contains("returns nothing"), "{reason}");
            }
            other => unreachable!("expected NonLiteral, got {other:?}"),
        }
    }

    #[test]
    fn malformed_source_returns_parse_error_not_panic() {
        // Unterminated string literal — guaranteed to bork the parser.
        let src = "export function paths() { return [\"unterminated;\n";
        let err = extract_paths(src, "broken.tsx").expect_err("must fail");
        assert!(matches!(err, PathsExtractError::Parse { .. }));
    }

    #[test]
    fn literal_with_template_substitution_reported_as_non_literal() {
        let src = r#"
            const NAME = "x";
            export function paths() {
                return [{ params: { slug: `prefix-${NAME}` } }];
            }
        "#;
        match extract_ok(src) {
            PathsExtraction::NonLiteral { reason } => {
                assert!(reason.contains("substitution"), "{reason}");
            }
            other => unreachable!("expected NonLiteral, got {other:?}"),
        }
    }

    #[test]
    fn literal_with_nested_arrays_extracted() {
        let src = r#"
            export function paths() {
                return [
                    { params: { slug: ["a", "b", "c"] } },
                ];
            }
        "#;
        match extract_ok(src) {
            PathsExtraction::Literal(v) => {
                assert_eq!(
                    v,
                    json!([{ "params": { "slug": ["a", "b", "c"] } }]),
                );
            }
            other => unreachable!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn unrelated_exports_dont_confuse_extractor() {
        let src = r#"
            export const helper = (x: number) => x + 1;
            export const frontmatter = { title: "X" };
            export function paths() {
                return [{ params: { slug: "ok" } }];
            }
            export default function Page() { return null; }
        "#;
        match extract_ok(src) {
            PathsExtraction::Literal(v) => {
                assert_eq!(v, json!([{ "params": { "slug": "ok" } }]));
            }
            other => unreachable!("expected Literal, got {other:?}"),
        }
    }
}
