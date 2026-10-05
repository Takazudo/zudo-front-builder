use std::collections::BTreeSet;

use crate::{Diagnostic, DiagnosticCode};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VariantKind {
    Breakpoint { name: String, max: bool },
    Dark,
    Pointer { fine: bool },
    Relation { peer: bool, state: String },
    State(String),
    PseudoElement(String),
}

impl VariantKind {
    fn rank(&self) -> u8 {
        match self {
            Self::Breakpoint { .. } => 0,
            Self::Dark => 1,
            Self::Pointer { .. } => 2,
            Self::Relation { .. } => 3,
            Self::State(_) => 4,
            Self::PseudoElement(_) => 5,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Variant {
    pub raw: String,
    pub kind: VariantKind,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VariantChain(pub Vec<Variant>);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VariantVocabulary {
    pub breakpoints: BTreeSet<String>,
    pub dark: bool,
}

impl VariantVocabulary {
    pub fn new(breakpoints: impl IntoIterator<Item = String>, dark: bool) -> Self {
        Self {
            breakpoints: breakpoints.into_iter().collect(),
            dark,
        }
    }

    fn classify(&self, raw: &str) -> Option<VariantKind> {
        if raw == "dark" && self.dark {
            return Some(VariantKind::Dark);
        }
        if raw == "pointer-coarse" || raw == "pointer-fine" {
            return Some(VariantKind::Pointer {
                fine: raw == "pointer-fine",
            });
        }
        if let Some(name) = raw.strip_prefix("max-") {
            if self.breakpoints.contains(name) {
                return Some(VariantKind::Breakpoint {
                    name: name.to_owned(),
                    max: true,
                });
            }
        }
        if self.breakpoints.contains(raw) {
            return Some(VariantKind::Breakpoint {
                name: raw.to_owned(),
                max: false,
            });
        }
        if let Some(state) = raw.strip_prefix("group-") {
            if is_state(state) {
                return Some(VariantKind::Relation {
                    peer: false,
                    state: state.to_owned(),
                });
            }
        }
        if let Some(state) = raw.strip_prefix("peer-") {
            if is_state(state) {
                return Some(VariantKind::Relation {
                    peer: true,
                    state: state.to_owned(),
                });
            }
        }
        if is_state(raw) {
            return Some(VariantKind::State(raw.to_owned()));
        }
        if [
            "before",
            "after",
            "marker",
            "placeholder",
            "backdrop",
            "selection",
        ]
        .contains(&raw)
        {
            return Some(VariantKind::PseudoElement(raw.to_owned()));
        }
        None
    }

    pub(crate) fn parse(
        &self,
        text: &str,
        raw: &[&str],
        utility: &str,
    ) -> Result<VariantChain, Diagnostic> {
        let mut chain = Vec::with_capacity(raw.len());
        for name in raw {
            let kind = self.classify(name).ok_or_else(|| {
                Diagnostic::parse(
                    text,
                    DiagnosticCode::Zw002,
                    "R04",
                    "unknown or unconfigured variant",
                )
            })?;
            chain.push(Variant {
                raw: (*name).to_owned(),
                kind,
            });
        }
        let mut seen = BTreeSet::new();
        if chain
            .iter()
            .any(|variant| !seen.insert(variant.kind.rank()))
        {
            return Err(Diagnostic::parse(
                text,
                DiagnosticCode::Zw003,
                "R05",
                "duplicate variant class",
            ));
        }
        if chain
            .windows(2)
            .any(|pair| pair[0].kind.rank() > pair[1].kind.rank())
        {
            let mut sorted = chain.clone();
            sorted.sort_by_key(|variant| variant.kind.rank());
            let mut diagnostic = Diagnostic::parse(
                text,
                DiagnosticCode::Zw003,
                "R06",
                "noncanonical variant order",
            );
            diagnostic.suggested_spelling = Some(format!(
                "{}:{utility}",
                sorted
                    .iter()
                    .map(|variant| variant.raw.as_str())
                    .collect::<Vec<_>>()
                    .join(":")
            ));
            return Err(diagnostic);
        }
        Ok(VariantChain(chain))
    }
}

pub(crate) fn is_state(state: &str) -> bool {
    [
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
    ]
    .contains(&state)
}
