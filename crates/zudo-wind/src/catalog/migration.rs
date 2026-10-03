//! Bounded migration vocabulary: Tailwind v4 utility names that zudo-wind v1
//! deliberately does not implement. A match is reported, never generated, so
//! a migrating project learns which classes silently lost their CSS.

use crate::Candidate;

/// Bump when an entry is added or removed; diagnostics name the version so a
/// report stays interpretable after the vocabulary changes.
pub const MIGRATION_VOCABULARY_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ForeignValue {
    /// The root alone, such as `container`.
    Bare,
    Keyword(&'static str),
    Integer,
    /// A bracketed value, such as `content-[""]`.
    Arbitrary,
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

use ForeignValue::{Any, Arbitrary, Bare, Integer, Keyword};

const DISPLAY: &str = "a display declaration";

const FAMILIES: &[ForeignFamily] = &[
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
pub fn foreign_family(candidate: &Candidate) -> Option<&'static ForeignFamily> {
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
            && family.values.iter().any(|value| match value {
                Bare => false,
                Keyword(keyword) => suffix == *keyword,
                Integer => suffix.bytes().all(|byte| byte.is_ascii_digit()),
                Arbitrary => suffix.starts_with('[') && suffix.ends_with(']') && suffix.len() > 2,
                Any => true,
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_candidate, VariantVocabulary};

    fn family(text: &str) -> Option<&'static str> {
        let vocabulary = VariantVocabulary::new(Vec::<String>::new(), false);
        foreign_family(&parse_candidate(text, &vocabulary).unwrap()).map(|family| family.root)
    }

    #[test]
    fn listed_migration_names_match_their_family() {
        for (text, root) in [
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
        ] {
            assert_eq!(family(text), None, "{text}");
        }
    }
}
