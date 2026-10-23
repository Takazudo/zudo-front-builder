//! Test oracle helper (#4449): the literal module specifiers an emitted ES
//! module loads, read with a real parser so assertions over bundler output
//! never depend on a regex guessing the minifier's spelling.
use oxc::{
    allocator::Allocator,
    ast::ast::{
        ExportAllDeclaration, ExportFromDeclaration, Expression, ImportDeclaration,
        ImportExpression,
    },
    ast_visit::{walk, Visit},
    parser::Parser,
    span::SourceType,
};

/// Every literal specifier of a static import, side-effect import, re-export
/// (`export … from`, `export * from`) and dynamic `import()` in `source`, in
/// source order. A substitution-free template literal counts as a literal;
/// computed `import()` arguments are skipped. Errors when `source` is not a
/// valid ES module.
#[doc(hidden)]
pub fn module_specifiers(source: &str) -> Result<Vec<String>, String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::mjs()).parse();
    if parsed.fatal_error || parsed.diagnostics.has_errors() {
        return Err("source does not parse as an ES module".to_string());
    }
    let mut found = Specifiers::default();
    found.visit_program(&parsed.program);
    Ok(found.0)
}

#[derive(Default)]
struct Specifiers(Vec<String>);

impl<'a> Visit<'a> for Specifiers {
    fn visit_import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        self.0.push(decl.source.value.as_str().to_owned());
    }

    fn visit_export_from_declaration(&mut self, decl: &ExportFromDeclaration<'a>) {
        self.0.push(decl.source.value.as_str().to_owned());
    }

    fn visit_export_all_declaration(&mut self, decl: &ExportAllDeclaration<'a>) {
        self.0.push(decl.source.value.as_str().to_owned());
    }

    fn visit_import_expression(&mut self, import: &ImportExpression<'a>) {
        match import.source.without_parentheses() {
            Expression::StringLiteral(literal) => self.0.push(literal.value.as_str().to_owned()),
            // Minifiers may spell a literal specifier as a substitution-free template.
            Expression::TemplateLiteral(template) if template.expressions.is_empty() => self
                .0
                .push(template.quasis[0].value.raw.as_str().to_owned()),
            _ => {}
        }
        walk::walk_import_expression(self, import);
    }
}

#[cfg(test)]
mod tests {
    use super::module_specifiers;

    #[test]
    fn collects_every_static_reexport_and_literal_dynamic_specifier() {
        let found = module_specifiers(
            "import a from \"./a.js\";\nimport \"./side.js\";\nexport { b } from \"./b.js\";\n\
             export * from \"./c.js\";\nconst d = () => import(\"./d.js\");\n\
             const e = () => import(`./e.js`);\nconst f = (x) => import(x);\nexport { d, e, f, a };\n",
        )
        .unwrap();
        assert_eq!(
            found,
            [
                "./a.js",
                "./side.js",
                "./b.js",
                "./c.js",
                "./d.js",
                "./e.js"
            ]
        );
    }

    #[test]
    fn rejects_source_that_is_not_an_es_module() {
        assert!(module_specifiers("import {").is_err());
    }
}
