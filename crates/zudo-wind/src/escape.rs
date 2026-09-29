/// Escapes one class name as a CSS identifier (CSSOM identifier serialization).
/// NUL becomes U+FFFD; all other Unicode scalar values retain their identity.
pub fn escape_class_name(value: &str) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch == '\0' {
            output.push('\u{fffd}');
        } else if ch <= '\u{1f}'
            || ch == '\u{7f}'
            || ch.is_ascii_digit() && (index == 0 || index == 1 && value.starts_with('-'))
        {
            output.push_str(&format!("\\{:x} ", ch as u32));
        } else if ch == '-' && value == "-" {
            output.push_str("\\-");
        } else if ch >= '\u{80}' || ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            output.push(ch);
        } else {
            output.push('\\');
            output.push(ch);
        }
    }
    output
}

pub(crate) fn quoted_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightningcss::{
        rules::CssRule,
        selector::Component,
        stylesheet::{ParserOptions, StyleSheet},
    };

    #[test]
    fn punctuation_unicode_and_identifier_edges_round_trip() {
        let mut cases = vec![
            "2xl:w-[24px]".to_owned(),
            "-2".to_owned(),
            "-".to_owned(),
            "--x".to_owned(),
            "é雪😀".to_owned(),
            "a b".to_owned(),
            "_a".to_owned(),
            "a\\b".to_owned(),
        ];
        // Every ASCII scalar except NUL, both initially and inside an ident.
        for ch in (1..=127).map(|code| char::from_u32(code).unwrap()) {
            cases.push(format!("a{ch}b"));
            cases.push(format!("{ch}x"));
        }
        for raw in cases {
            let css = format!(".{} {{ color: red; }}", escape_class_name(&raw));
            let sheet = StyleSheet::parse(&css, ParserOptions::default()).unwrap();
            let CssRule::Style(rule) = &sheet.rules.0[0] else {
                panic!("{raw:?}")
            };
            assert_eq!(rule.selectors.0.len(), 1);
            let components: Vec<_> = rule.selectors.0[0].iter_raw_match_order().collect();
            assert_eq!(components.len(), 1, "{raw:?}");
            let Component::Class(class) = components[0] else {
                panic!("{raw:?}")
            };
            assert_eq!(class.0.as_ref(), raw, "{css}");
        }
        assert_eq!(escape_class_name("2xl:w-[24px]"), "\\32 xl\\:w-\\[24px\\]");
        assert_eq!(escape_class_name("\0"), "\u{fffd}");
        assert_eq!(escape_class_name(""), "");
    }
}
