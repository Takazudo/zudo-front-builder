use crate::{tokenizer, Diagnostic, DiagnosticCode, VariantChain, VariantVocabulary};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub raw: String,
    pub variants: VariantChain,
    pub utility: UtilityPart,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UtilityPart {
    pub negative: bool,
    pub important: bool,
    /// The complete named portion, before a top-level slash.
    pub named: String,
    /// The bracket contents when the name ends in `-[...]`.
    pub arbitrary_value: Option<String>,
    /// Reserved for the resolver's arbitrary-property representation.
    pub arbitrary_property: Option<(String, String)>,
    pub slash_modifier: Option<String>,
}

/// Parses syntax and variant vocabulary. Utility root, token and CSS-property
/// validation happen in the catalog/resolver, after this structural pass.
pub fn parse_candidate(
    text: &str,
    vocabulary: &VariantVocabulary,
) -> Result<Candidate, Diagnostic> {
    let parts = tokenizer::split(text).map_err(|error| match error {
        tokenizer::SplitError::Empty => Diagnostic::parse(
            text,
            DiagnosticCode::Zw001,
            "R01",
            "empty candidate, variant or utility",
        ),
        tokenizer::SplitError::Delimiter => Diagnostic::parse(
            text,
            DiagnosticCode::Zw001,
            "R02",
            "unbalanced delimiter, quote or escape",
        ),
    })?;

    // Reserved forms have priority over unknown variants and outer punctuation.
    for variant in &parts.variants {
        if variant.starts_with("group-") && variant.contains('/')
            || variant.starts_with("peer-") && variant.contains('/')
        {
            return Err(unsupported(
                text,
                "R07",
                "named relation variants are unsupported",
            ));
        }
        if variant.starts_with("aria-") || variant.starts_with("data-") {
            return Err(unsupported(
                text,
                "R08",
                "attribute variants are unsupported",
            ));
        }
        if variant.starts_with('[') {
            return Err(unsupported(
                text,
                "R09",
                "arbitrary-selector variants are unsupported",
            ));
        }
    }
    if parts.utility.starts_with("group/") || parts.utility.starts_with("peer/") {
        return Err(unsupported(text, "R07", "named markers are unsupported"));
    }
    if parts.utility.starts_with('[') {
        return Err(unsupported(
            text,
            "R10",
            "arbitrary properties are unsupported",
        ));
    }
    if parts.leading_important || parts.trailing_important {
        return Err(unsupported(
            text,
            "R11",
            "important modifiers are unsupported",
        ));
    }
    if text.chars().any(char::is_whitespace)
        || text.chars().any(char::is_control)
        || parts.variants.iter().any(|part| !valid_outer(part))
    {
        return Err(Diagnostic::parse(
            text,
            DiagnosticCode::Zw001,
            "R03",
            "invalid outer candidate characters",
        ));
    }

    let utility = parts.utility;
    let outer_prefix = utility.split('[').next().unwrap_or(utility);
    if utility == "-" {
        return Err(Diagnostic::parse(
            text,
            DiagnosticCode::Zw001,
            "R01",
            "empty utility body",
        ));
    }
    if utility.starts_with('(') || outer_prefix.contains('\\') {
        return Err(Diagnostic::parse(
            text,
            DiagnosticCode::Zw001,
            "R03",
            "invalid outer utility characters",
        ));
    }
    if (utility == "group" || utility == "peer") && !parts.variants.is_empty()
        || utility.starts_with("group/")
        || utility.starts_with("peer/")
    {
        return Err(unsupported(
            text,
            "R22",
            "markers cannot have variants or modifiers",
        ));
    }
    if contains_url_function(text) {
        return Err(Diagnostic::parse(
            text,
            DiagnosticCode::Zw005,
            "R16",
            "url() is forbidden in candidates",
        ));
    }
    let variants = vocabulary.parse(text, &parts.variants, utility)?;
    if variants
        .0
        .iter()
        .any(|v| matches!(v.kind, crate::VariantKind::PseudoElement(_)))
        && ["divide-", "space-"]
            .iter()
            .any(|prefix| utility.starts_with(prefix))
    {
        return Err(Diagnostic::parse(
            text,
            DiagnosticCode::Zw005,
            "R23",
            "child utilities cannot target pseudo-elements",
        ));
    }
    if ["ring-", "animate-", "scale-", "transform-"]
        .iter()
        .any(|prefix| utility.starts_with(prefix))
    {
        return Err(unsupported(
            text,
            "R20",
            "recognized utility family is unsupported",
        ));
    }

    let body = utility.strip_prefix('-').unwrap_or(utility);
    let (named, slash_modifier) = if let Some((before, after)) = parts.slash {
        let before = before.strip_prefix('-').unwrap_or(before);
        if before.is_empty() || !valid_decimal(after) {
            return Err(Diagnostic::parse(
                text,
                DiagnosticCode::Zw005,
                "R14",
                "invalid slash modifier",
            ));
        }
        (before, Some(after.to_owned()))
    } else {
        (body, None)
    };
    if named.is_empty() {
        return Err(Diagnostic::parse(
            text,
            DiagnosticCode::Zw001,
            "R01",
            "empty utility body",
        ));
    }
    let arbitrary_value = if let Some(open) = named.find("-[") {
        if !named.ends_with(']') || open == 0 || !valid_outer(&named[..open]) {
            return Err(Diagnostic::parse(
                text,
                DiagnosticCode::Zw005,
                "R15",
                "invalid arbitrary value",
            ));
        }
        let value = &named[open + 2..named.len() - 1];
        if value.is_empty() || value.contains(';') {
            return Err(Diagnostic::parse(
                text,
                DiagnosticCode::Zw005,
                "R15",
                "empty or injected arbitrary value",
            ));
        }
        Some(value.to_owned())
    } else {
        if !valid_outer(named) {
            return Err(Diagnostic::parse(
                text,
                DiagnosticCode::Zw001,
                "R03",
                "invalid named utility characters",
            ));
        }
        None
    };
    Ok(Candidate {
        raw: text.to_owned(),
        variants,
        utility: UtilityPart {
            negative: parts.negative,
            important: false,
            named: named.to_owned(),
            arbitrary_value,
            arbitrary_property: None,
            slash_modifier,
        },
    })
}

fn unsupported(text: &str, rejection_id: &'static str, message: &str) -> Diagnostic {
    Diagnostic::parse(text, DiagnosticCode::Zw004, rejection_id, message)
}

fn valid_outer(value: &str) -> bool {
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '.')
}

fn valid_decimal(value: &str) -> bool {
    let mut parts = value.split('.');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    parts.next().is_none()
        && !first.is_empty()
        && first.bytes().all(|ch| ch.is_ascii_digit())
        && second.is_none_or(|fraction| {
            !fraction.is_empty() && fraction.bytes().all(|ch| ch.is_ascii_digit())
        })
}

fn contains_url_function(text: &str) -> bool {
    let mut decoded = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 && chars.peek().is_some_and(|next| next.is_ascii_hexdigit()) {
            hex.push(chars.next().unwrap_or_default());
        }
        if hex.is_empty() {
            if let Some(next) = chars.next() {
                decoded.push(next);
            }
        } else if let Ok(codepoint) = u32::from_str_radix(&hex, 16) {
            if let Some(decoded_char) = char::from_u32(codepoint) {
                decoded.push(decoded_char);
            }
            if chars.peek().is_some_and(|next| next.is_whitespace()) {
                chars.next();
            }
        }
    }
    decoded.to_ascii_lowercase().contains("url(")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocabulary() -> VariantVocabulary {
        VariantVocabulary::new(["sm".to_owned(), "md".to_owned()], true)
    }

    fn accepted(text: &str) -> Candidate {
        parse_candidate(text, &vocabulary()).unwrap_or_else(|error| panic!("{text}: {error:?}"))
    }

    fn rejected(text: &str, code: DiagnosticCode, id: &str) {
        let error = parse_candidate(text, &vocabulary()).unwrap_err();
        assert_eq!((error.code, error.rejection_id), (code, Some(id)), "{text}");
    }

    #[test]
    fn g01_candidate_chain() {
        assert_eq!(accepted("sm:dark:hover:bg-panel").variants.0.len(), 3);
    }

    #[test]
    fn g02_variant_alternatives() {
        for text in [
            "sm:block",
            "max-sm:block",
            "dark:block",
            "group-hover:block",
            "focus-visible:block",
            "before:block",
        ] {
            assert_eq!(accepted(text).variants.0.len(), 1);
        }
    }

    #[test]
    fn g03_breakpoint_min_and_max() {
        assert!(matches!(
            accepted("max-sm:hidden").variants.0[0].kind,
            crate::VariantKind::Breakpoint { max: true, .. }
        ));
    }

    #[test]
    fn g04_configured_dark() {
        accepted("dark:block");
        rejected("not-dark:block", DiagnosticCode::Zw002, "R04");
        assert_eq!(
            parse_candidate("dark:block", &VariantVocabulary::default())
                .unwrap_err()
                .code,
            DiagnosticCode::Zw002
        );
    }

    #[test]
    fn g05_all_states() {
        for state in [
            "hover",
            "focus",
            "focus-visible",
            "active",
            "disabled",
            "first",
            "last",
            "focus-within",
            "open",
        ] {
            accepted(&format!("{state}:block"));
        }
    }

    #[test]
    fn g06_all_relation_states() {
        for relation in ["group", "peer"] {
            for state in [
                "hover",
                "focus",
                "focus-visible",
                "active",
                "disabled",
                "first",
                "last",
                "focus-within",
                "open",
            ] {
                accepted(&format!("{relation}-{state}:block"));
            }
        }
    }

    #[test]
    fn g07_all_pseudo_elements() {
        for pseudo in ["before", "after", "marker", "placeholder", "backdrop"] {
            accepted(&format!("{pseudo}:block"));
        }
    }

    #[test]
    fn g08_negative_arbitrary_and_modifier_structure() {
        assert!(accepted("-mt-2").utility.negative);
        assert_eq!(
            accepted("bg-panel/40").utility.slash_modifier.as_deref(),
            Some("40")
        );
        assert_eq!(
            accepted("-left-[calc(var(--spacing-icon-lg)/2)]")
                .utility
                .arbitrary_value
                .as_deref(),
            Some("calc(var(--spacing-icon-lg)/2)")
        );
    }

    #[test]
    fn g09_named_and_g10_decimal_remain_unsplit() {
        assert_eq!(accepted("gap-x-1.5").utility.named, "gap-x-1.5");
        assert_eq!(accepted("py-0.5").utility.named, "py-0.5");
    }

    #[test]
    fn g11_fraction_remains_for_catalog() {
        assert_eq!(
            accepted("w-1/2").utility.slash_modifier.as_deref(),
            Some("2")
        );
    }

    #[test]
    fn g12_balanced_css_value() {
        assert_eq!(
            accepted("grid-cols-[auto_1fr]")
                .utility
                .arbitrary_value
                .as_deref(),
            Some("auto_1fr")
        );
        assert_eq!(accepted("aspect-[1200/630]").utility.slash_modifier, None);
        assert_eq!(
            accepted("transition-[left,color]")
                .utility
                .arbitrary_value
                .as_deref(),
            Some("left,color")
        );
    }

    #[test]
    fn g13_token_name_and_g17_positive_integer_are_resolver_inputs() {
        assert_eq!(accepted("max-w-2xl").utility.named, "max-w-2xl");
        assert_eq!(
            accepted("aspect-[1200/630]")
                .utility
                .arbitrary_value
                .as_deref(),
            Some("1200/630")
        );
    }

    #[test]
    fn g14_standalone_markers() {
        accepted("group");
        accepted("peer");
    }

    #[test]
    fn g15_escape_inside_value() {
        assert_eq!(
            accepted("content-['a\\_b']")
                .utility
                .arbitrary_value
                .as_deref(),
            Some("'a\\_b'")
        );
    }

    #[test]
    fn g16_root_is_preserved_for_longest_match() {
        assert_eq!(accepted("scroll-mt-2").utility.named, "scroll-mt-2");
    }

    #[test]
    fn g18_configured_breakpoint() {
        accepted("sm:block");
        rejected("lg:block", DiagnosticCode::Zw002, "R04");
    }

    #[test]
    fn structural_split_preserves_nested_separators() {
        let a = crate::structural_split("[&_a:hover]:underline").unwrap();
        assert_eq!((a.variants, a.utility), (vec!["[&_a:hover]"], "underline"));
        let b = crate::structural_split("aspect-[1200/630]").unwrap();
        assert_eq!((b.utility, b.slash), ("aspect-[1200/630]", None));
        let c = crate::structural_split("w-[calc(100%-2rem)]").unwrap();
        assert_eq!((c.utility, c.slash), ("w-[calc(100%-2rem)]", None));
    }

    #[test]
    fn r01_to_r11_syntax_rejections() {
        for (text, code, id) in [
            ("hover::block", DiagnosticCode::Zw001, "R01"),
            ("w-[calc(1px]", DiagnosticCode::Zw001, "R02"),
            ("w-(10px)", DiagnosticCode::Zw001, "R03"),
            ("lg:block", DiagnosticCode::Zw002, "R04"),
            ("hover:focus:block", DiagnosticCode::Zw003, "R05"),
            ("hover:sm:block", DiagnosticCode::Zw003, "R06"),
            ("group/menu", DiagnosticCode::Zw004, "R07"),
            ("group-hover/menu:block", DiagnosticCode::Zw004, "R07"),
            ("aria-pressed:block", DiagnosticCode::Zw004, "R08"),
            ("data-[state=open]:block", DiagnosticCode::Zw004, "R08"),
            ("[&_a]:underline", DiagnosticCode::Zw004, "R09"),
            ("[overflow-wrap:anywhere]", DiagnosticCode::Zw004, "R10"),
            ("!block", DiagnosticCode::Zw004, "R11"),
            ("block!", DiagnosticCode::Zw004, "R11"),
        ] {
            rejected(text, code, id);
        }
    }

    #[test]
    fn r06_suggests_canonical_order() {
        let error = parse_candidate("hover:sm:block", &vocabulary()).unwrap_err();
        assert_eq!(error.suggested_spelling.as_deref(), Some("sm:hover:block"));
    }

    #[test]
    fn r20_r22_r23_rejections() {
        rejected("ring-2", DiagnosticCode::Zw004, "R20");
        rejected("animate-spin", DiagnosticCode::Zw004, "R20");
        rejected("hover:group", DiagnosticCode::Zw004, "R22");
        rejected("before:divide-y", DiagnosticCode::Zw005, "R23");
    }

    #[test]
    fn slash_modifier_requires_decimal_syntax() {
        rejected("bg-panel/foo", DiagnosticCode::Zw005, "R14");
        rejected("bg-panel/1.2.3", DiagnosticCode::Zw005, "R14");
    }

    #[test]
    fn r16_url_function_including_case_and_escape() {
        for text in ["bg-[url(x)]", "bg-[URL(x)]", "bg-[u\\72l(x)]"] {
            rejected(text, DiagnosticCode::Zw005, "R16");
        }
    }

    #[test]
    fn resolver_rejections_keep_their_structural_inputs() {
        // R12–R19 and R21 need the catalog, tokens, CSS-property parser or origin.
        for text in [
            "-p-2",
            "w-1/0",
            "w-0.5/2",
            "bg-panel/101",
            "p-2/50",
            "w-[red]",
            "rounded",
            "site-header",
        ] {
            accepted(text);
        }
    }
}
