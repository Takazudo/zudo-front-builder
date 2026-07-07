//! Vite-style eager `import.meta.glob(...)` expansion (#665 / sub-issue
//! #670).
//!
//! Relocated out of [`crate::bundler`] into its own module (issue #1402) so
//! the helpers are `pub` and reachable from outside `zfb-build`, in
//! preparation for the full #1385 pt.1 fix (#1404): the downstream
//! shadow-materialisation orchestration for that fix lives in `crates/zfb`,
//! which already depends on both `zfb-build` and `zfb-islands`, so exposing
//! these helpers here (rather than adding a new crate, or a `zfb-build`
//! dependency to `zfb-islands`) is the minimal layering change. This move is
//! a pure relocation: only visibility (`pub`) and module location changed
//! from the original `bundler.rs` implementation — no behavior change.
//!
//! `import.meta.glob(...)` is a Vite-only build-time macro: Vite statically
//! expands it at transform time into a set of `import * as ...` declarations
//! plus an object literal mapping each matched relative path to its namespace.
//! esbuild knows nothing about it and leaves it verbatim; at SSR render time
//! the runtime evaluates `import.meta.glob` as `undefined` and throws, so the
//! module's named exports surface as `undefined`. The esbuild CLI cannot load
//! JS plugins (see the "Esbuild binary resolution" / MDX-precompile
//! rationale in the [`crate::bundler`] module docs), so this expansion MUST
//! run Rust-side, mirroring how MDX is pre-compiled inside
//! `materialise_shadow` before esbuild ever sees the shadow tree.
//!
//! Scope of THIS step (Wave 1):
//!   * Only the eager form `import.meta.glob('<literal>', { eager: true })`.
//!   * Pattern must be a string literal anchored at the source file's dir.
//!   * Anything else (lazy/default, non-literal pattern, `import()` mode, …)
//!     is an explicit `Err` — silently mis-expanding user code is the failure
//!     mode this whole task exists to avoid.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use walkdir::WalkDir;
use zfb_types::path_to_posix_string;

use crate::bundler::is_pruned_infra_dir;

/// Detected `import.meta.glob(...)` call in a source file: the byte range
/// (0-based, into the original source string) the call occupies, plus the
/// validated arguments (or the reason the form is unsupported).
pub struct GlobCall {
    /// 0-based byte offset of the start of the call expression.
    pub lo: usize,
    /// 0-based byte offset just past the end of the call expression.
    pub hi: usize,
    /// `Ok(pattern)` for a supported eager+string-literal form;
    /// `Err(reason)` names the unsupported shape.
    pub parsed: std::result::Result<String, String>,
}

/// SWC `Visit` collector that records every `import.meta.glob(...)` call
/// expression's span and validates its arguments. We collect spans rather
/// than mutate the AST so the rest of the user's source is spliced through
/// byte-for-byte (no codegen → no comment loss, no reformatting).
pub struct GlobCallCollector {
    /// Byte offset to subtract from every span so it indexes the source
    /// string. SWC's `BytePos` is global to the `SourceMap`; the first
    /// file does NOT start at 0 (it starts at `SourceFile::start_pos`,
    /// typically `BytePos(1)`). Indexing the string with a raw `BytePos`
    /// is off-by-one corruption — this base correction is the fix.
    pub base: u32,
    /// Every `import.meta.glob(...)` call found so far, in source order.
    pub calls: Vec<GlobCall>,
}

/// Result of expanding `import.meta.glob(...)` in one source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobExpansion {
    /// Source with supported eager string-literal glob calls rewritten into
    /// namespace imports plus object literals.
    pub expanded_source: String,
    /// Absolute paths of every file matched by any expanded glob call, sorted
    /// and deduped. Shadow materialisers use this to mirror the target graph
    /// without parsing generated import strings.
    pub matched_files: Vec<PathBuf>,
}

impl swc_core::ecma::visit::Visit for GlobCallCollector {
    fn visit_call_expr(&mut self, node: &swc_core::ecma::ast::CallExpr) {
        use swc_core::common::Spanned;
        use swc_core::ecma::visit::VisitWith;
        if let Some(parsed) = parse_import_meta_glob_call(node) {
            let lo = (node.span().lo().0 - self.base) as usize;
            let hi = (node.span().hi().0 - self.base) as usize;
            self.calls.push(GlobCall { lo, hi, parsed });
        }
        // Recurse so nested calls (e.g. inside an arrow body) are still seen.
        node.visit_children_with(self);
    }
}

/// If `call`'s callee is exactly `import.meta.glob`, return `Some` with the
/// validated pattern (`Ok`) or an unsupported-form reason (`Err`). Returns
/// `None` when the callee is some other call entirely — those are left
/// untouched.
fn parse_import_meta_glob_call(
    call: &swc_core::ecma::ast::CallExpr,
) -> Option<std::result::Result<String, String>> {
    use swc_core::ecma::ast::{Callee, Expr, Lit, MemberProp, MetaPropKind};

    // Callee must be a plain expression that is a member access `<obj>.glob`.
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = &**callee_expr else {
        return None;
    };
    // `.glob` (not `.foo`, not a computed `["glob"]`).
    if !matches!(&member.prop, MemberProp::Ident(i) if i.sym == "glob") {
        return None;
    }
    // `<obj>` must be the `import.meta` meta-property.
    match &*member.obj {
        Expr::MetaProp(mp) if mp.kind == MetaPropKind::ImportMeta => {}
        _ => return None,
    }

    // It IS `import.meta.glob(...)` — from here on every divergence is a
    // hard `Err` (the form is reachable user code; we must not mis-expand).
    let unsupported = |reason: &str| {
        Some(Err(format!(
            "zfb bundler: unsupported `import.meta.glob` form: {reason}. \
             Only `import.meta.glob('<string-literal>', {{ eager: true }})` is \
             supported. For lazy / dynamic / `import()`-mode globs, expand the \
             set with a codegen helper or replace it with explicit static \
             imports."
        )))
    };

    if !call.args.is_empty() && call.args[0].spread.is_some() {
        return unsupported("spread argument");
    }

    // First arg: a string-literal pattern.
    let pattern = match call.args.first() {
        Some(arg) => match &*arg.expr {
            Expr::Lit(Lit::Str(s)) => wtf8_atom_to_string(&s.value),
            _ => return unsupported("pattern is not a string literal"),
        },
        None => return unsupported("missing glob pattern argument"),
    };

    // Second arg MUST be `{ eager: true }`. Vite's DEFAULT (no options) is
    // LAZY, so a missing options object is also unsupported here.
    let Some(opts_arg) = call.args.get(1) else {
        return unsupported(
            "missing `{ eager: true }` options object (the \
                            default lazy form is not supported)",
        );
    };
    if opts_arg.spread.is_some() {
        return unsupported("spread in options argument");
    }
    let Expr::Object(obj) = &*opts_arg.expr else {
        return unsupported("options argument is not an object literal");
    };

    let mut eager_is_true = false;
    for prop in &obj.props {
        use swc_core::ecma::ast::{Prop, PropName, PropOrSpread};
        let PropOrSpread::Prop(p) = prop else {
            return unsupported("spread in options object");
        };
        let Prop::KeyValue(kv) = &**p else {
            return unsupported("non key-value property in options object");
        };
        let key = match &kv.key {
            PropName::Ident(i) => i.sym.as_str().to_owned(),
            PropName::Str(s) => wtf8_atom_to_string(&s.value),
            _ => return unsupported("computed key in options object"),
        };
        match key.as_str() {
            "eager" => match &*kv.value {
                Expr::Lit(Lit::Bool(b)) => {
                    if !b.value {
                        return unsupported("`eager: false` (lazy mode)");
                    }
                    eager_is_true = true;
                }
                _ => return unsupported("`eager` is not a boolean literal"),
            },
            // `import: 'default'` selects a named export; `as`/`query` are
            // Vite asset-pipeline knobs. None are modelled in this first step.
            "import" => return unsupported("`import` option (named-export selection)"),
            "query" => return unsupported("`query` option"),
            "as" => return unsupported("`as` option (asset-mode glob)"),
            other => return unsupported(&format!("unrecognised option `{other}`")),
        }
    }

    if !eager_is_true {
        return unsupported("options object does not set `eager: true`");
    }

    Some(Ok(pattern))
}

/// Convert SWC's `Wtf8Atom` string value to a Rust `String`, preferring the
/// already-decoded UTF-8 view and falling back to lossy decoding for the
/// (practically impossible for a glob pattern) lone-surrogate case. Mirrors
/// `zfb_content::tsx_frontmatter`'s helper of the same shape.
fn wtf8_atom_to_string(atom: &swc_core::atoms::Wtf8Atom) -> String {
    match atom.as_str() {
        Some(a) => a.to_owned(),
        None => atom.to_string_lossy().into_owned(),
    }
}

fn collect_import_meta_glob_calls(source: &str) -> Result<Vec<GlobCall>> {
    use swc_core::common::sync::Lrc;
    use swc_core::common::{FileName, SourceMap};
    use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
    use swc_core::ecma::visit::VisitWith;

    // Fast path: if the literal substring never appears, there is nothing to
    // collect. This keeps callers cheap while still making the parser the
    // source of truth for string/comment-only occurrences when it is present.
    if !source.contains("import.meta.glob") {
        return Ok(Vec::new());
    }

    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), source.to_string());
    // Base offset for converting global `BytePos` → 0-based string index.
    let base = fm.start_pos.0;

    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: true,
            decorators: false,
            dts: false,
            no_early_errors: false,
            disallow_ambiguous_jsx_like: false,
        }),
        swc_core::ecma::ast::EsVersion::Es2022,
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let module = parser.parse_module().map_err(|e| {
        anyhow!("zfb bundler: failed to parse module for import.meta.glob expansion: {e:?}")
    })?;

    let mut collector = GlobCallCollector {
        base,
        calls: Vec::new(),
    };
    module.visit_with(&mut collector);

    Ok(collector.calls)
}

/// Return whether `source` contains a real `import.meta.glob(...)` call.
///
/// The literal substring is used only as a fast path. When present, the
/// source is parsed as TSX and inspected with the same [`GlobCallCollector`]
/// used by [`expand_import_meta_glob`], so occurrences inside strings and
/// comments do not count. Unsupported call forms still count as presence:
/// callers that only need to know whether the Vite macro exists must not
/// silently ignore a lazy/dynamic form.
///
/// # Errors
///
/// Returns an error when `source` contains the substring but cannot be parsed
/// as a TSX module.
pub fn source_contains_import_meta_glob(source: &str) -> anyhow::Result<bool> {
    Ok(!collect_import_meta_glob_calls(source)?.is_empty())
}

/// Expand Vite's eager `import.meta.glob(...)` macro in `source`.
///
/// Parses `source` as a TSX module (so JSX / TS syntax is accepted), collects
/// every `import.meta.glob(...)` **call expression** via the SWC AST, and
/// replaces each with an inline object literal `{ './rel': __glob_N, … }`,
/// hoisting the matching `import * as __glob_N from '<rel>'` declarations to
/// the top of the file. Because we splice the original byte ranges, every
/// other byte of the user's source — comments, formatting, even occurrences
/// of the literal text `import.meta.glob(` inside a string or comment — is
/// preserved verbatim and NOT rewritten (those never parse as a call so the
/// AST never sees them).
///
/// `file_dir` is the directory of the **original source file** (NOT the shadow
/// copy); globs resolve against it so the matched relative paths line up with
/// the files esbuild later resolves through the shadow tree.
///
/// `is_excluded` is consulted for every candidate match (absolute path); a
/// `true` verdict drops that file from the expansion. In this Wave-1 task the
/// call sites pass a no-op `&|_| false`; the Wave-2 `bundle.exclude` task
/// (#672) supplies the real predicate. **Path contract:** `is_excluded`
/// receives the *absolute* path of the matched file — the most general shape,
/// from which a glob/relative predicate can derive whatever it needs.
///
/// # Errors
///
/// * The source fails to parse as a TSX module.
/// * Any `import.meta.glob` occurrence uses an unsupported form
///   (non-eager / default-lazy, non-literal pattern, `import()` mode,
///   `as`/`query`/`import` options, …). The message names the form.
///
/// No matching files is NOT an error: it expands to `{}` (Vite parity).
pub fn expand_import_meta_glob(
    source: &str,
    file_dir: &Path,
    is_excluded: &dyn Fn(&Path) -> bool,
) -> Result<String> {
    Ok(expand_import_meta_glob_with_matches(source, file_dir, is_excluded)?.expanded_source)
}

/// Expand Vite's eager `import.meta.glob(...)` macro and return the matched
/// target files alongside the rewritten source.
///
/// See [`expand_import_meta_glob`] for the supported syntax and error
/// contract. This richer form exists for callers that need to mirror the
/// expanded target graph into a shadow tree.
pub fn expand_import_meta_glob_with_matches(
    source: &str,
    file_dir: &Path,
    is_excluded: &dyn Fn(&Path) -> bool,
) -> Result<GlobExpansion> {
    let calls = collect_import_meta_glob_calls(source)?;

    if calls.is_empty() {
        // The substring was present but only inside strings/comments — no
        // real call. Return the source unchanged.
        return Ok(GlobExpansion {
            expanded_source: source.to_string(),
            matched_files: Vec::new(),
        });
    }

    // Calls are collected in source order (visit is pre-order, left-to-right
    // for arguments). Assign `__glob_N` indices in that order for stable
    // output, then splice in DESCENDING `lo` order so earlier offsets don't
    // shift as we mutate.
    let mut import_decls: Vec<String> = Vec::new();
    // (lo, hi, replacement_object_literal) per call, source order.
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    let mut glob_counter: usize = 0;
    let mut matched_files: BTreeSet<PathBuf> = BTreeSet::new();

    for call in &calls {
        let pattern = match &call.parsed {
            Ok(p) => p.clone(),
            Err(reason) => bail!("{reason}"),
        };

        let matches = glob_match_relative(file_dir, &pattern, is_excluded)?;

        // Build the object literal `{ './rel': __glob_N, … }`. Each unique
        // relative path gets one `import * as __glob_N` declaration; keys are
        // already sorted + deduped by `glob_match_relative`.
        let mut entries: Vec<String> = Vec::with_capacity(matches.len());
        for rel in &matches {
            let rel_path = rel.strip_prefix("./").unwrap_or(rel.as_str());
            matched_files.insert(file_dir.join(rel_path));

            let ident = format!("__glob_{glob_counter}");
            glob_counter += 1;
            // serde_json string-quotes the specifier/key so any exotic char
            // in a filename is escaped correctly rather than hand-quoted.
            let spec = serde_json::to_string(rel).unwrap_or_else(|_| format!("{rel:?}"));
            import_decls.push(format!("import * as {ident} from {spec};"));
            entries.push(format!("  {spec}: {ident}"));
        }
        let object_literal = if entries.is_empty() {
            "{}".to_string()
        } else {
            format!("{{\n{}\n}}", entries.join(",\n"))
        };
        replacements.push((call.lo, call.hi, object_literal));
    }

    // Splice the call expressions, descending by `lo` so byte offsets stay
    // valid throughout. Each range is validated against the ORIGINAL source
    // before mutating `out`: the bytes must be in range, lie on char
    // boundaries, and start with `import`. A failure here would mean the
    // SourceMap `BytePos` base correction is wrong — we return an error
    // rather than panic or (worse) splice at the wrong offset and silently
    // corrupt the user's code.
    let mut out = source.to_string();
    for (lo, hi, replacement) in replacements.iter().rev() {
        let valid = source
            .get(*lo..*hi)
            .is_some_and(|s| s.starts_with("import"));
        if !valid {
            bail!(
                "zfb bundler: internal error — import.meta.glob splice range \
                 [{lo}..{hi}] is invalid or does not start at `import` \
                 (BytePos base correction bug). Source length {}.",
                source.len()
            );
        }
        out.replace_range(*lo..*hi, replacement);
    }

    // Hoist the generated `import * as __glob_N` declarations to the top of
    // the module. ESM `import` declarations must be top-level; prepending
    // keeps them valid regardless of where the macro appeared. A leading
    // shebang (`#!…`) MUST stay on line 1, so insert the imports AFTER it
    // rather than before — prepending before a shebang would break a Node
    // script. (Rare for a bundled module, but cheap to get right.)
    let expanded_source = if import_decls.is_empty() {
        out
    } else {
        let decls = import_decls.join("\n");
        if out.starts_with("#!") {
            let nl = out.find('\n').map(|i| i + 1).unwrap_or(out.len());
            let (shebang, rest) = out.split_at(nl);
            format!("{shebang}{decls}\n{rest}")
        } else {
            format!("{decls}\n{out}")
        }
    };

    Ok(GlobExpansion {
        expanded_source,
        matched_files: matched_files.into_iter().collect(),
    })
}

/// Walk `file_dir` and return the POSIX `./`-prefixed relative paths of every
/// file matching `pattern` (Vite/gitignore glob semantics), sorted + deduped.
///
/// `pattern` is matched against the `./`-prefixed POSIX relative path so it
/// behaves exactly like Vite's anchoring (`'./*.tsx'` matches `./a.tsx` but
/// not `./sub/a.tsx`; `'./**/*.tsx'` matches both). `is_excluded` drops a
/// match by its absolute path.
pub fn glob_match_relative(
    file_dir: &Path,
    pattern: &str,
    is_excluded: &dyn Fn(&Path) -> bool,
) -> Result<Vec<String>> {
    // `../`-rooted patterns would shift the walk root above `file_dir`; not
    // modelled in this first step. Reject explicitly rather than silently
    // mis-resolve against the wrong directory.
    if pattern.starts_with("../") || pattern.contains("/../") {
        bail!(
            "zfb bundler: unsupported `import.meta.glob` pattern {pattern:?}: \
             parent-directory (`../`) patterns are not supported in this step. \
             Move the globbed files under the importer's directory, or expand \
             the set with explicit static imports."
        );
    }

    // `literal_separator(true)` makes `*` stop at `/` and `**` recurse —
    // gitignore/Vite semantics. Without it, `./*.tsx` would wrongly match a
    // nested `./a/b.tsx`.
    let glob = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| anyhow!("zfb bundler: invalid import.meta.glob pattern {pattern:?}: {e}"))?
        .compile_matcher();

    let mut out: Vec<String> = Vec::new();
    for entry in WalkDir::new(file_dir)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| !is_pruned_infra_dir(e))
    {
        let entry = match entry {
            Ok(e) => e,
            // A transient walk error (e.g. a vanished file) should not abort
            // the build; skip it. Genuine config errors surface elsewhere.
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let abs = entry.path();
        let rel = match abs.strip_prefix(file_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let rel_posix = path_to_posix_string(rel);
        if rel_posix.is_empty() {
            continue;
        }
        // Match against the `./`-prefixed form so the pattern's own `./`
        // anchor lines up; the matched string is also the object key.
        let keyed = format!("./{rel_posix}");
        if !glob.is_match(&keyed) {
            continue;
        }
        if is_excluded(abs) {
            continue;
        }
        out.push(keyed);
    }

    // Deterministic, byte-stable: sort then dedupe.
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // -----------------------------------------------------------------
    // import.meta.glob eager transform (#665 / #670)
    // -----------------------------------------------------------------

    /// No-op exclude predicate matching the Wave-1 call-site shape.
    fn no_exclude(_: &Path) -> bool {
        false
    }

    /// Create a tempdir, write `(rel, body)` files (creating parent dirs),
    /// and return the dir. Each rel is a POSIX-ish path relative to the dir.
    fn fixture_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        for (rel, body) in files {
            let p = tmp.path().join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&p, body).unwrap();
        }
        tmp
    }

    #[test]
    fn import_meta_glob_zero_matches_expands_to_empty_object() {
        // Directory has the importer only — nothing matches `./widgets/*.tsx`.
        let dir = fixture_dir(&[]);
        let src = r#"
            const mods = import.meta.glob('./widgets/*.tsx', { eager: true });
            export default mods;
        "#;
        let out = expand_import_meta_glob(src, dir.path(), &no_exclude).unwrap();
        assert!(
            !out.contains("import.meta.glob("),
            "macro must be removed even with zero matches:\n{out}"
        );
        assert!(
            out.contains("{}"),
            "zero matches must expand to `{{}}`:\n{out}"
        );
        // No `import * as` declarations when there are no matches.
        assert!(
            !out.contains("import * as __glob_"),
            "no namespace imports should be generated for zero matches:\n{out}"
        );
    }

    #[test]
    fn import_meta_glob_one_match_expands_with_namespace_import() {
        let dir = fixture_dir(&[("widgets/a.tsx", "export const a = 1;")]);
        let src = r#"const m = import.meta.glob('./widgets/*.tsx', { eager: true });"#;
        let expansion = expand_import_meta_glob_with_matches(src, dir.path(), &no_exclude).unwrap();
        let out = &expansion.expanded_source;
        assert!(!out.contains("import.meta.glob("), "macro removed:\n{out}");
        assert!(
            out.contains(r#"import * as __glob_0 from "./widgets/a.tsx";"#),
            "namespace import for the match:\n{out}"
        );
        assert!(
            out.contains(r#""./widgets/a.tsx": __glob_0"#),
            "object key → namespace mapping:\n{out}"
        );
        assert_eq!(
            expansion.matched_files,
            vec![dir.path().join("widgets/a.tsx")],
            "matched target paths are exposed for shadow materialisation"
        );
    }

    #[test]
    fn import_meta_glob_many_matches_sorted_and_deduped() {
        let dir = fixture_dir(&[
            ("widgets/c.tsx", "export const c = 1;"),
            ("widgets/a.tsx", "export const a = 1;"),
            ("widgets/b.tsx", "export const b = 1;"),
            // Non-matching extension — must be ignored.
            ("widgets/readme.md", "# nope"),
        ]);
        let src = r#"export const m = import.meta.glob('./widgets/*.tsx', { eager: true });"#;
        let out = expand_import_meta_glob(src, dir.path(), &no_exclude).unwrap();

        let a = out.find("./widgets/a.tsx").expect("a present");
        let b = out.find("./widgets/b.tsx").expect("b present");
        let c = out.find("./widgets/c.tsx").expect("c present");
        assert!(a < b && b < c, "keys must be sorted a<b<c:\n{out}");
        assert!(
            !out.contains("readme.md"),
            ".md must not match *.tsx:\n{out}"
        );
        // Three distinct namespace identifiers, dense from 0.
        assert!(out.contains("__glob_0"));
        assert!(out.contains("__glob_1"));
        assert!(out.contains("__glob_2"));
    }

    #[test]
    fn import_meta_glob_nested_path_keyed_relative_to_file_dir() {
        // `components/a/b.tsx` globbed from `components/` → key `./a/b.tsx`.
        let dir = fixture_dir(&[
            ("a/b.tsx", "export const b = 1;"),
            ("top.tsx", "export const t = 1;"),
        ]);
        let src = r#"export const m = import.meta.glob('./**/*.tsx', { eager: true });"#;
        let out = expand_import_meta_glob(src, dir.path(), &no_exclude).unwrap();
        assert!(
            out.contains(r#""./a/b.tsx""#),
            "nested match keyed as ./a/b.tsx:\n{out}"
        );
        assert!(
            out.contains(r#""./top.tsx""#),
            "top-level match also present for ./**/*.tsx:\n{out}"
        );
    }

    #[test]
    fn import_meta_glob_single_star_does_not_cross_slash() {
        // `./*.tsx` must NOT match the nested `a/b.tsx` (literal_separator).
        let dir = fixture_dir(&[
            ("a/b.tsx", "export const b = 1;"),
            ("top.tsx", "export const t = 1;"),
        ]);
        let src = r#"export const m = import.meta.glob('./*.tsx', { eager: true });"#;
        let out = expand_import_meta_glob(src, dir.path(), &no_exclude).unwrap();
        assert!(out.contains(r#""./top.tsx""#), "top-level match:\n{out}");
        assert!(
            !out.contains("./a/b.tsx"),
            "single `*` must not cross `/`:\n{out}"
        );
    }

    #[test]
    fn import_meta_glob_unsupported_lazy_default_is_err() {
        // No options object → Vite default is LAZY → unsupported.
        let dir = fixture_dir(&[("widgets/a.tsx", "export const a = 1;")]);
        let src = r#"const m = import.meta.glob('./widgets/*.tsx');"#;
        let err = expand_import_meta_glob(src, dir.path(), &no_exclude)
            .expect_err("default lazy form must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("import.meta.glob"), "names the macro: {msg}");
    }

    #[test]
    fn import_meta_glob_unsupported_eager_false_is_err() {
        let dir = fixture_dir(&[("widgets/a.tsx", "export const a = 1;")]);
        let src = r#"const m = import.meta.glob('./widgets/*.tsx', { eager: false });"#;
        let err = expand_import_meta_glob(src, dir.path(), &no_exclude)
            .expect_err("eager:false must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("eager"), "message names eager/lazy: {msg}");
    }

    #[test]
    fn import_meta_glob_unsupported_nonliteral_pattern_is_err() {
        let dir = fixture_dir(&[]);
        let src = r#"
            const p = './widgets/*.tsx';
            const m = import.meta.glob(p, { eager: true });
        "#;
        let err = expand_import_meta_glob(src, dir.path(), &no_exclude)
            .expect_err("non-literal pattern must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("string literal"), "names the form: {msg}");
    }

    #[test]
    fn import_meta_glob_string_and_comment_occurrences_not_rewritten() {
        // Adversarial: the literal text `import.meta.glob(` appears inside a
        // string literal, a line comment, and a block comment. NONE of those
        // are real call expressions, so the AST never sees them and they must
        // survive verbatim. A real call elsewhere IS rewritten.
        let dir = fixture_dir(&[("widgets/a.tsx", "export const a = 1;")]);
        let src = r#"
            // a comment mentioning import.meta.glob('./x.tsx', { eager: true })
            const doc = "literal import.meta.glob('./y.tsx', { eager: true }) text";
            /* block: import.meta.glob('./z.tsx', { eager: true }) */
            const real = import.meta.glob('./widgets/*.tsx', { eager: true });
        "#;
        let out = expand_import_meta_glob(src, dir.path(), &no_exclude).unwrap();

        // The three decoy occurrences survive verbatim.
        assert!(
            out.contains("// a comment mentioning import.meta.glob('./x.tsx', { eager: true })"),
            "line-comment occurrence must NOT be rewritten:\n{out}"
        );
        assert!(
            out.contains(r#""literal import.meta.glob('./y.tsx', { eager: true }) text""#),
            "string-literal occurrence must NOT be rewritten:\n{out}"
        );
        assert!(
            out.contains("/* block: import.meta.glob('./z.tsx', { eager: true }) */"),
            "block-comment occurrence must NOT be rewritten:\n{out}"
        );
        // The single REAL call was expanded.
        assert!(
            out.contains(r#"import * as __glob_0 from "./widgets/a.tsx";"#),
            "the real call must be expanded:\n{out}"
        );
        // The decoys are NOT among the expanded files (only the real glob ran).
        assert!(
            !out.contains("./x.tsx\": __glob"),
            "decoy x not expanded:\n{out}"
        );
        assert!(
            !out.contains("./y.tsx\": __glob"),
            "decoy y not expanded:\n{out}"
        );
        assert!(
            !out.contains("./z.tsx\": __glob"),
            "decoy z not expanded:\n{out}"
        );
    }

    #[test]
    fn import_meta_glob_is_excluded_predicate_drops_match() {
        // Wiring proof: a closure that excludes `b.tsx` by absolute path must
        // remove it from the expansion while keeping `a.tsx`. This is the seam
        // #672 (`bundle.exclude`) plugs into.
        let dir = fixture_dir(&[
            ("widgets/a.tsx", "export const a = 1;"),
            ("widgets/b.tsx", "export const b = 1;"),
        ]);
        let src = r#"export const m = import.meta.glob('./widgets/*.tsx', { eager: true });"#;
        let exclude = |p: &Path| p.file_name().and_then(|s| s.to_str()) == Some("b.tsx");
        let out = expand_import_meta_glob(src, dir.path(), &exclude).unwrap();
        assert!(out.contains("./widgets/a.tsx"), "a kept:\n{out}");
        assert!(
            !out.contains("./widgets/b.tsx"),
            "b must be excluded by the predicate:\n{out}"
        );
    }

    #[test]
    fn import_meta_glob_no_substring_returns_source_unchanged() {
        // Zero-regression: a file without the macro is returned byte-identical.
        let dir = fixture_dir(&[]);
        let src = "export default function X() { return 1; }\n// glob? no.\n";
        let out = expand_import_meta_glob(src, dir.path(), &no_exclude).unwrap();
        assert_eq!(out, src, "unrelated source must be unchanged");
    }

    #[test]
    fn import_meta_glob_two_calls_in_one_file_splice_and_global_counter() {
        // Hardens the riskiest path: TWO distinct glob calls in one source.
        // Exercises the descending-order multi-range splice and the global
        // `__glob_N` counter that runs across both calls.
        let dir = fixture_dir(&[
            ("x/one.tsx", "export const one = 1;"),
            ("y/two.tsx", "export const two = 2;"),
            ("y/three.tsx", "export const three = 3;"),
        ]);
        let src = r#"
            export const a = import.meta.glob('./x/*.tsx', { eager: true });
            export const b = import.meta.glob('./y/*.tsx', { eager: true });
        "#;
        let out = expand_import_meta_glob(src, dir.path(), &no_exclude).unwrap();

        assert!(
            !out.contains("import.meta.glob("),
            "both calls removed:\n{out}"
        );
        // First call → __glob_0 (./x/one.tsx).
        assert!(
            out.contains(r#"import * as __glob_0 from "./x/one.tsx";"#),
            "first call's match is __glob_0:\n{out}"
        );
        // Second call's matches continue the global counter: __glob_1, __glob_2
        // (sorted: ./y/three.tsx before ./y/two.tsx).
        assert!(
            out.contains(r#"import * as __glob_1 from "./y/three.tsx";"#),
            "second call's first match is __glob_1:\n{out}"
        );
        assert!(
            out.contains(r#"import * as __glob_2 from "./y/two.tsx";"#),
            "second call's second match is __glob_2:\n{out}"
        );
        // Both object literals keep their own keys (splice didn't cross-wire).
        assert!(
            out.contains(r#""./x/one.tsx": __glob_0"#),
            "obj a key:\n{out}"
        );
        assert!(
            out.contains(r#""./y/three.tsx": __glob_1"#),
            "obj b key 1:\n{out}"
        );
        assert!(
            out.contains(r#""./y/two.tsx": __glob_2"#),
            "obj b key 2:\n{out}"
        );
        // x file must NOT appear in the y object and vice-versa: the `a`
        // assignment's object must contain only the x key.
        let a_obj_start = out.find("export const a =").expect("a decl");
        let b_obj_start = out.find("export const b =").expect("b decl");
        let a_slice = &out[a_obj_start..b_obj_start];
        assert!(
            a_slice.contains("./x/one.tsx") && !a_slice.contains("./y/"),
            "object `a` must hold only the x match:\n{a_slice}"
        );
    }

    #[test]
    fn import_meta_glob_preserves_leading_shebang() {
        // A leading `#!` must stay on line 1; generated imports go AFTER it.
        let dir = fixture_dir(&[("widgets/a.tsx", "export const a = 1;")]);
        let src = "#!/usr/bin/env node\nconst m = import.meta.glob('./widgets/*.tsx', { eager: true });\n";
        let out = expand_import_meta_glob(src, dir.path(), &no_exclude).unwrap();
        assert!(
            out.starts_with("#!/usr/bin/env node\n"),
            "shebang must remain on line 1:\n{out}"
        );
        assert!(
            out.contains(r#"import * as __glob_0 from "./widgets/a.tsx";"#),
            "imports still generated after shebang:\n{out}"
        );
        // The import line must come AFTER the shebang, not before it.
        let shebang_at = out.find("#!").unwrap();
        let import_at = out.find("import * as __glob_0").unwrap();
        assert!(shebang_at < import_at, "imports after shebang:\n{out}");
    }

    #[test]
    fn import_meta_glob_parent_dir_pattern_is_err() {
        let dir = fixture_dir(&[]);
        let src = r#"const m = import.meta.glob('../widgets/*.tsx', { eager: true });"#;
        let err = expand_import_meta_glob(src, dir.path(), &no_exclude)
            .expect_err("../ pattern must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("parent-directory") || msg.contains(".."),
            "names the limit: {msg}"
        );
    }

    #[test]
    fn source_contains_import_meta_glob_positive_eager() {
        let src = r#"const m = import.meta.glob('./widgets/*.tsx', { eager: true });"#;
        assert!(
            source_contains_import_meta_glob(src).unwrap(),
            "eager call must count as present"
        );
    }

    #[test]
    fn source_contains_import_meta_glob_positive_lazy() {
        let src = r#"const m = import.meta.glob('./widgets/*.tsx');"#;
        assert!(
            source_contains_import_meta_glob(src).unwrap(),
            "unsupported lazy call must still count as present"
        );
    }

    #[test]
    fn source_contains_import_meta_glob_negative_absent() {
        let src = "export const x = 1;\n";
        assert!(
            !source_contains_import_meta_glob(src).unwrap(),
            "source without the substring must be absent"
        );
    }

    #[test]
    fn source_contains_import_meta_glob_negative_string_and_comment_only() {
        let src = r#"
            // import.meta.glob('./comment.tsx', { eager: true })
            const doc = "import.meta.glob('./string.tsx', { eager: true })";
            /* import.meta.glob('./block.tsx') */
        "#;
        assert!(
            !source_contains_import_meta_glob(src).unwrap(),
            "string/comment-only occurrences must not count as calls"
        );
    }

    #[test]
    fn source_contains_import_meta_glob_parse_failure_is_err() {
        let src = "const m = import.meta.glob('./widgets/*.tsx', { eager: true ";
        let err =
            source_contains_import_meta_glob(src).expect_err("parse failure must be surfaced");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to parse module"),
            "parse error should be explicit: {msg}"
        );
    }
}
