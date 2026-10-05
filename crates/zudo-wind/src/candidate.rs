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

    if text.chars().any(char::is_whitespace) || text.chars().any(char::is_control) {
        return Err(Diagnostic::parse(
            text,
            DiagnosticCode::Zw001,
            "R03",
            "invalid outer candidate characters",
        ));
    }
    // Reserved forms have priority over unknown variants and outer punctuation.
    for variant in &parts.variants {
        if (variant.starts_with("group-") || variant.starts_with("peer-"))
            && tokenizer::split(variant).is_ok_and(|parts| parts.slash.is_some())
        {
            return Err(unsupported(
                text,
                "R07",
                "named relation variants are unsupported",
            ));
        }
        if variant.starts_with("aria-")
            || variant.starts_with("data-")
            || ["group-aria-", "group-data-", "peer-aria-", "peer-data-"]
                .iter()
                .any(|prefix| variant.starts_with(prefix))
        {
            if variant.contains('[')
                && (!valid_bracket_variant(variant)
                    || ![
                        "aria-",
                        "data-",
                        "group-aria-",
                        "group-data-",
                        "peer-aria-",
                        "peer-data-",
                    ]
                    .contains(&variant.split_once('[').map_or("", |(head, _)| head)))
            {
                return Err(Diagnostic::parse(
                    text,
                    DiagnosticCode::Zw001,
                    "R03",
                    "invalid outer candidate characters",
                ));
            }
            return Err(unsupported(
                text,
                "R08",
                "attribute variants are unsupported",
            ));
        }
        if variant.starts_with('[') {
            if !valid_bracket_variant(variant) {
                return Err(Diagnostic::parse(
                    text,
                    DiagnosticCode::Zw001,
                    "R03",
                    "invalid outer candidate characters",
                ));
            }
            return Err(unsupported(
                text,
                "R09",
                "arbitrary-selector variants are unsupported",
            ));
        }
        if ["has-", "not-", "nth-", "nth-last-", "supports-"]
            .iter()
            .any(|prefix| {
                variant
                    .strip_prefix(prefix)
                    .is_some_and(|tail| tail.starts_with('['))
            })
            && valid_bracket_variant(variant)
        {
            return Err(unsupported(
                text,
                "R09",
                "arbitrary-selector variants are unsupported",
            ));
        }
        if ["min-", "max-"].iter().any(|prefix| {
            variant
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.starts_with('['))
        }) && valid_bracket_variant(variant)
        {
            return Err(unsupported(
                text,
                "R24",
                "arbitrary breakpoint variants are unsupported; configure wind.breakpoints",
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
    if parts.variants.iter().any(|part| !valid_outer(part)) {
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
        // An underscore is valid in an authored CSS identifier (BEM, snake_case);
        // the resolver applies the utility-root boundary check that keeps `p_4` invalid.
        if !valid_outer(named) && !valid_authored_identifier(named) {
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

fn valid_bracket_variant(value: &str) -> bool {
    let Some(open) = value.find('[') else {
        return false;
    };
    if !value.ends_with(']')
        || value.len() <= open + 2
        || !valid_outer(&value[..open])
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        return false;
    }
    let mut depth = 0;
    let mut quote = None;
    let mut escape = false;
    for (offset, ch) in value[open..].char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 && open + offset != value.len() - 1 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0 && quote.is_none() && !escape
}

fn valid_authored_identifier(value: &str) -> bool {
    value.contains('_')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_'))
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
            "pointer-coarse:block",
            "pointer-fine:block",
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
    fn pointer_capability_does_not_require_breakpoint_or_dark_config() {
        for pointer in ["pointer-coarse", "pointer-fine"] {
            assert!(
                parse_candidate(&format!("{pointer}:block"), &VariantVocabulary::default()).is_ok()
            );
        }
    }

    #[test]
    fn g05_all_states() {
        for state in [
            "hover",
            "focus",
            "focus-visible",
            "active",
            "disabled",
            "checked",
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
                "checked",
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
        for pseudo in [
            "before",
            "after",
            "marker",
            "placeholder",
            "backdrop",
            "selection",
        ] {
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
    fn g09_named_body_remains_unsplit() {
        assert_eq!(accepted("gap-x-1.5").utility.named, "gap-x-1.5");
    }

    #[test]
    fn g10_decimal_modifier() {
        assert_eq!(
            accepted("bg-panel/0.5").utility.slash_modifier.as_deref(),
            Some("0.5")
        );
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
    fn g13_token_name_is_resolver_input() {
        assert_eq!(accepted("max-w-2xl").utility.named, "max-w-2xl");
    }

    #[test]
    fn g17_positive_integer_is_preserved_for_resolver() {
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
    fn r01_empty_segment() {
        rejected("hover::block", DiagnosticCode::Zw001, "R01");
    }

    #[test]
    fn r02_unbalanced_delimiter() {
        rejected("w-[calc(1px]", DiagnosticCode::Zw001, "R02");
    }

    #[test]
    fn r03_forbidden_outer_punctuation() {
        rejected("w-(10px)", DiagnosticCode::Zw001, "R03");
    }

    #[test]
    fn underscore_identifiers_parse_but_brackets_stay_strict() {
        for text in [
            "card__title",
            "card_title",
            "block__el--mod",
            "md:card__title",
        ] {
            let candidate = parse_candidate(text, &vocabulary()).unwrap();
            assert!(candidate.utility.named.contains('_'), "{text}");
        }
        rejected("card__x-[1px]", DiagnosticCode::Zw005, "R15");
        rejected("p-[", DiagnosticCode::Zw001, "R02");
        rejected("card__tit$le", DiagnosticCode::Zw001, "R03");
    }

    #[test]
    fn r04_unknown_variant() {
        rejected("lg:block", DiagnosticCode::Zw002, "R04");
        for variant in [
            "any-pointer-coarse",
            "any-pointer-fine",
            "motion-safe",
            "motion-reduce",
            "print",
            "portrait",
            "max-pointer-coarse",
        ] {
            rejected(&format!("{variant}:block"), DiagnosticCode::Zw002, "R04");
        }
        for state in ["indeterminate", "required", "invalid"] {
            for prefix in ["", "group-", "peer-"] {
                rejected(
                    &format!("{prefix}{state}:block"),
                    DiagnosticCode::Zw002,
                    "R04",
                );
            }
        }
    }

    #[test]
    fn r05_duplicate_variant_class() {
        rejected("hover:focus:block", DiagnosticCode::Zw003, "R05");
        for text in [
            "pointer-coarse:pointer-coarse:block",
            "pointer-fine:pointer-fine:block",
            "pointer-coarse:pointer-fine:block",
            "pointer-fine:pointer-coarse:block",
        ] {
            rejected(text, DiagnosticCode::Zw003, "R05");
        }
    }

    #[test]
    fn r06_noncanonical_order_suggests_spelling() {
        rejected("hover:sm:block", DiagnosticCode::Zw003, "R06");
        let error = parse_candidate("hover:sm:block", &vocabulary()).unwrap_err();
        assert_eq!(error.suggested_spelling.as_deref(), Some("sm:hover:block"));
        let error =
            parse_candidate("peer-focus:pointer-coarse:dark:sm:block", &vocabulary()).unwrap_err();
        assert_eq!(error.rejection_id, Some("R06"));
        assert_eq!(
            error.suggested_spelling.as_deref(),
            Some("sm:dark:pointer-coarse:peer-focus:block")
        );
    }

    #[test]
    fn r07_named_relation_forms() {
        rejected("group/menu", DiagnosticCode::Zw004, "R07");
        rejected("group-hover/menu:block", DiagnosticCode::Zw004, "R07");
        rejected(
            "group-data-[state=open]/menu:block",
            DiagnosticCode::Zw004,
            "R07",
        );
    }

    #[test]
    fn r08_attribute_variants() {
        rejected("aria-pressed:block", DiagnosticCode::Zw004, "R08");
        rejected("data-[state=open]:block", DiagnosticCode::Zw004, "R08");
        for variant in [
            "group-aria-[expanded=true]",
            "group-data-[current=true]",
            "group-data-[value='[']",
            "group-data-[value=foo/bar]",
            "peer-aria-[pressed=true]",
            "peer-data-[state=open]",
        ] {
            rejected(&format!("{variant}:block"), DiagnosticCode::Zw004, "R08");
        }
    }

    #[test]
    fn r09_arbitrary_selector_variant() {
        rejected("[&_a]:underline", DiagnosticCode::Zw004, "R09");
        rejected(
            "[@media(pointer:coarse)]:block",
            DiagnosticCode::Zw004,
            "R09",
        );
        for variant in [
            "has-[:focus-visible]",
            "has-[\\]]",
            "not-[:first-child]",
            "nth-[2]",
            "nth-last-[3n+1]",
            "supports-[display:grid]",
        ] {
            rejected(&format!("{variant}:block"), DiagnosticCode::Zw004, "R09");
        }
    }

    #[test]
    fn functional_breakpoints_and_malformed_controls() {
        for variant in ["min-[56rem]", "max-[42rem]"] {
            rejected(&format!("{variant}:block"), DiagnosticCode::Zw004, "R24");
        }
        for text in [
            "min-[]:block",
            "has-[]:block",
            "group-data-[]:block",
            "group-data-foo[bar]:block",
            "group-data-foo-[bar]:block",
            "min-[56rem]junk:block",
            "has-[x][y]:block",
            "[&_a]junk:block",
        ] {
            rejected(text, DiagnosticCode::Zw001, "R03");
        }
        for text in ["min-[56rem:block", "has-[:focus-visible:block"] {
            rejected(text, DiagnosticCode::Zw001, "R02");
        }
        rejected("custom-[state=open]:block", DiagnosticCode::Zw001, "R03");
    }

    #[test]
    fn r10_arbitrary_property() {
        rejected("[overflow-wrap:anywhere]", DiagnosticCode::Zw004, "R10");
    }

    #[test]
    fn r11_important_forms() {
        rejected("!block", DiagnosticCode::Zw004, "R11");
        rejected("block!", DiagnosticCode::Zw004, "R11");
    }

    #[test]
    fn r12_negative_permission_is_resolver_owned() {
        let part = accepted("-p-2").utility;
        assert!(part.negative);
        assert_eq!(part.named, "p-2");
    }

    #[test]
    fn r13_fraction_semantics_are_resolver_owned() {
        for text in ["w-1/0", "w-0.5/2"] {
            assert_eq!(
                accepted(text).utility.slash_modifier.as_deref(),
                Some(if text == "w-1/0" { "0" } else { "2" })
            );
        }
    }

    #[test]
    fn r14_modifier_syntax_and_family_semantics() {
        rejected("bg-panel/foo", DiagnosticCode::Zw005, "R14");
        rejected("bg-panel/1.2.3", DiagnosticCode::Zw005, "R14");
        // Range and family checks belong to the utility resolver.
        assert_eq!(
            accepted("bg-panel/101").utility.slash_modifier.as_deref(),
            Some("101")
        );
        assert_eq!(
            accepted("p-2/50").utility.slash_modifier.as_deref(),
            Some("50")
        );
    }

    #[test]
    fn r15_arbitrary_value_shape_and_property_validation_boundary() {
        rejected("w-[]", DiagnosticCode::Zw005, "R15");
        rejected("w-[1px;color:red]", DiagnosticCode::Zw005, "R15");
        assert_eq!(
            accepted("w-[red]").utility.arbitrary_value.as_deref(),
            Some("red")
        );
    }

    #[test]
    fn r16_url_function_including_case_and_escape() {
        for text in ["bg-[url(x)]", "bg-[URL(x)]", "bg-[u\\72l(x)]"] {
            rejected(text, DiagnosticCode::Zw005, "R16");
        }
    }

    #[test]
    fn r17_missing_default_is_resolver_owned() {
        assert_eq!(accepted("rounded").utility.named, "rounded");
    }

    #[test]
    fn r18_invalid_configured_token_name_is_config_owned() {
        // The parser sees only candidates, never `colors.center` or radii.DEFAULT.
        assert_eq!(accepted("text-center").utility.named, "text-center");
    }

    #[test]
    fn r19_invalid_config_shape_is_config_owned() {
        // `spec: 2` and duplicate widths have no candidate representation.
        assert_eq!(accepted("sm:block").variants.0[0].raw, "sm");
    }

    #[test]
    fn r20_recognized_unsupported_families() {
        rejected("ring-2", DiagnosticCode::Zw004, "R20");
        rejected("animate-spin", DiagnosticCode::Zw004, "R20");
    }

    #[test]
    fn r21_unknown_explicit_class_needs_origin_and_catalog() {
        assert_eq!(accepted("site-header").utility.named, "site-header");
    }

    #[test]
    fn r22_variant_on_marker() {
        rejected("hover:group", DiagnosticCode::Zw004, "R22");
    }

    #[test]
    fn r23_child_utility_with_pseudo_element() {
        rejected("before:divide-y", DiagnosticCode::Zw005, "R23");
        rejected("selection:space-x-2", DiagnosticCode::Zw005, "R23");
        rejected("selection:divide-y", DiagnosticCode::Zw005, "R23");
    }
}
