//! Bounded migration vocabulary: Tailwind v4 utility names that zudo-wind v1
//! deliberately does not implement. A match is reported, never generated, so
//! a migrating project learns which classes silently lost their CSS.

use crate::{Candidate, TokenCategory, ValidatedTokens};

/// Bump when an entry is added or removed; diagnostics name the version so a
/// report stays interpretable after the vocabulary changes.
pub const MIGRATION_VOCABULARY_VERSION: u32 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ForeignValue {
    /// The root alone, such as `container`.
    Bare,
    Keyword(&'static str),
    Integer,
    /// A bracketed value, such as `content-[""]`.
    Arbitrary,
    /// A configured token in the named category. Shared with gradient stops,
    /// where arbitrary authored prefixes must not be claimed wholesale.
    Token(TokenCategory),
    /// Any nonempty suffix; only for roots no authored class plausibly shares.
    Any,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForeignFamily {
    pub root: &'static str,
    values: &'static [ForeignValue],
    /// The authored-CSS replacement the warning recommends.
    pub alternative: &'static str,
}

use ForeignValue::{Any, Arbitrary, Bare, Integer, Keyword, Token};

const DISPLAY: &str = "a display declaration";
const GRADIENT: &str = "a background-image gradient declaration";
const LINEAR_GRADIENT: &str = "a background-image: linear-gradient(...) declaration";
const RADIAL_GRADIENT: &str = "a background-image: radial-gradient(...) declaration";
const CONIC_GRADIENT: &str = "a background-image: conic-gradient(...) declaration";

const LEGACY_GRADIENT_DIRECTIONS: &[ForeignValue] = &[
    Keyword("to-t"),
    Keyword("to-tr"),
    Keyword("to-r"),
    Keyword("to-br"),
    Keyword("to-b"),
    Keyword("to-bl"),
    Keyword("to-l"),
    Keyword("to-tl"),
    Arbitrary,
];

const LINEAR_GRADIENT_DIRECTIONS: &[ForeignValue] = &[
    Keyword("to-t"),
    Keyword("to-tr"),
    Keyword("to-r"),
    Keyword("to-br"),
    Keyword("to-b"),
    Keyword("to-bl"),
    Keyword("to-l"),
    Keyword("to-tl"),
    Integer,
    Arbitrary,
];

const GRADIENT_STOP_VALUES: &[ForeignValue] = &[
    Token(TokenCategory::Color),
    Keyword("current"),
    Keyword("transparent"),
    Arbitrary,
];

const FAMILIES: &[ForeignFamily] = &[
    ForeignFamily {
        root: "bg-linear",
        values: LINEAR_GRADIENT_DIRECTIONS,
        alternative: LINEAR_GRADIENT,
    },
    ForeignFamily {
        root: "bg-gradient",
        values: LEGACY_GRADIENT_DIRECTIONS,
        alternative: LINEAR_GRADIENT,
    },
    ForeignFamily {
        root: "bg-radial",
        values: &[Bare, Arbitrary],
        alternative: RADIAL_GRADIENT,
    },
    ForeignFamily {
        root: "bg-conic",
        values: &[Bare, Integer, Arbitrary],
        alternative: CONIC_GRADIENT,
    },
    ForeignFamily {
        root: "from",
        values: GRADIENT_STOP_VALUES,
        alternative: GRADIENT,
    },
    ForeignFamily {
        root: "via",
        values: GRADIENT_STOP_VALUES,
        alternative: GRADIENT,
    },
    ForeignFamily {
        root: "to",
        values: GRADIENT_STOP_VALUES,
        alternative: GRADIENT,
    },
    ForeignFamily {
        root: "isolate",
        values: &[Bare],
        alternative: "an isolation declaration",
    },
    ForeignFamily {
        root: "isolation",
        values: &[Keyword("auto")],
        alternative: "an isolation declaration",
    },
    ForeignFamily {
        root: "float",
        values: &[
            Keyword("left"),
            Keyword("right"),
            Keyword("none"),
            Keyword("start"),
            Keyword("end"),
        ],
        alternative: "a float declaration",
    },
    ForeignFamily {
        root: "clear",
        values: &[
            Keyword("left"),
            Keyword("right"),
            Keyword("both"),
            Keyword("none"),
            Keyword("start"),
            Keyword("end"),
        ],
        alternative: "a clear declaration",
    },
    ForeignFamily {
        root: "break",
        values: &[Keyword("keep")],
        alternative: "a word-break declaration",
    },
    ForeignFamily {
        root: "break-before",
        values: &[
            Keyword("auto"),
            Keyword("avoid"),
            Keyword("all"),
            Keyword("avoid-page"),
            Keyword("page"),
            Keyword("left"),
            Keyword("right"),
            Keyword("column"),
        ],
        alternative: "a break-before declaration",
    },
    ForeignFamily {
        root: "break-after",
        values: &[
            Keyword("auto"),
            Keyword("avoid"),
            Keyword("all"),
            Keyword("avoid-page"),
            Keyword("page"),
            Keyword("left"),
            Keyword("right"),
            Keyword("column"),
        ],
        alternative: "a break-after declaration",
    },
    ForeignFamily {
        root: "break-inside",
        values: &[
            Keyword("auto"),
            Keyword("avoid"),
            Keyword("avoid-page"),
            Keyword("avoid-column"),
        ],
        alternative: "a break-inside declaration",
    },
    ForeignFamily {
        root: "origin",
        values: &[
            Keyword("center"),
            Keyword("top"),
            Keyword("top-right"),
            Keyword("right"),
            Keyword("bottom-right"),
            Keyword("bottom"),
            Keyword("bottom-left"),
            Keyword("left"),
            Keyword("top-left"),
            Arbitrary,
        ],
        alternative: "a transform-origin declaration",
    },
    ForeignFamily {
        root: "blur",
        values: &[
            Bare,
            Keyword("none"),
            Keyword("xs"),
            Keyword("sm"),
            Keyword("md"),
            Keyword("lg"),
            Keyword("xl"),
            Keyword("2xl"),
            Keyword("3xl"),
            Arbitrary,
        ],
        alternative: "a filter declaration",
    },
    ForeignFamily {
        root: "brightness",
        values: &[Integer, Arbitrary],
        alternative: "a filter declaration",
    },
    ForeignFamily {
        root: "contrast",
        values: &[Integer, Arbitrary],
        alternative: "a filter declaration",
    },
    ForeignFamily {
        root: "saturate",
        values: &[Integer, Arbitrary],
        alternative: "a filter declaration",
    },
    ForeignFamily {
        root: "hue-rotate",
        values: &[Integer, Arbitrary],
        alternative: "a filter declaration",
    },
    ForeignFamily {
        root: "grayscale",
        values: &[Bare, Integer, Arbitrary],
        alternative: "a filter declaration",
    },
    ForeignFamily {
        root: "invert",
        values: &[Bare, Integer, Arbitrary],
        alternative: "a filter declaration",
    },
    ForeignFamily {
        root: "sepia",
        values: &[Bare, Integer, Arbitrary],
        alternative: "a filter declaration",
    },
    ForeignFamily {
        root: "drop-shadow",
        values: &[
            Bare,
            Keyword("none"),
            Keyword("xs"),
            Keyword("sm"),
            Keyword("md"),
            Keyword("lg"),
            Keyword("xl"),
            Keyword("2xl"),
            Arbitrary,
            Token(TokenCategory::Shadow),
        ],
        alternative: "a filter declaration",
    },
    ForeignFamily {
        root: "filter",
        values: &[Keyword("none"), Arbitrary],
        alternative: "a filter declaration",
    },
    ForeignFamily {
        root: "backdrop-filter",
        values: &[Keyword("none"), Arbitrary],
        alternative: "a backdrop-filter declaration",
    },
    ForeignFamily {
        root: "backdrop-brightness",
        values: &[Integer, Arbitrary],
        alternative: "a backdrop-filter declaration",
    },
    ForeignFamily {
        root: "backdrop-contrast",
        values: &[Integer, Arbitrary],
        alternative: "a backdrop-filter declaration",
    },
    ForeignFamily {
        root: "backdrop-saturate",
        values: &[Integer, Arbitrary],
        alternative: "a backdrop-filter declaration",
    },
    ForeignFamily {
        root: "backdrop-hue-rotate",
        values: &[Integer, Arbitrary],
        alternative: "a backdrop-filter declaration",
    },
    ForeignFamily {
        root: "backdrop-grayscale",
        values: &[Bare, Integer, Arbitrary],
        alternative: "a backdrop-filter declaration",
    },
    ForeignFamily {
        root: "backdrop-invert",
        values: &[Bare, Integer, Arbitrary],
        alternative: "a backdrop-filter declaration",
    },
    ForeignFamily {
        root: "backdrop-sepia",
        values: &[Bare, Integer, Arbitrary],
        alternative: "a backdrop-filter declaration",
    },
    ForeignFamily {
        root: "backdrop-opacity",
        values: &[Integer, Arbitrary],
        alternative: "a backdrop-filter declaration",
    },
    ForeignFamily {
        root: "text-shadow",
        values: &[
            Bare,
            Keyword("none"),
            Keyword("2xs"),
            Keyword("xs"),
            Keyword("sm"),
            Keyword("md"),
            Keyword("lg"),
            Arbitrary,
            Token(TokenCategory::Shadow),
        ],
        alternative: "a text-shadow declaration",
    },
    ForeignFamily {
        root: "ms",
        values: &[Any],
        alternative: "a margin-inline-start declaration",
    },
    ForeignFamily {
        root: "me",
        values: &[Any],
        alternative: "a margin-inline-end declaration",
    },
    ForeignFamily {
        root: "ps",
        values: &[Any],
        alternative: "a padding-inline-start declaration",
    },
    ForeignFamily {
        root: "pe",
        values: &[Any],
        alternative: "a padding-inline-end declaration",
    },
    ForeignFamily {
        root: "start",
        values: &[Any],
        alternative: "an inset-inline-start declaration",
    },
    ForeignFamily {
        root: "end",
        values: &[Any],
        alternative: "an inset-inline-end declaration",
    },
    ForeignFamily {
        root: "border-s",
        values: &[Bare, Any],
        alternative: "border-inline-start width, style or color declarations",
    },
    ForeignFamily {
        root: "border-e",
        values: &[Bare, Any],
        alternative: "border-inline-end width, style or color declarations",
    },
    ForeignFamily {
        root: "table",
        values: &[
            Bare,
            Keyword("auto"),
            Keyword("fixed"),
            Keyword("caption"),
            Keyword("cell"),
            Keyword("column"),
            Keyword("column-group"),
            Keyword("footer-group"),
            Keyword("header-group"),
            Keyword("row"),
            Keyword("row-group"),
        ],
        alternative: "a display or table-layout declaration",
    },
    ForeignFamily {
        root: "inline-table",
        values: &[Bare],
        alternative: DISPLAY,
    },
    ForeignFamily {
        root: "contents",
        values: &[Bare],
        alternative: DISPLAY,
    },
    ForeignFamily {
        root: "flow-root",
        values: &[Bare],
        alternative: DISPLAY,
    },
    ForeignFamily {
        root: "list-item",
        values: &[Bare],
        alternative: DISPLAY,
    },
    ForeignFamily {
        root: "container",
        values: &[Bare],
        alternative: "width and max-width declarations per breakpoint",
    },
    ForeignFamily {
        root: "not-sr-only",
        values: &[Bare],
        alternative: "declarations that undo the sr-only clipping",
    },
    ForeignFamily {
        root: "line-clamp",
        values: &[Any],
        alternative: "overflow, display: -webkit-box and -webkit-line-clamp declarations",
    },
    ForeignFamily {
        root: "order",
        values: &[
            Integer,
            Keyword("first"),
            Keyword("last"),
            Keyword("none"),
            Arbitrary,
        ],
        alternative: "an order declaration",
    },
    ForeignFamily {
        root: "basis",
        values: &[Any],
        alternative: "a flex-basis declaration",
    },
    ForeignFamily {
        root: "fill",
        values: &[Keyword("current"), Keyword("none"), Arbitrary],
        alternative: "a fill declaration",
    },
    ForeignFamily {
        root: "stroke",
        values: &[Keyword("current"), Keyword("none"), Integer, Arbitrary],
        alternative: "a stroke or stroke-width declaration",
    },
    ForeignFamily {
        root: "backdrop-blur",
        values: &[Bare, Any],
        alternative: "a backdrop-filter declaration",
    },
    ForeignFamily {
        root: "appearance",
        values: &[Keyword("none"), Keyword("auto")],
        alternative: "an appearance declaration",
    },
    ForeignFamily {
        root: "will-change",
        values: &[Any],
        alternative: "a will-change declaration",
    },
    ForeignFamily {
        root: "content",
        values: &[Keyword("none"), Arbitrary],
        alternative: "a content declaration on a ::before or ::after rule",
    },
];

/// The foreign family a parsed candidate's utility names, if any. Variants
/// and slash modifiers do not affect membership (`before:content-none`,
/// `basis-1/2`).
pub fn foreign_family(
    candidate: &Candidate,
    tokens: &ValidatedTokens,
) -> Option<&'static ForeignFamily> {
    let name = candidate.utility.named.as_str();
    FAMILIES.iter().find(|family| {
        if name == family.root {
            return family.values.contains(&Bare);
        }
        let Some(suffix) = name
            .strip_prefix(family.root)
            .and_then(|rest| rest.strip_prefix('-'))
        else {
            return false;
        };
        !suffix.is_empty()
            && family
                .values
                .iter()
                .any(|value| matches_value(*value, suffix, tokens))
    })
}

fn matches_value(value: ForeignValue, suffix: &str, tokens: &ValidatedTokens) -> bool {
    match value {
        Bare => false,
        Keyword(keyword) => suffix == keyword,
        Integer => suffix.bytes().all(|byte| byte.is_ascii_digit()),
        Arbitrary => suffix.starts_with('[') && suffix.ends_with(']') && suffix.len() > 2,
        Token(category) => tokens.contains(category, suffix),
        Any => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_candidate, VariantVocabulary};

    fn family(text: &str) -> Option<&'static str> {
        let vocabulary = VariantVocabulary::new(vec!["lg".to_owned()], false);
        foreign_family(
            &parse_candidate(text, &vocabulary).unwrap(),
            &crate::TokenConfig::default().validate().unwrap(),
        )
        .map(|family| family.root)
    }

    #[test]
    fn listed_migration_names_match_their_family() {
        for (text, root) in [
            ("bg-linear-to-r", "bg-linear"),
            ("bg-linear-45", "bg-linear"),
            ("-bg-linear-45", "bg-linear"),
            ("bg-linear-[135deg]", "bg-linear"),
            ("bg-gradient-to-b", "bg-gradient"),
            ("bg-radial", "bg-radial"),
            ("bg-radial-[at_25%_25%]", "bg-radial"),
            ("bg-conic", "bg-conic"),
            ("bg-conic-45", "bg-conic"),
            ("-bg-conic-45", "bg-conic"),
            ("bg-conic-[from_90deg]", "bg-conic"),
            ("table", "table"),
            ("table-row", "table"),
            ("contents", "contents"),
            ("flow-root", "flow-root"),
            ("line-clamp-2", "line-clamp"),
            ("order-1", "order"),
            ("basis-1/2", "basis"),
            ("fill-current", "fill"),
            ("stroke-current", "stroke"),
            ("backdrop-blur-sm", "backdrop-blur"),
            ("appearance-none", "appearance"),
            ("will-change-transform", "will-change"),
            ("not-sr-only", "not-sr-only"),
            ("content-[\"\"]", "content"),
            ("before:content-none", "content"),
            ("container", "container"),
            ("ms-auto", "ms"),
            ("-me-[3px]", "me"),
            ("ps-hsp-md", "ps"),
            ("pe-[3px]", "pe"),
            ("start-0", "start"),
            ("end-[3px]", "end"),
            ("border-s", "border-s"),
            ("border-s-2", "border-s"),
            ("border-s-[3px]", "border-s"),
            ("border-e-accent", "border-e"),
            ("border-e-[red]", "border-e"),
            ("isolate", "isolate"),
            ("isolation-auto", "isolation"),
            ("float-right", "float"),
            ("lg:float-start", "float"),
            ("clear-both", "clear"),
            ("break-keep", "break"),
            ("break-before-page", "break-before"),
            ("break-after-avoid", "break-after"),
            ("break-inside-avoid-column", "break-inside"),
            ("origin-left", "origin"),
            ("origin-[25%_75%]", "origin"),
            ("blur-sm", "blur"),
            ("brightness-0", "brightness"),
            ("contrast-125", "contrast"),
            ("saturate-150", "saturate"),
            ("-hue-rotate-90", "hue-rotate"),
            ("hue-rotate-[30deg]", "hue-rotate"),
            ("grayscale", "grayscale"),
            ("invert-0", "invert"),
            ("sepia", "sepia"),
            ("drop-shadow", "drop-shadow"),
            ("filter-none", "filter"),
            ("filter-[blur(2px)]", "filter"),
            ("backdrop-filter-none", "backdrop-filter"),
            ("backdrop-filter-[blur(2px)]", "backdrop-filter"),
            ("backdrop-blur-sm", "backdrop-blur"),
            ("backdrop-brightness-50", "backdrop-brightness"),
            ("backdrop-grayscale", "backdrop-grayscale"),
            ("backdrop-opacity-70", "backdrop-opacity"),
            ("text-shadow-none", "text-shadow"),
            ("text-shadow-md", "text-shadow"),
        ] {
            assert_eq!(family(text), Some(root), "{text}");
        }
    }

    #[test]
    fn plausible_authored_names_stay_outside_the_vocabulary() {
        for text in [
            "table-wrapper",
            "order-summary",
            "content-area",
            "container-inner",
            "fill-panel",
            "appearance-card",
            "contents-list",
            "ms",
            "me",
            "ps",
            "pe",
            "start",
            "end",
            "border-started",
            "origin-story",
            "float-label",
            "clear-fix",
            "to-do",
            "from-top",
            "brightness-panel",
            "hue-rotate-wheel",
            "backdrop-card",
            "text-shadow-title",
        ] {
            assert_eq!(family(text), None, "{text}");
        }
    }

    #[test]
    fn token_values_are_exact_and_category_scoped() {
        let mut config = crate::TokenConfig::default();
        config.colors.insert("zd-black".into(), "#000".into());
        config
            .shadows
            .insert("editorial".into(), "0 1px 2px #000".into());
        let tokens = config.validate().unwrap();
        let vocabulary = VariantVocabulary::new(Vec::<String>::new(), false);
        let parsed = parse_candidate("text-shadow-editorial", &vocabulary).unwrap();
        assert_eq!(
            foreign_family(&parsed, &tokens).map(|family| family.root),
            Some("text-shadow")
        );
        let parsed = parse_candidate("text-shadow-zd-black", &vocabulary).unwrap();
        assert!(foreign_family(&parsed, &tokens).is_none());
        assert!(matches_value(
            Token(TokenCategory::Color),
            "zd-black",
            &tokens
        ));
        assert!(!matches_value(
            Token(TokenCategory::Color),
            "black",
            &tokens
        ));
    }

    #[test]
    fn gradient_stops_require_color_tokens_or_supported_value_forms() {
        let mut config = crate::TokenConfig::default();
        config.colors.insert("zd-black".into(), "#000".into());
        config
            .shadows
            .insert("shadow-only".into(), "0 1px 2px #000".into());
        let tokens = config.validate().unwrap();
        let vocabulary = VariantVocabulary::default();

        for (text, root) in [
            ("from-zd-black", "from"),
            ("via-zd-black", "via"),
            ("to-zd-black", "to"),
            ("from-zd-black/70", "from"),
            ("to-transparent", "to"),
            ("via-current", "via"),
            ("from-[#8B1E1E]", "from"),
            ("to-[60%]", "to"),
        ] {
            let candidate = parse_candidate(text, &vocabulary).unwrap();
            assert_eq!(
                foreign_family(&candidate, &tokens).map(|family| family.root),
                Some(root),
                "{text}"
            );
        }

        for text in [
            "from-missing-color",
            "from-shadow-only",
            "bg-gradient-45",
            "to-do",
            "from-top",
            "via-route",
        ] {
            let candidate = parse_candidate(text, &vocabulary).unwrap();
            assert_eq!(foreign_family(&candidate, &tokens), None, "{text}");
        }
    }
}
