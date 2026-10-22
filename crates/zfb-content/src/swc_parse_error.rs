//! Readable text for SWC parser errors.

use swc_core::common::{SourceMap, Spanned};
use swc_core::ecma::parser::error::Error;

/// Describe `err` as `line L, column C: <message>` (both 1-based).
///
/// SWC's `Debug` form shows only a byte range and an error enum (for example
/// `Error { error: (2264..2265, TS1109) }`), which gives the user nothing to
/// act on (#4427). `cm` must be the source map the failing file was parsed
/// from.
pub fn describe_parse_error(cm: &SourceMap, err: &Error) -> String {
    let loc = cm.lookup_char_pos(err.span().lo);
    format!(
        "line {}, column {}: {}",
        loc.line,
        loc.col_display + 1,
        err.kind().msg()
    )
}

#[cfg(test)]
mod tests {
    use super::describe_parse_error;
    use swc_core::common::{sync::Lrc, FileName, SourceMap};
    use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};

    fn describe(source: &str) -> String {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(FileName::Anon.into(), source.to_string());
        let lexer = Lexer::new(
            Syntax::Typescript(TsSyntax {
                tsx: true,
                ..Default::default()
            }),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let err = Parser::new_from(lexer).parse_module().unwrap_err();
        describe_parse_error(&cm, &err)
    }

    #[test]
    fn names_line_column_and_message() {
        assert_eq!(
            describe("const ok = 1;\nconst = 1;\n"),
            "line 2, column 7: Unexpected token `=`. Expected yield, an identifier, [ or {"
        );
    }

    #[test]
    fn an_error_code_becomes_its_message() {
        let text = describe("export const a = 1;\nconst b = = 2;\n");
        assert!(text.starts_with("line 2, column 11: "), "{text}");
        assert!(!text.contains("TS1109"), "{text}");
    }
}
