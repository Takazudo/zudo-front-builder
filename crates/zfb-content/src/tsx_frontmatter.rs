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
//!
//! ### The default export's first parameter (#2352)
//!
//! The same walk also records the *shape* of the default export's first
//! parameter as a [`DefaultExportFirstParam`]. zfb calls a page's
//! default export with the page's **props object** — never with a
//! `Request` — so a `prerender = false` route written as
//! `export default async function Handler(request: Request)` silently
//! 405s forever while `tsc` stays happy. Capturing the parameter shape
//! here (rather than in a second parse) lets the engine surface that
//! mistake; the gate itself is [`ssr_request_param_tier`].

use std::path::PathBuf;

use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap, Span, Spanned};
use swc_core::ecma::ast::{
    BlockStmt, BlockStmtOrExpr, Decl, DefaultDecl, EsVersion, ExportSpecifier, Expr,
    ImportSpecifier, Lit, MemberExpr, MemberProp, Module, ModuleDecl, ModuleExportName, ModuleItem,
    ObjectLit, Pat, Prop, PropName, PropOrSpread, Stmt, TsEntityName, TsType, TsTypeAnn,
    TsTypeParamDecl, UnaryOp, VarDeclKind,
};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::visit::{Visit, VisitWith};
use thiserror::Error;

/// Output of [`extract`]. A successful extraction always produces a
/// `frontmatter` (the required export); the two sibling string exports
/// are optional and absent when the source did not declare them. The
/// `prerender` flag always carries a value because it has a sensible
/// default (`true` = SSG).
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
    /// Value of the optional `export const prerender = …` boolean
    /// literal.
    ///
    /// Default: `true` (the page is rendered at build time / SSG).
    ///
    /// Only literal boolean expressions count — `true` or `false` after
    /// stripping TypeScript wrappers (`as const`, `satisfies`, etc.).
    /// Anything that requires runtime evaluation (function calls,
    /// ternaries, identifier references, member access, …) falls back
    /// to the default `true`; the extractor does **not** error in that
    /// case, and does **not** silently coerce it to `false`. A missing
    /// `export const prerender` likewise yields `true`.
    ///
    /// The build orchestrator consumes this field to decide which
    /// pages are emitted at build time (SSG, `prerender == true`) vs
    /// added to a runtime SSR manifest (`prerender == false`).
    pub prerender: bool,
    /// Shape of the default export's first parameter (#2352). Joined
    /// with `prerender` by [`ssr_request_param_tier`] to detect the
    /// silent `(request: Request)` SSR handler mistake.
    pub default_export_param: DefaultExportFirstParam,
}

/// What the module walk could see of the default export's **first**
/// parameter. Deliberately coarse: this is a lint input, not a type
/// checker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultExportFirstParam {
    /// No default export at all, a type-only one
    /// (`export default interface …`), or a default-export function
    /// literal declaring zero parameters — the correct shape for an
    /// API route.
    Absent,
    /// The first parameter is an object or array destructuring pattern
    /// (`{ params }`, `[a, b]`) — the ordinary page-props shape.
    Destructured,
    /// The first parameter is a plain binding identifier.
    Plain(PlainFirstParam),
    /// A default export this walk cannot see through: an identifier
    /// reference (`export default handler`), a call
    /// (`export default wrap(handler)`), a class, a `default`-named
    /// re-export (`export { handler as default }`), or any other
    /// non-function-literal expression. Never fires the gate — see the
    /// epic's documented misses.
    Opaque,
}

/// A plain (non-destructured) first parameter of the default export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainFirstParam {
    /// The binding's name, after unwrapping a default value
    /// (`Pat::Assign`) or a rest element (`Pat::Rest`).
    pub name: String,
    /// `true` only when the TS annotation is the global `Request` —
    /// a `TsTypeRef` whose entity name is the bare ident `Request` or
    /// the qualified `globalThis.Request`, carrying no type arguments.
    /// A local alias (`type Req = Request`), an unrelated `MyRequest`,
    /// and a generic `Request<T>` all leave this `false`; resolving
    /// them would need a type checker, and guessing from the spelling
    /// is exactly the substring match the spec forbids.
    pub annotation_is_request: bool,
    /// `true` when the function body reads a `Request`-only member off
    /// this parameter (`.method`, `.json()`, … — see
    /// `REQUEST_ONLY_MEMBERS`). Behavioural evidence, independent of the
    /// type annotation, which is what keeps the gate enforceable for
    /// `.js` / `.jsx` routes — they can carry `prerender = false` but no
    /// annotation, so the strong tier would otherwise be unreachable for
    /// them (#2361). Only consulted for a parameter already named
    /// `request` / `req`; on its own it is NOT enough to fire the gate,
    /// because a props object from `getStaticProps` may legitimately
    /// carry a field like `url`.
    pub body_uses_request_members: bool,
    /// 1-based location of the parameter's identifier, in this file's
    /// existing `line:column` convention (see [`Ctx::line`]).
    pub line: usize,
    /// 1-based column; see `line`.
    pub col: usize,
}

/// How confident the detector is that a plain first parameter is the
/// mistaken `Request` shape.
///
/// The tiers differ in downstream **wording** everywhere, and since #2361
/// also in **severity**: `zfb check` fails on `Strong` and only warns on
/// `Heuristic` (`zfb dev` / `zfb build` warn on both). That split exists
/// because there is no suppression mechanism — hard-failing a project
/// whose props parameter is merely *named* `request` would trap it with
/// no way out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestParamTier {
    /// Near-conclusive. Either the parameter is annotated with the global
    /// `Request` (and nothing local shadows that name), or it is named
    /// `request` / `req` **and** the body reads a `Request`-only member.
    /// The second half is what keeps `.js` / `.jsx` routes enforceable —
    /// they cannot carry an annotation at all.
    Strong,
    /// The parameter is *only* named `request` / `req`. A props object can
    /// legitimately carry that name, so the wording hedges and `zfb check`
    /// warns rather than failing.
    Heuristic,
}

/// Parameter names that raise the heuristic tier on their own. Exact,
/// lowercase matches only.
const HEURISTIC_PARAM_NAMES: [&str; 2] = ["request", "req"];

impl DefaultExportFirstParam {
    /// Conditions 2-4 of the epic's gate: a function-literal default
    /// export whose first parameter is a plain binding identifier that
    /// is either annotated `Request` (strong) or named `request` /
    /// `req` (heuristic).
    ///
    /// Condition 1 (the route's resolved `prerender == false`) is the
    /// caller's to supply — use [`ssr_request_param_tier`] to join it.
    pub fn request_param_tier(&self) -> Option<RequestParamTier> {
        let plain = match self {
            DefaultExportFirstParam::Plain(p) => p,
            _ => return None,
        };
        if plain.annotation_is_request {
            return Some(RequestParamTier::Strong);
        }
        if HEURISTIC_PARAM_NAMES.contains(&plain.name.as_str()) {
            // Behavioural evidence promotes the naming heuristic to strong:
            // a parameter named `request` whose body reads `.method` /
            // `.json()` / … is near-conclusively the #2350 mistake. This is
            // load-bearing for `.js` / `.jsx` routes, which cannot carry a
            // `Request` annotation at all and would otherwise never reach
            // the strong tier — leaving a tier-gated `zfb check` unable to
            // enforce the contract for them (#2361).
            //
            // The name gate is what makes this safe: body evidence alone is
            // NOT a path to strong, because a props object legitimately may
            // carry a field like `url`.
            if plain.body_uses_request_members {
                return Some(RequestParamTier::Strong);
            }
            return Some(RequestParamTier::Heuristic);
        }
        None
    }
}

/// The epic's full gate: a `prerender = false` route whose default
/// export takes the mistaken `Request` first parameter. `Some(tier)`
/// means "report this page"; the tier picks the wording.
///
/// Every consumer (`zfb dev`, `zfb build`, `zfb check`) must call this
/// rather than re-deriving the rule — one gate, three surfaces.
pub fn ssr_request_param_tier(
    prerender: bool,
    param: &DefaultExportFirstParam,
) -> Option<RequestParamTier> {
    if prerender {
        return None;
    }
    param.request_param_tier()
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
    ///
    /// `prerender` carries the value the loop resolved *before* this
    /// error was raised — a lone `export const prerender = <bool>` with
    /// no `frontmatter` sibling is still parsed (default `true`). The
    /// consumer ([`crate::extract_tsx_frontmatter`] caller
    /// `build_prerender_map`) reads it so the `output: static` gate can
    /// reject a frontmatter-less `prerender = false` page instead of
    /// silently shipping it as SSG (#1198).
    ///
    /// `default_export_param` rides along for the same reason (#2352):
    /// an API route commonly declares `export const prerender = false`
    /// and no `frontmatter` at all, so the handler-shape detector would
    /// be blind to the single most common shape it exists to catch if
    /// the verdict were dropped on this path.
    #[error("{file}: missing required `export const frontmatter`")]
    MissingFrontmatter {
        file: String,
        prerender: bool,
        default_export_param: DefaultExportFirstParam,
    },

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
    #[error(
        "{file}:{line}:{col}: non-literal value not allowed in `export const {export}` ({reason})"
    )]
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
        // Pre-pass, deliberately before the classification walk below —
        // see the field's doc comment for why order-independence matters.
        request_shadowed: request_is_shadowed(&module),
    };

    let mut frontmatter: Option<JsonValue> = None;
    let mut extension: Option<String> = None;
    let mut content_type: Option<String> = None;
    // `prerender` defaults to `true` (SSG). Only a top-level
    // `export const prerender = <bool literal>` flips it. Computed
    // values (calls, ternaries, identifiers, …) are treated as "not
    // specified" and leave the default in place — they do NOT error.
    let mut prerender = true;
    // Track "have we already seen this export?" separately from the
    // value slot. We can't rely on `frontmatter.is_some()` because a
    // failed extraction (e.g. duplicate after a good one) must still
    // report against the *second* declaration; flipping these flags
    // before parsing the new initializer keeps that ordering honest.
    let mut frontmatter_seen = false;
    let mut extension_seen = false;
    let mut content_type_seen = false;
    // First-parameter shape of the default export (#2352). `None` = the
    // walk has not seen a default export yet; the first one wins (two
    // default exports are a TS error anyway, and first-wins keeps the
    // verdict deterministic).
    let mut default_export_param: Option<DefaultExportFirstParam> = None;

    for item in module.body.iter() {
        // The default-export shape rides on THIS walk — the detector
        // must not cost a second SWC parse (#2351).
        if let ModuleItem::ModuleDecl(decl) = item {
            if default_export_param.is_none() {
                default_export_param = default_export_first_param(decl, &ctx);
            }
        }

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

            // `prerender` is handled out-of-band from the strict
            // `ExportTarget` machinery: it has a default, never errors
            // on a non-literal initializer, and is therefore not part
            // of the duplicate-detection / wrong-shape error paths
            // that the other three exports share.
            if name_str == "prerender" {
                if let Some(init) = &declarator.init {
                    if let Expr::Lit(Lit::Bool(b)) = unwrap_ts_wrappers(init) {
                        // Last literal-bool declaration wins. Computed
                        // initializers leave the prior value (or the
                        // default `true`) untouched.
                        prerender = b.value;
                    }
                }
                continue;
            }

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

    let default_export_param = default_export_param.unwrap_or(DefaultExportFirstParam::Absent);

    let frontmatter = frontmatter.ok_or_else(|| TsxFrontmatterError::MissingFrontmatter {
        file: file_name.to_string(),
        // Surface the loop-resolved `prerender` instead of discarding it:
        // a lone `export const prerender = false` (no `frontmatter`) must
        // still reach the `output: static` gate (#1198).
        prerender,
        default_export_param: default_export_param.clone(),
    })?;

    Ok(TsxFrontmatter {
        frontmatter,
        extension,
        content_type,
        prerender,
        default_export_param,
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
    /// Does this module declare or import a top-level binding named
    /// `Request`, shadowing the global Fetch type? Computed by
    /// [`request_is_shadowed`] in a pre-pass, because a `type` /
    /// `interface` / `class` declaration may appear AFTER the default
    /// export — folding it into the classification walk would make the
    /// verdict depend on declaration order (#2361).
    request_shadowed: bool,
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

/// Classify a module declaration's default export, if it is one.
/// `None` means "not a default export" — the caller keeps looking.
fn default_export_first_param(decl: &ModuleDecl, ctx: &Ctx<'_>) -> Option<DefaultExportFirstParam> {
    match decl {
        // `export default function Page(…) {}` / `export default class …`
        ModuleDecl::ExportDefaultDecl(d) => Some(match &d.decl {
            DefaultDecl::Fn(f) => first_param_shape(
                f.function.params.iter().map(|p| &p.pat),
                FnBody::Block(f.function.body.as_ref()),
                // A function's own `<Request>` type parameter shadows the
                // global just as a module-scope declaration does.
                ctx.request_shadowed
                    || type_params_shadow_request(f.function.type_params.as_deref()),
                ctx,
            ),
            // A class is not a function literal — condition 2 fails.
            DefaultDecl::Class(_) => DefaultExportFirstParam::Opaque,
            // Type-only: there is no runtime default export to call.
            DefaultDecl::TsInterfaceDecl(_) => DefaultExportFirstParam::Absent,
        }),
        // `export default (…) => {}`, `export default handler`, …
        ModuleDecl::ExportDefaultExpr(e) => Some(match unwrap_ts_wrappers(&e.expr) {
            Expr::Arrow(a) => first_param_shape(
                a.params.iter(),
                FnBody::Arrow(&a.body),
                ctx.request_shadowed || type_params_shadow_request(a.type_params.as_deref()),
                ctx,
            ),
            Expr::Fn(f) => first_param_shape(
                f.function.params.iter().map(|p| &p.pat),
                FnBody::Block(f.function.body.as_ref()),
                ctx.request_shadowed
                    || type_params_shadow_request(f.function.type_params.as_deref()),
                ctx,
            ),
            // Identifier references, calls, classes, and everything else
            // are the epic's documented misses — seeing through them
            // would mean resolving bindings across modules.
            _ => DefaultExportFirstParam::Opaque,
        }),
        // `export { handler as default }` / `export { default } from "…"`
        // — a real default export whose value lives elsewhere.
        ModuleDecl::ExportNamed(named) => named
            .specifiers
            .iter()
            .any(export_specifier_names_default)
            .then_some(DefaultExportFirstParam::Opaque),
        _ => None,
    }
}

/// Does this export specifier introduce a `default` export? Covers both
/// `export { handler as default }` (aliased) and
/// `export { default } from "./handler"` (re-exported as-is).
fn export_specifier_names_default(spec: &ExportSpecifier) -> bool {
    let named = match spec {
        ExportSpecifier::Named(n) => n,
        // `export * as default from "…"` — also a default export.
        ExportSpecifier::Namespace(ns) => return module_export_name_is_default(&ns.name),
        ExportSpecifier::Default(_) => return true,
    };
    if named.is_type_only {
        return false;
    }
    // The *exported* name is what matters; it falls back to `orig` when
    // there is no `as` clause (`export { default } from "…"`).
    let exported = named.exported.as_ref().unwrap_or(&named.orig);
    module_export_name_is_default(exported)
}

fn module_export_name_is_default(name: &ModuleExportName) -> bool {
    match name {
        ModuleExportName::Ident(i) => i.sym.as_ref() == "default",
        ModuleExportName::Str(s) => wtf8_to_string(&s.value) == "default",
    }
}

/// Classify the FIRST parameter of a function-literal default export.
/// Zero parameters is [`DefaultExportFirstParam::Absent`] — the correct
/// API-handler shape.
fn first_param_shape<'a, I>(
    mut params: I,
    body: FnBody<'_>,
    shadowed: bool,
    ctx: &Ctx<'_>,
) -> DefaultExportFirstParam
where
    I: Iterator<Item = &'a Pat>,
{
    // TypeScript's `this` pseudo-parameter is erased at compile time and
    // receives no argument, so the props object still arrives in the
    // slot after it. Classifying it as the first parameter would report
    // `function Page(this: Request, props: Props)` — a correct page — at
    // the strong tier.
    match params.find(|pat| !is_this_pseudo_param(pat)) {
        Some(pat) => classify_param_pat(pat, body, shadowed, ctx),
        None => DefaultExportFirstParam::Absent,
    }
}

fn is_this_pseudo_param(pat: &Pat) -> bool {
    matches!(pat, Pat::Ident(bi) if bi.id.sym.as_ref() == "this")
}

/// The default export's body, for the behavioural-evidence check. Two
/// shapes because an arrow may have an expression body
/// (`(request) => new Response(request.method)`) rather than a block.
#[derive(Copy, Clone)]
enum FnBody<'a> {
    Block(Option<&'a BlockStmt>),
    Arrow(&'a BlockStmtOrExpr),
}

impl FnBody<'_> {
    fn reads_request_members(&self, param_name: &str) -> bool {
        match self {
            FnBody::Block(Some(b)) => visit_reads_request_members(*b, param_name),
            FnBody::Block(None) => false,
            FnBody::Arrow(b) => visit_reads_request_members(*b, param_name),
        }
    }
}

/// Unwrap a default value (`Pat::Assign`) and a rest element
/// (`Pat::Rest`) — condition 3 — then classify what is underneath.
fn classify_param_pat(
    pat: &Pat,
    body: FnBody<'_>,
    shadowed: bool,
    ctx: &Ctx<'_>,
) -> DefaultExportFirstParam {
    let mut cur = pat;
    loop {
        cur = match cur {
            Pat::Ident(bi) => {
                let (line, col) = ctx.line(bi.id.span);
                let name = bi.id.sym.as_ref().to_owned();
                return DefaultExportFirstParam::Plain(PlainFirstParam {
                    annotation_is_request: bi
                        .type_ann
                        .as_deref()
                        .is_some_and(|ann| type_ann_is_global_request(ann, shadowed)),
                    body_uses_request_members: body.reads_request_members(&name),
                    name,
                    line,
                    col,
                });
            }
            Pat::Object(_) | Pat::Array(_) => return DefaultExportFirstParam::Destructured,
            Pat::Assign(a) => &a.left,
            Pat::Rest(r) => &r.arg,
            // `Pat::Expr` is for-in/for-of only and `Pat::Invalid` comes
            // from a recovered parse; neither is a shape to lint on.
            Pat::Expr(_) | Pat::Invalid(_) => return DefaultExportFirstParam::Opaque,
        };
    }
}

/// Does this module introduce a top-level binding named `Request`,
/// shadowing the global Fetch type?
///
/// When it does, a bare `Request` annotation is NOT evidence of the
/// `(request: Request)` mistake — it refers to the local type, which may
/// legitimately be the page's props shape (#2361). The qualified
/// `globalThis.Request` is unaffected and stays strong-tier even here;
/// that is the whole point of writing it qualified.
///
/// Counts as shadowing (all introduce a module-scope `Request` binding):
/// an import specifier — value or `import type`, plain or `as Request`;
/// and a top-level `type` / `interface` / `class` / `enum` declaration,
/// whether or not it is exported.
///
/// Deliberately does NOT count:
/// - `declare global { interface Request { … } }` — that *augments* the
///   global rather than shadowing it, so a bare `Request` still means the
///   Fetch type.
/// - `export { Request } from "./types"` / `export type { Request } from …`
///   — a re-export creates no local binding.
/// - a binding inside the handler's own body — it cannot affect what the
///   parameter's annotation resolves to.
/// - a namespace import (`import * as X`), because `X.Request` is already
///   excluded from the strong tier by [`entity_name_is_global_request`].
///
/// A function's own type parameter (`function Page<Request>(request: Request)`)
/// also shadows, but is per-function rather than module-scope — it is
/// handled at the classification site by [`type_params_shadow_request`].
fn request_is_shadowed(module: &Module) -> bool {
    module.body.iter().any(|item| match item {
        ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
            import.specifiers.iter().any(import_specifier_binds_request)
        }
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => decl_binds_request(&e.decl),
        ModuleItem::Stmt(Stmt::Decl(decl)) => decl_binds_request(decl),
        _ => false,
    })
}

/// Does this import specifier bind the local name `Request`? The LOCAL
/// name is what shadows, so `import { Foo as Request }` counts and
/// `import { Request as Foo }` does not. A namespace import binds `X`,
/// not `Request`, so it never counts (see [`request_is_shadowed`]).
fn import_specifier_binds_request(spec: &ImportSpecifier) -> bool {
    match spec {
        ImportSpecifier::Named(n) => n.local.sym.as_ref() == "Request",
        ImportSpecifier::Default(d) => d.local.sym.as_ref() == "Request",
        ImportSpecifier::Namespace(_) => false,
    }
}

/// Does this declaration introduce a type named `Request`? Only the
/// type-position kinds matter — a `const`/`let`/`fn` named `Request` is a
/// value binding and cannot be what a type annotation resolves to.
fn decl_binds_request(decl: &Decl) -> bool {
    match decl {
        Decl::TsTypeAlias(a) => a.id.sym.as_ref() == "Request",
        Decl::TsInterface(i) => i.id.sym.as_ref() == "Request",
        Decl::TsEnum(e) => e.id.sym.as_ref() == "Request",
        Decl::Class(c) => c.ident.sym.as_ref() == "Request",
        _ => false,
    }
}

/// Does this function's own type-parameter list shadow `Request`?
/// `export default function Page<Request>(request: Request)` annotates
/// the parameter with the type variable, not the Fetch type.
fn type_params_shadow_request(type_params: Option<&TsTypeParamDecl>) -> bool {
    type_params.is_some_and(|tp: &TsTypeParamDecl| {
        tp.params.iter().any(|p| p.name.sym.as_ref() == "Request")
    })
}

/// Members that only a `Request` carries — reading one of these off the
/// first parameter is near-conclusive that the author believes they were
/// handed the incoming request.
///
/// Deliberately excludes plausible props-object field names (`body`,
/// `params`, `title`, …). `url` and `headers` are borderline but stay in:
/// this list is only ever consulted for a parameter ALREADY named
/// `request` / `req`, so a props object would have to be named `request`
/// *and* carry `.url` to trip it.
const REQUEST_ONLY_MEMBERS: [&str; 10] = [
    "method",
    "headers",
    "url",
    "json",
    "text",
    "formData",
    "arrayBuffer",
    "blob",
    "bodyUsed",
    "clone",
];

/// Does `node` read a `Request`-only member off `param_name`?
///
/// This is what keeps the gate enforceable for `.js` / `.jsx` routes.
/// Those are routable script pages (`zfb_types::SCRIPT_PAGE_EXTENSIONS`)
/// that can carry `prerender = false`, but they cannot carry a TS
/// annotation at all — so the strong tier would be unreachable for them
/// and a tier-gated `zfb check` would never enforce the contract there
/// (#2361). Behavioural evidence restores it: `request.method` in a plain
/// JS handler is exactly the bug #2350 reported.
fn visit_reads_request_members<N>(node: &N, param_name: &str) -> bool
where
    N: VisitWith<MemberFinder>,
{
    let mut finder = MemberFinder {
        param: param_name.to_owned(),
        found: false,
        rebound: false,
    };
    node.visit_with(&mut finder);
    finder.evidence()
}

/// Owns its `param` as a `String` so the visitor carries no lifetime,
/// which keeps [`visit_reads_request_members`]'s `VisitWith` bound simple
/// enough to accept both a `BlockStmt` and a `BlockStmtOrExpr`.
struct MemberFinder {
    param: String,
    found: bool,
    /// Set when anything inside the body binds the same name — a nested
    /// function/arrow parameter, a `let`/`const`/`var`, a `catch` binding,
    /// a destructuring alias, or a nested `function`/`class` declaration.
    ///
    /// Such a binding is a DIFFERENT variable, so its member reads say
    /// nothing about the outer parameter. Rather than doing real scope
    /// analysis, any rebinding disqualifies body evidence wholesale — the
    /// asymmetry justifies it: a miss here just falls back to the heuristic
    /// tier (a warning), while a false positive fails `zfb check` and
    /// breaks a correct build.
    rebound: bool,
}

impl MemberFinder {
    /// Body evidence counts only when a member was read AND nothing
    /// rebound the name anywhere inside the body.
    fn evidence(&self) -> bool {
        self.found && !self.rebound
    }
}

impl Visit for MemberFinder {
    /// Every binding position that produces a `BindingIdent`: function and
    /// arrow parameters, variable declarators, `catch` params, and
    /// destructuring aliases. Member-expression properties are `IdentName`
    /// and plain reads are `Ident`, so neither reaches this hook.
    fn visit_binding_ident(&mut self, node: &swc_core::ecma::ast::BindingIdent) {
        if node.id.sym.as_ref() == self.param {
            self.rebound = true;
        }
        node.visit_children_with(self);
    }

    /// `function request() {}` — a declaration name is an `Ident`, not a
    /// `BindingIdent`, so it needs its own hook.
    fn visit_fn_decl(&mut self, node: &swc_core::ecma::ast::FnDecl) {
        if node.ident.sym.as_ref() == self.param {
            self.rebound = true;
        }
        node.visit_children_with(self);
    }

    /// `class request {}` — same reason as `visit_fn_decl`.
    fn visit_class_decl(&mut self, node: &swc_core::ecma::ast::ClassDecl) {
        if node.ident.sym.as_ref() == self.param {
            self.rebound = true;
        }
        node.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, node: &MemberExpr) {
        // `<param>.<member>` — the object must be the parameter itself, so
        // an unrelated `ctx.request.method` does not count.
        if let Expr::Ident(obj) = &*node.obj {
            if obj.sym.as_ref() == self.param {
                let member: Option<String> = match &node.prop {
                    MemberProp::Ident(i) => Some(i.sym.as_ref().to_owned()),
                    // `request["method"]` — same read, bracket spelling.
                    MemberProp::Computed(c) => match &*c.expr {
                        Expr::Lit(Lit::Str(s)) => Some(wtf8_to_string(&s.value)),
                        _ => None,
                    },
                    MemberProp::PrivateName(_) => None,
                };
                if member.is_some_and(|m| REQUEST_ONLY_MEMBERS.contains(&m.as_str())) {
                    self.found = true;
                }
            }
        }
        node.visit_children_with(self);
    }
}

/// Is this annotation the global `Request`?
///
/// Matched against the type AST — a `TsTypeRef` with no type arguments
/// whose entity name is the bare ident `Request` or the qualified
/// `globalThis.Request`. Rendering the type to a string and
/// substring-matching would accept `MyRequest` and `Request<T>`, which
/// is precisely what the strong tier must not do (#2352).
/// `shadowed` is true when a module-scope or function-scope binding named
/// `Request` exists — see [`request_is_shadowed`]. It suppresses only the
/// BARE `Request` spelling; `globalThis.Request` is explicitly qualified
/// and stays strong-tier regardless (#2361).
fn type_ann_is_global_request(ann: &TsTypeAnn, shadowed: bool) -> bool {
    type_is_global_request(&ann.type_ann, shadowed)
}

fn type_is_global_request(ty: &TsType, shadowed: bool) -> bool {
    match ty {
        TsType::TsParenthesizedType(p) => type_is_global_request(&p.type_ann, shadowed),
        TsType::TsTypeRef(r) => {
            // `Request<T>` carries type arguments — a different type.
            r.type_params.is_none() && entity_name_is_global_request(&r.type_name, shadowed)
        }
        _ => false,
    }
}

fn entity_name_is_global_request(name: &TsEntityName, shadowed: bool) -> bool {
    match name {
        // A bare `Request` only means the Fetch type when nothing local
        // shadows it. The qualified arm below is deliberately NOT gated on
        // `shadowed` — `globalThis.Request` cannot be shadowed.
        TsEntityName::Ident(i) => !shadowed && i.sym.as_ref() == "Request",
        TsEntityName::TsQualifiedName(q) => {
            // Exactly `globalThis.Request`; any deeper qualification
            // (`foo.globalThis.Request`, `ns.Request`) is some other type.
            matches!(&q.left, TsEntityName::Ident(l) if l.sym.as_ref() == "globalThis")
                && q.right.sym.as_ref() == "Request"
        }
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
fn expect_string(expr: &Expr, export: &str, ctx: &Ctx<'_>) -> Result<String, TsxFrontmatterError> {
    match unwrap_ts_wrappers(expr) {
        Expr::Lit(Lit::Str(s)) => Ok(wtf8_to_string(&s.value)),
        Expr::Tpl(tpl) if tpl.exprs.is_empty() => tpl_quasi_to_string(tpl)
            .ok_or_else(|| ctx.computed(tpl.span, export, "empty template literal has no quasi")),
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
                    // On duplicate keys, last declaration wins, matching
                    // JS object-literal evaluation order. `serde_json::Map`
                    // preserves insertion order, so the final value is
                    // deterministic.
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
        PropName::Computed(c) => {
            Err(ctx.computed(c.span, export, "computed property keys are not allowed"))
        }
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
                        return Err(ctx.computed(arr.span, export, "array holes are not allowed"));
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
            let s = tpl_quasi_to_string(tpl).ok_or_else(|| {
                ctx.computed(tpl.span, export, "empty template literal has no quasi")
            })?;
            Ok(JsonValue::String(s))
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
            _ => Err(ctx.computed(u.span, export, "unary operator is not a literal value")),
        },
        // Everything below requires runtime evaluation or is otherwise
        // not a literal; report with a kind-specific reason so the
        // page author sees what they used.
        Expr::Ident(i) => {
            Err(ctx.computed(i.span, export, "identifier reference is not a literal"))
        }
        Expr::Call(c) => Err(ctx.computed(c.span, export, "function call is not a literal")),
        Expr::New(n) => Err(ctx.computed(n.span, export, "`new` expression is not a literal")),
        Expr::Member(m) => Err(ctx.computed(m.span, export, "member access is not a literal")),
        Expr::Bin(b) => Err(ctx.computed(b.span, export, "binary expression is not a literal")),
        Expr::Cond(c) => {
            Err(ctx.computed(c.span, export, "conditional expression is not a literal"))
        }
        Expr::Arrow(a) => Err(ctx.computed(a.span, export, "arrow function is not a literal")),
        Expr::Fn(f) => Err(ctx.computed(
            f.function.span,
            export,
            "function expression is not a literal",
        )),
        Expr::Class(c) => {
            Err(ctx.computed(c.class.span, export, "class expression is not a literal"))
        }
        Expr::This(t) => Err(ctx.computed(t.span, export, "`this` is not a literal")),
        Expr::Update(u) => Err(ctx.computed(u.span, export, "update expression is not a literal")),
        Expr::Assign(a) => {
            Err(ctx.computed(a.span, export, "assignment expression is not a literal"))
        }
        Expr::Seq(s) => Err(ctx.computed(s.span, export, "sequence expression is not a literal")),
        Expr::Yield(y) => Err(ctx.computed(y.span, export, "`yield` is not a literal")),
        Expr::Await(a) => Err(ctx.computed(a.span, export, "`await` is not a literal")),
        Expr::TaggedTpl(t) => {
            Err(ctx.computed(t.span, export, "tagged template literal is not a literal"))
        }
        Expr::JSXElement(j) => {
            Err(ctx.computed(j.span, export, "JSX element is not a literal value"))
        }
        Expr::JSXFragment(j) => {
            Err(ctx.computed(j.span, export, "JSX fragment is not a literal value"))
        }
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
fn lit_to_json(lit: &Lit, export: &str, ctx: &Ctx<'_>) -> Result<JsonValue, TsxFrontmatterError> {
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
        Lit::Regex(r) => {
            Err(ctx.computed(r.span, export, "regex literal is not representable in JSON"))
        }
        Lit::JSXText(j) => Err(ctx.computed(j.span, export, "JSX text is not a literal value")),
    }
}

/// Render a substitution-free template literal as a plain string,
/// preferring the cooked value and falling back to raw if SWC didn't
/// produce a cooked atom (rare — only happens for explicit invalid
/// escape sequences in tagged templates, which we already reject).
fn tpl_quasi_to_string(tpl: &swc_core::ecma::ast::Tpl) -> Option<String> {
    let q = tpl.quasis.first()?;
    Some(match &q.cooked {
        Some(c) => wtf8_to_string(c),
        None => q.raw.as_str().to_owned(),
    })
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
        // Use strict `<` against 2^64 (u64::MAX as f64 rounds up to 2^64,
        // making `<= u64::MAX as f64` accept 2^64 itself, which then
        // saturates to u64::MAX on the cast).
        if value >= 0.0 && value < 2f64.powi(64) {
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
            other => unreachable!("expected ComputedValue, got {other:?}"),
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
            other => unreachable!("expected ComputedValue, got {other:?}"),
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
            other => unreachable!("expected ComputedValue, got {other:?}"),
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
            // No `prerender` export → carries the SSG default (`true`).
            TsxFrontmatterError::MissingFrontmatter {
                file, prerender, ..
            } => {
                assert_eq!(file, "no-fm.tsx");
                assert!(prerender, "absent prerender defaults to SSG (true)");
            }
            other => unreachable!("expected MissingFrontmatter, got {other:?}"),
        }
    }

    #[test]
    fn lone_prerender_false_surfaced_on_missing_frontmatter() {
        // A page with `export const prerender = false` and NO `frontmatter`
        // still fails with MissingFrontmatter — but the resolved flag must
        // ride along so the `output: static` gate can reject it (#1198),
        // rather than being silently discarded and defaulting to SSG.
        let src = "export const prerender = false;\nexport default function() { return null; }\n";
        let err = extract(src, "ssr-only.tsx").expect_err("must fail (no frontmatter)");
        match err {
            TsxFrontmatterError::MissingFrontmatter {
                file, prerender, ..
            } => {
                assert_eq!(file, "ssr-only.tsx");
                assert!(
                    !prerender,
                    "lone `prerender = false` must survive on MissingFrontmatter"
                );
            }
            other => unreachable!("expected MissingFrontmatter, got {other:?}"),
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
            other => unreachable!("expected DuplicateExport, got {other:?}"),
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
            other => unreachable!("expected WrongShape, got {other:?}"),
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
            other => unreachable!("expected WrongShape, got {other:?}"),
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

    // ----- prerender export -----

    #[test]
    fn prerender_defaults_to_true_when_missing() {
        // No `export const prerender` at all — page is SSG by default.
        let src = r#"
            export const frontmatter = { title: "X" };
        "#;
        let out = extract_ok(src);
        assert!(
            out.prerender,
            "missing prerender export should default to true (SSG)",
        );
    }

    #[test]
    fn prerender_true_literal_extracted() {
        let src = r#"
            export const frontmatter = { title: "X" };
            export const prerender = true;
        "#;
        let out = extract_ok(src);
        assert!(out.prerender, "literal `true` should be extracted as true");
    }

    #[test]
    fn prerender_false_literal_extracted() {
        // The "opt out of SSG, route to SSR manifest" case that T6
        // depends on. Anything other than this exact literal-`false`
        // shape MUST NOT produce `prerender == false`.
        let src = r#"
            export const frontmatter = { title: "X" };
            export const prerender = false;
        "#;
        let out = extract_ok(src);
        assert!(
            !out.prerender,
            "literal `false` should be extracted as false (SSR opt-out)",
        );
    }

    #[test]
    fn prerender_computed_falls_back_to_default_true() {
        // A function call is a runtime value. Per spec the extractor
        // must treat this as "not specified" and leave the default
        // `true` in place — it must NOT error, and it must NOT
        // silently coerce the value to `false`.
        let src = r#"
            function decide(): boolean { return false; }
            export const frontmatter = { title: "X" };
            export const prerender = decide();
        "#;
        let out = extract(src, "page.tsx")
            .expect("computed prerender must not error — it should fall back to the default");
        assert!(
            out.prerender,
            "computed prerender should fall back to default `true`, not silently flip to `false`",
        );
    }

    #[test]
    fn prerender_with_ts_wrappers_recognized() {
        // `as const` / `satisfies` shouldn't hide a literal bool from
        // the extractor; the underlying shape is what matters.
        let src = r#"
            export const frontmatter = { title: "X" };
            export const prerender = false as const;
        "#;
        let out = extract_ok(src);
        assert!(
            !out.prerender,
            "`false as const` should be extracted as false",
        );
    }

    #[test]
    fn prerender_parenthesized_literal_recognized() {
        // Parens are a TS-wrapper-style passthrough — `(false)` is the
        // same as `false` for this extractor.
        let src = r#"
            export const frontmatter = { title: "X" };
            export const prerender = (false);
        "#;
        let out = extract_ok(src);
        assert!(
            !out.prerender,
            "parenthesized `false` should be extracted as false",
        );
    }

    #[test]
    fn prerender_unary_not_falls_back_to_default() {
        // `!true` is a unary expression, not a boolean literal — it
        // requires "evaluation" by the extractor's standards. Per the
        // literal-only contract, this must fall back to the default
        // `true` rather than be evaluated to `false`.
        let src = r#"
            export const frontmatter = { title: "X" };
            export const prerender = !true;
        "#;
        let out = extract(src, "page.tsx")
            .expect("unary-not prerender must not error — it should fall back to the default");
        assert!(
            out.prerender,
            "`!true` is not a literal — should fall back to default `true`",
        );
    }

    #[test]
    fn prerender_duplicate_last_literal_wins() {
        // Duplicate `export const prerender` is not flagged (unlike
        // the strict frontmatter / extension / contentType exports);
        // the last literal-bool initializer wins. Documented choice
        // — `prerender` is a hint with a default, so we'd rather be
        // permissive than break a build over a stale duplicate.
        let src = r#"
            export const frontmatter = { title: "X" };
            export const prerender = true;
            export const prerender = false;
        "#;
        let out = extract_ok(src);
        assert!(
            !out.prerender,
            "duplicate prerender — last literal should win (false)",
        );
    }

    #[test]
    fn prerender_non_boolean_literal_falls_back_to_default() {
        // A string literal is not a boolean — fall back to default
        // rather than coerce or error.
        let src = r#"
            export const frontmatter = { title: "X" };
            export const prerender = "false";
        "#;
        let out = extract(src, "page.tsx").expect(
            "non-bool literal prerender must not error — it should fall back to the default",
        );
        assert!(
            out.prerender,
            "string-literal prerender should fall back to default `true`",
        );
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

    // ----- default export first parameter (#2352) -----

    /// Wrap a default export in the minimal page a successful `extract`
    /// needs, and hand back the captured first-parameter shape.
    fn param_shape_of(default_export: &str) -> DefaultExportFirstParam {
        let src = format!("export const frontmatter = {{ title: \"X\" }};\n{default_export}\n");
        extract_ok(&src).default_export_param
    }

    /// The gate as a `prerender = false` route sees it.
    fn tier_of(default_export: &str) -> Option<RequestParamTier> {
        ssr_request_param_tier(false, &param_shape_of(default_export))
    }

    fn plain_of(default_export: &str) -> PlainFirstParam {
        match param_shape_of(default_export) {
            DefaultExportFirstParam::Plain(p) => p,
            other => unreachable!("expected a plain first parameter, got {other:?}"),
        }
    }

    #[test]
    fn a_shadowed_request_type_does_not_reach_the_strong_tier() {
        // #2361: every one of these introduces a module-scope `Request`
        // binding, so a bare `Request` annotation refers to the LOCAL type
        // and is not evidence of the #2350 mistake. The parameter name
        // keeps the heuristic tier — it is still worth a warning.
        for shadow in [
            "import type { Request } from \"./types\";",
            // A value import can introduce a type too (an imported class).
            "import { Request } from \"./types\";",
            // The LOCAL name is what shadows.
            "import { Foo as Request } from \"./types\";",
            "import Request from \"./types\";",
            "type Request = { params: Record<string, string> };",
            "interface Request { params: Record<string, string> }",
            "enum Request { A }",
            "class Request {}",
            // Exported declarations shadow exactly the same way.
            "export type Request = { a: string };",
            "export interface Request { a: string }",
            "export class Request {}",
        ] {
            let src = format!(
                "{shadow}\nexport default function Handler(request: Request) {{ return null; }}"
            );
            let plain = plain_of(&src);
            assert!(
                !plain.annotation_is_request,
                "a shadowed `Request` must not read as the global type: {shadow:?}"
            );
            assert_eq!(
                tier_of(&src),
                Some(RequestParamTier::Heuristic),
                "expected heuristic (name only) for {shadow:?}"
            );
        }
    }

    #[test]
    fn shadow_detection_is_order_independent() {
        // The declaration may appear AFTER the default export — the shadow
        // set is computed in a pre-pass precisely so the verdict does not
        // depend on declaration order (#2361).
        let src = "export default function Handler(request: Request) { return null; }\n\
                   type Request = { a: string };";
        assert!(!plain_of(src).annotation_is_request);
        assert_eq!(tier_of(src), Some(RequestParamTier::Heuristic));
    }

    #[test]
    fn a_function_type_parameter_named_request_shadows_too() {
        // `<Request>` is a type variable, not the Fetch type.
        let src = "export default function Handler<Request>(request: Request) { return null; }";
        assert!(!plain_of(src).annotation_is_request);
        assert_eq!(tier_of(src), Some(RequestParamTier::Heuristic));
    }

    #[test]
    fn globally_qualified_request_stays_strong_even_when_shadowed() {
        // THE correctness pin for #2361's fix: `globalThis.Request` is
        // explicitly qualified and cannot be shadowed, so it must survive
        // a local `Request` declaration at the strong tier. Downgrading it
        // would silently disarm the gate for anyone writing the
        // unambiguous spelling.
        let src = "import type { Request } from \"./types\";\n\
                   export default function Handler(request: globalThis.Request) { return null; }";
        assert!(plain_of(src).annotation_is_request);
        assert_eq!(tier_of(src), Some(RequestParamTier::Strong));
    }

    #[test]
    fn shapes_that_do_not_actually_shadow_leave_the_strong_tier_intact() {
        for non_shadow in [
            // `declare global` AUGMENTS the global rather than shadowing
            // it — a bare `Request` still means the Fetch type.
            "declare global { interface Request { extra?: string } }",
            // A re-export creates no local binding.
            "export { Request } from \"./types\";",
            "export type { Request } from \"./types\";",
            // The local name here is `Foo`, not `Request`.
            "import { Request as Foo } from \"./types\";",
            // A namespace import binds `X`.
            "import * as X from \"./types\";",
            // Value bindings cannot be what a type annotation resolves to.
            "const Request = 1;",
            "function Request() {}",
        ] {
            let src = format!(
                "{non_shadow}\nexport default function Handler(request: Request) {{ return null; }}"
            );
            assert!(
                plain_of(&src).annotation_is_request,
                "{non_shadow:?} does not shadow — the annotation is still the global `Request`"
            );
            assert_eq!(
                tier_of(&src),
                Some(RequestParamTier::Strong),
                "{non_shadow:?}"
            );
        }
    }

    #[test]
    fn a_binding_inside_the_handler_body_does_not_shadow() {
        // Function-body scope cannot affect what the parameter's own
        // annotation resolves to.
        let src =
            "export default function Handler(request: Request) { class Request {} return null; }";
        assert!(plain_of(src).annotation_is_request);
        assert_eq!(tier_of(src), Some(RequestParamTier::Strong));
    }

    #[test]
    fn body_evidence_promotes_an_unannotated_request_param_to_strong() {
        // #2361: `.js` / `.jsx` routes cannot carry an annotation at all,
        // so without this the strong tier would be unreachable for them
        // and a tier-gated `zfb check` could never enforce the contract.
        // Reading a `Request`-only member is the behavioural substitute.
        for body in [
            "if (request.method !== \"POST\") { return null; } return null;",
            "return request.json();",
            "return request.headers.get(\"x\");",
            // Bracket spelling is the same read.
            "return request[\"method\"];",
        ] {
            let src = format!("export default async function Handler(request) {{ {body} }}");
            let plain = plain_of(&src);
            assert!(
                !plain.annotation_is_request,
                "no annotation is present in {body:?}"
            );
            assert!(
                plain.body_uses_request_members,
                "expected body evidence for {body:?}"
            );
            assert_eq!(
                tier_of(&src),
                Some(RequestParamTier::Strong),
                "body evidence must promote to strong for {body:?}"
            );
        }
        // Arrow with an expression body — no block to walk.
        let arrow = "export default (request) => new Response(request.method);";
        assert!(plain_of(arrow).body_uses_request_members);
        assert_eq!(tier_of(arrow), Some(RequestParamTier::Strong));
    }

    #[test]
    fn body_evidence_alone_never_fires_without_the_naming_signal() {
        // A props object from `getStaticProps` may legitimately carry a
        // field like `url` or `method`, so body evidence is a PROMOTER of
        // the naming heuristic, never an independent path to strong.
        for src in [
            "export default function Page(props) { return props.method; }",
            "export default function Page(data) { return data.json(); }",
            "export default function Page(input: MyProps) { return input.url; }",
        ] {
            assert_eq!(
                tier_of(src),
                None,
                "body evidence must not fire for a non-`request` name: {src:?}"
            );
        }
    }

    #[test]
    fn body_evidence_requires_the_member_to_be_read_off_the_parameter_itself() {
        // `ctx.request.method` reads off a DIFFERENT object; the parameter
        // is never touched, so there is no evidence about it.
        let src = "export default function Handler(request) { return ctx.request.method; }";
        let plain = plain_of(src);
        assert!(
            !plain.body_uses_request_members,
            "a member read off another object is not evidence about the parameter"
        );
        // Still heuristic on the name alone — that part is unchanged.
        assert_eq!(tier_of(src), Some(RequestParamTier::Heuristic));
    }

    #[test]
    fn body_evidence_ignores_a_nested_binding_that_reuses_the_parameter_name() {
        // A nested scope may legitimately rebind the name, and its member
        // reads say nothing about the OUTER props parameter. Attributing
        // them to it would promote a correct page to Strong and hard-fail
        // `zfb check` — the exact false-positive class #2361 is about.
        //
        // Bias: for this promoter a miss is cheap (falls back to Heuristic,
        // which warns) while a false positive breaks a build. So ANY
        // rebinding of the name inside the body disqualifies body evidence,
        // rather than attempting real scope analysis.
        for src in [
            // Nested arrow parameter (codex's reported shape).
            "export default function Page(request) { return items.map(request => request.method); }",
            // Nested function expression parameter.
            "export default function Page(request) { return items.map(function (request) { return request.method; }); }",
            // Local re-declaration.
            "export default function Page(request) { const request2 = 1; let request = new Request('/'); return request.method; }",
            // catch binding.
            "export default function Page(request) { try { f(); } catch (request) { return request.method; } return null; }",
            // Destructured rebinding.
            "export default function Page(request) { const { a: request } = x; return request.method; }",
        ] {
            let plain = plain_of(src);
            assert!(
                !plain.body_uses_request_members,
                "a rebinding of the name must disqualify body evidence: {src:?}"
            );
            assert_eq!(
                tier_of(src),
                Some(RequestParamTier::Heuristic),
                "must stay heuristic (warn), never promote to strong: {src:?}"
            );
        }
    }

    #[test]
    fn a_request_named_param_without_body_evidence_stays_heuristic() {
        // The escape-hatch case: a legitimately-named props parameter that
        // never touches a `Request`-only member warns rather than failing
        // `zfb check` (#2361's severity split).
        for src in [
            "export default function Page(request) { return null; }",
            "export default function Page(req) { return req.title; }",
        ] {
            assert_eq!(tier_of(src), Some(RequestParamTier::Heuristic), "{src:?}");
        }
    }

    #[test]
    fn request_annotation_reaches_the_strong_tier_on_every_function_literal_form() {
        // Condition 2 admits all three function-literal forms; the
        // annotation makes each one strong.
        for form in [
            "export default async function Handler(request: Request) { return null; }",
            "export default async function (request: Request) { return null; }",
            "export default async (request: Request) => null;",
        ] {
            assert_eq!(
                tier_of(form),
                Some(RequestParamTier::Strong),
                "expected the strong tier for {form:?}",
            );
        }
    }

    #[test]
    fn qualified_global_this_request_reaches_the_strong_tier() {
        for form in [
            "export default function Handler(request: globalThis.Request) { return null; }",
            "export default function (request: globalThis.Request) { return null; }",
            "export default (request: globalThis.Request) => null;",
        ] {
            assert_eq!(
                tier_of(form),
                Some(RequestParamTier::Strong),
                "expected the strong tier for {form:?}",
            );
        }
    }

    #[test]
    fn request_and_req_names_reach_the_heuristic_tier_without_an_annotation() {
        for form in [
            "export default function Handler(request) { return null; }",
            "export default function (request) { return null; }",
            "export default (request) => null;",
            "export default function Handler(req) { return null; }",
            "export default function (req) { return null; }",
            "export default (req) => null;",
        ] {
            assert_eq!(
                tier_of(form),
                Some(RequestParamTier::Heuristic),
                "expected the heuristic tier for {form:?}",
            );
        }
    }

    #[test]
    fn plain_param_carries_name_and_source_location() {
        // The location convention is the file's existing one: 1-based
        // line/column of the offending node (here, the parameter).
        let plain = plain_of("export default function Handler(request: Request) { return null; }");
        assert_eq!(plain.name, "request");
        assert!(plain.annotation_is_request);
        // Line 1 is the injected `frontmatter` export, line 2 the handler.
        assert_eq!(plain.line, 2, "parameter should be reported on line 2");
        assert!(plain.col >= 1, "column should be 1-based");
    }

    #[test]
    fn non_firing_shapes_produce_no_tier() {
        // The correct API-handler shape, every destructuring form, and a
        // props parameter. None of these may ever fire.
        let cases = [
            (
                "export default async function Handler() { return null; }",
                DefaultExportFirstParam::Absent,
            ),
            (
                "export default async () => null;",
                DefaultExportFirstParam::Absent,
            ),
            (
                "export default function Page({ params }) { return null; }",
                DefaultExportFirstParam::Destructured,
            ),
            (
                "export default function Page({ title, params }) { return null; }",
                DefaultExportFirstParam::Destructured,
            ),
            (
                "export default function Page([a, b]) { return null; }",
                DefaultExportFirstParam::Destructured,
            ),
            (
                "export default ({ params }) => null;",
                DefaultExportFirstParam::Destructured,
            ),
            (
                "export default class Handler { }",
                DefaultExportFirstParam::Opaque,
            ),
        ];
        for (form, expected) in cases {
            let shape = param_shape_of(form);
            assert_eq!(shape, expected, "unexpected shape for {form:?}");
            assert_eq!(shape.request_param_tier(), None, "{form:?} must not fire");
        }
        // A `props` parameter IS a plain identifier — the shape is
        // captured, but neither tier applies.
        for form in [
            "export default function Page(props) { return null; }",
            "export default function Page(props: PageProps) { return null; }",
        ] {
            assert_eq!(plain_of(form).name, "props");
            assert_eq!(tier_of(form), None, "{form:?} must not fire");
        }
    }

    #[test]
    fn default_exports_the_walk_cannot_see_through_are_opaque() {
        // The epic's documented misses. Each is a real default export
        // whose value lives behind a binding this AST-only walk refuses
        // to resolve — pinned so the boundary is recorded, not
        // rediscovered.
        for form in [
            "function handler(request: Request) { return null; }\nexport default handler;",
            "function handler(request: Request) { return null; }\nexport { handler as default };",
            "export { default } from \"./handler\";",
            "export default wrap(handler);",
            "export default handler as Handler;",
        ] {
            let shape = param_shape_of(form);
            assert_eq!(
                shape,
                DefaultExportFirstParam::Opaque,
                "expected Opaque for {form:?}",
            );
            assert_eq!(shape.request_param_tier(), None, "{form:?} must not fire");
        }
    }

    #[test]
    fn a_named_export_that_is_not_default_leaves_the_shape_absent() {
        // Guard against the `default`-detection above over-triggering on
        // ordinary named re-exports.
        assert_eq!(
            param_shape_of(
                "function handler(request: Request) { return null; }\nexport { handler };"
            ),
            DefaultExportFirstParam::Absent,
        );
        assert_eq!(
            param_shape_of("export { thing } from \"./other\";"),
            DefaultExportFirstParam::Absent,
        );
        // `export { default as handler } from "…"` re-exports someone
        // else's default under a different name — this module has none.
        assert_eq!(
            param_shape_of("export { default as handler } from \"./other\";"),
            DefaultExportFirstParam::Absent,
        );
    }

    #[test]
    fn lookalike_annotations_never_reach_the_strong_tier() {
        // The precision requirement: matched against the type AST, not a
        // rendered string. `MyRequest` and `Request<T>` must not be
        // mistaken for the global `Request`.
        for form in [
            "export default function Handler(request: MyRequest) { return null; }",
            "export default function Handler(request: Request<T>) { return null; }",
            "export default function Handler(request: WebRequest) { return null; }",
            "export default function Handler(request: Req) { return null; }",
            "export default function Handler(request: ns.Request) { return null; }",
        ] {
            let plain = plain_of(form);
            assert!(
                !plain.annotation_is_request,
                "{form:?} must not resolve to the global Request",
            );
            // The *name* still carries the heuristic tier — that is the
            // documented behavior (a mis-annotated `request` parameter is
            // exactly as broken), so assert the tier, not merely "fires".
            assert_eq!(
                tier_of(form),
                Some(RequestParamTier::Heuristic),
                "{form:?} should fall back to the heuristic tier on its name",
            );
        }
    }

    #[test]
    fn local_request_type_alias_is_not_resolved() {
        // `type Req = Request` would need a type checker to follow.
        // Recorded as a miss of the STRONG tier; the parameter name keeps
        // the heuristic one.
        let src =
            "type Req = Request;\nexport default function Handler(request: Req) { return null; }";
        let plain = plain_of(src);
        assert!(!plain.annotation_is_request);
        assert_eq!(tier_of(src), Some(RequestParamTier::Heuristic));
        // A parameter named neither `request` nor `req` behind the same
        // alias is a total miss — the other half of the boundary.
        let renamed =
            "type Req = Request;\nexport default function Handler(input: Req) { return null; }";
        assert_eq!(tier_of(renamed), None);
    }

    #[test]
    fn assign_and_rest_patterns_are_unwrapped() {
        // Condition 3 unwraps a default value and a rest element before
        // deciding whether the parameter is a plain identifier.
        assert_eq!(
            tier_of(
                "export default function Handler(request = new Request(\"/\")) { return null; }"
            ),
            Some(RequestParamTier::Heuristic),
            "a defaulted `request` is still a plain identifier",
        );
        assert_eq!(
            tier_of(
                "export default function Handler(request: Request = new Request(\"/\")) { return null; }"
            ),
            Some(RequestParamTier::Strong),
            "the annotation survives the default value",
        );
        // `(...args)` unwraps to the plain identifier `args` — captured,
        // but neither tier applies.
        let rest = "export default function Handler(...args) { return null; }";
        assert_eq!(plain_of(rest).name, "args");
        assert_eq!(tier_of(rest), None);
        // A destructured rest element stays destructured.
        assert_eq!(
            param_shape_of("export default function Page(...[first]) { return null; }"),
            DefaultExportFirstParam::Destructured,
        );
    }

    #[test]
    fn only_the_first_parameter_is_considered() {
        // A `Request` in second position is not the shape zfb miscalls.
        assert_eq!(
            tier_of("export default function Page({ params }, request: Request) { return null; }"),
            None,
        );
        assert_eq!(
            tier_of("export default function Page(props, request: Request) { return null; }"),
            None,
        );
    }

    #[test]
    fn typescript_this_pseudo_param_is_not_the_first_runtime_parameter() {
        // `this: T` is erased at compile time and receives no argument —
        // the props object still lands in the slot after it. Treating it
        // as the first parameter would report a correct page at the
        // strong tier.
        let ok = "export default function Page(this: Request, props: Props) { return null; }";
        assert_eq!(plain_of(ok).name, "props");
        assert_eq!(tier_of(ok), None, "the erased `this` must not fire");
        // The parameter AFTER it is the real one, and still fires.
        assert_eq!(
            tier_of(
                "export default function Handler(this: unknown, request: Request) { return null; }"
            ),
            Some(RequestParamTier::Strong),
        );
        assert_eq!(
            param_shape_of(
                "export default function Page(this: unknown, { params }) { return null; }"
            ),
            DefaultExportFirstParam::Destructured,
        );
        // A handler whose only declared parameter is the pseudo one takes
        // no runtime arguments at all.
        assert_eq!(
            param_shape_of("export default function Handler(this: Request) { return null; }"),
            DefaultExportFirstParam::Absent,
        );
    }

    #[test]
    fn prerender_true_never_fires_the_gate() {
        // Condition 1 belongs to the caller. An SSG page whose component
        // happens to take a parameter named `request` is not this bug.
        let shape =
            param_shape_of("export default function Page(request: Request) { return null; }");
        assert_eq!(shape.request_param_tier(), Some(RequestParamTier::Strong));
        assert_eq!(
            ssr_request_param_tier(true, &shape),
            None,
            "an SSG route must never be reported",
        );
        assert_eq!(
            ssr_request_param_tier(false, &shape),
            Some(RequestParamTier::Strong),
        );
    }

    #[test]
    fn interface_default_export_has_no_runtime_parameter() {
        // Type-only — there is no callable default export at all.
        assert_eq!(
            param_shape_of("export default interface Props { title: string }"),
            DefaultExportFirstParam::Absent,
        );
    }

    #[test]
    fn missing_frontmatter_path_carries_the_handler_verdict() {
        // The common API-route shape: `prerender = false`, no
        // `frontmatter` export at all. The detector would be blind to
        // its own headline case if the verdict were dropped here.
        let src = "export const prerender = false;\n\
                   export default async function handler(request: Request) { return new Response(\"\"); }\n";
        let err = extract(src, "pages/api/submit.tsx").expect_err("must fail (no frontmatter)");
        match err {
            TsxFrontmatterError::MissingFrontmatter {
                file,
                prerender,
                default_export_param,
            } => {
                assert_eq!(file, "pages/api/submit.tsx");
                assert!(!prerender);
                assert_eq!(
                    ssr_request_param_tier(prerender, &default_export_param),
                    Some(RequestParamTier::Strong),
                    "the strong-tier verdict must survive the error path",
                );
                match default_export_param {
                    DefaultExportFirstParam::Plain(p) => {
                        assert_eq!(p.name, "request");
                        assert_eq!(p.line, 2, "handler is on line 2 of the snippet");
                    }
                    other => unreachable!("expected a plain first parameter, got {other:?}"),
                }
            }
            other => unreachable!("expected MissingFrontmatter, got {other:?}"),
        }
    }

    #[test]
    fn missing_frontmatter_path_carries_a_non_firing_verdict_too() {
        // The error path is not a "found something" channel — a
        // frontmatter-less page with a correct handler reports the shape
        // it actually has.
        let src = "export const prerender = false;\nexport default async function handler() { return new Response(\"\"); }\n";
        let err = extract(src, "pages/api/ok.tsx").expect_err("must fail (no frontmatter)");
        match err {
            TsxFrontmatterError::MissingFrontmatter {
                prerender,
                default_export_param,
                ..
            } => {
                assert_eq!(default_export_param, DefaultExportFirstParam::Absent);
                assert_eq!(
                    ssr_request_param_tier(prerender, &default_export_param),
                    None
                );
            }
            other => unreachable!("expected MissingFrontmatter, got {other:?}"),
        }
    }

    #[test]
    fn page_without_a_default_export_reports_absent() {
        let out = extract_ok("export const frontmatter = { title: \"X\" };");
        assert_eq!(out.default_export_param, DefaultExportFirstParam::Absent);
        assert_eq!(out.default_export_param.request_param_tier(), None);
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

    // ----- number_to_json boundary -----

    #[test]
    fn number_to_json_u64_max_boundary() {
        // u64::MAX as f64 rounds up to 2^64, so feeding it back through
        // `value as u64` would saturate. Values that can't fit in u64
        // must fall through to the f64 path.
        let two_pow_64 = 2f64.powi(64);
        // Exactly 2^64 is out of range — must NOT be stored as u64.
        let v = number_to_json(two_pow_64).expect("finite, should produce Some");
        assert!(
            v.as_u64().is_none(),
            "2^64 must not saturate into u64::MAX; got {v:?}",
        );
        // u64::MAX itself (2^64 - 1) cannot be represented exactly as f64
        // (it rounds to 2^64), so it also falls to the f64 path.
        let u64_max_f64 = u64::MAX as f64;
        let v2 = number_to_json(u64_max_f64).expect("finite, should produce Some");
        assert!(
            v2.as_u64().is_none(),
            "u64::MAX as f64 (rounds to 2^64) must not saturate; got {v2:?}",
        );
        // A value well inside the u64 range should use the integer lane.
        let in_range = number_to_json(1_000_000.0).expect("should produce Some");
        assert_eq!(in_range.as_u64(), Some(1_000_000));
    }

    // ----- tpl_quasi_to_string (fix #3: returns Option, no panic) -----

    #[test]
    fn tpl_quasi_to_string_empty_quasis_returns_none() {
        // Construct a synthetic Tpl with no quasis to prove the function
        // returns None instead of panicking (regression guard for the
        // release panic fixed in this PR).
        use swc_core::common::DUMMY_SP;
        use swc_core::ecma::ast::{Tpl, TplElement};
        let empty_tpl = Tpl {
            span: DUMMY_SP,
            exprs: vec![],
            quasis: vec![],
        };
        assert!(
            tpl_quasi_to_string(&empty_tpl).is_none(),
            "empty quasis should yield None, not panic",
        );
        // A normal single-quasi Tpl should still produce the string.
        let elem = TplElement {
            span: DUMMY_SP,
            tail: true,
            cooked: Some("hello".into()),
            raw: "hello".into(),
        };
        let normal_tpl = Tpl {
            span: DUMMY_SP,
            exprs: vec![],
            quasis: vec![elem],
        };
        assert_eq!(tpl_quasi_to_string(&normal_tpl).as_deref(), Some("hello"));
    }
}
