//! Esbuild keeps an unresolvable `import()` external, as a warning, when the
//! call's rejection is handled, so optional peers can be probed at runtime
//! (#4429). Rolldown already does this silently inside `try`; this covers the
//! `.then(onFulfilled, onRejected)` and `.catch(...)` chains esbuild accepts.
use oxc::{
    allocator::Allocator,
    ast::ast::{
        Argument, CallExpression, ExportAllDeclaration, ExportFromDeclaration, Expression,
        ImportDeclaration, ImportExpression,
    },
    ast_visit::{walk, Visit},
    parser::Parser,
    span::SourceType,
};
use std::{collections::BTreeSet, path::Path};

/// Whether a rendered `UNRESOLVED_IMPORT` warning names a specifier that its
/// importer only loads through rejection-handled `import()` calls.
pub(super) fn handled_unresolved(rendered: &str, process_cwd: &Path) -> bool {
    let Some((specifier, importer)) = rendered
        .lines()
        .next()
        .and_then(|line| line.split_once("Could not resolve '"))
        .and_then(|(_, rest)| rest.split_once("' in "))
    else {
        return false;
    };
    let importer = super::lexical_normal(&process_cwd.join(importer.trim()));
    std::fs::read_to_string(&importer)
        .is_ok_and(|source| only_handled_dynamic_imports(&importer, &source, specifier))
}

/// True when `specifier` appears in `source` only as `import()` calls whose
/// rejection is handled. Anything unparseable, or any other import form of the
/// same specifier, keeps the unresolved import an error.
fn only_handled_dynamic_imports(path: &Path, source: &str, specifier: &str) -> bool {
    let Ok(source_type) = SourceType::from_path(path) else {
        return false;
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.fatal_error || parsed.diagnostics.has_errors() {
        return false;
    }
    let mut finder = Finder {
        specifier,
        handled_calls: BTreeSet::new(),
        handled: 0,
        other: 0,
    };
    finder.visit_program(&parsed.program);
    finder.handled > 0 && finder.other == 0
}

struct Finder<'s> {
    specifier: &'s str,
    /// Start offsets of `import()` expressions a handler chain covers.
    handled_calls: BTreeSet<u32>,
    handled: usize,
    other: usize,
}

impl Finder<'_> {
    fn names_specifier(&self, source: &Expression) -> bool {
        matches!(source.without_parentheses(), Expression::StringLiteral(s) if s.value == self.specifier)
    }

    /// A then/catch chain around the `import()`: a rejection passes through a
    /// one-argument `.then`, so a `.catch` or a two-argument `.then` anywhere
    /// in the chain handles it.
    fn mark_handler_chain(&mut self, call: &CallExpression) {
        let mut call = call;
        let mut handles_rejection = false;
        loop {
            let Expression::StaticMemberExpression(member) = call.callee.without_parentheses()
            else {
                return;
            };
            let name = member.property.name.as_str();
            if name != "then" && name != "catch" {
                return;
            }
            handles_rejection |= name == "catch" || call.arguments.len() >= 2;
            match member.object.without_parentheses() {
                Expression::CallExpression(inner) => call = inner,
                Expression::ImportExpression(import) => {
                    if handles_rejection {
                        self.handled_calls.insert(import.span.start);
                    }
                    return;
                }
                _ => return,
            }
        }
    }
}

impl<'a> Visit<'a> for Finder<'_> {
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        self.mark_handler_chain(call);
        if matches!(&call.callee, Expression::Identifier(id) if id.name == "require")
            && matches!(call.arguments.first(), Some(Argument::StringLiteral(s)) if s.value == self.specifier)
        {
            self.other += 1;
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_import_expression(&mut self, import: &ImportExpression<'a>) {
        if self.names_specifier(&import.source) {
            if self.handled_calls.contains(&import.span.start) {
                self.handled += 1;
            } else {
                self.other += 1;
            }
        }
        walk::walk_import_expression(self, import);
    }

    fn visit_import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        if !decl.import_kind.is_type() && decl.source.value == self.specifier {
            self.other += 1;
        }
    }

    fn visit_export_from_declaration(&mut self, decl: &ExportFromDeclaration<'a>) {
        if !decl.export_kind.is_type() && decl.source.value == self.specifier {
            self.other += 1;
        }
    }

    fn visit_export_all_declaration(&mut self, decl: &ExportAllDeclaration<'a>) {
        if !decl.export_kind.is_type() && decl.source.value == self.specifier {
            self.other += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::only_handled_dynamic_imports;
    use std::path::Path;

    fn handled(source: &str) -> bool {
        only_handled_dynamic_imports(Path::new("lib/optional.ts"), source, "peer")
    }

    #[test]
    fn rejection_handlers_esbuild_accepts_make_the_import_optional() {
        for source in [
            r#"export const f = () => import("peer").then((m) => m, () => null);"#,
            r#"export const f = () => import("peer").catch(() => null);"#,
            r#"export const f = () => import("peer").then((m) => m.x).catch(() => null);"#,
            r#"export const f = () => (import("peer")).then((m) => m).then(String, () => "");"#,
            r#"export const f = () => import("peer").then(a, b).then(c);"#,
        ] {
            assert!(handled(source), "{source}");
        }
    }

    #[test]
    fn unhandled_or_mixed_imports_stay_errors() {
        for source in [
            r#"export const f = () => import("peer");"#,
            r#"export const f = () => import("peer").then((m) => m);"#,
            r#"export const f = () => import("peer").finally(() => {});"#,
            r#"export const f = () => { const p = import("peer"); return p.catch(() => null); };"#,
            r#"import x from "peer"; export const f = () => import("peer").catch(() => x);"#,
            r#"export { y } from "peer"; export const f = () => import("peer").catch(() => 1);"#,
            r#"export const f = () => [import("peer").catch(() => 1), import("peer")];"#,
            r#"export const f = () => import("other").catch(() => 1);"#,
            r#"export const f = () => import("peer").catch(() => null"#,
        ] {
            assert!(!handled(source), "{source}");
        }
    }

    #[test]
    fn type_only_imports_of_the_specifier_do_not_count() {
        assert!(handled(
            r#"import type { Peer } from "peer";
export const f = (): Promise<Peer | null> => import("peer").catch(() => null);"#
        ));
    }
}
