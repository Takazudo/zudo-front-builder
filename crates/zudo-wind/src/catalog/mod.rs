//! Versioned, deterministic utility vocabulary.

mod arbitrary;
mod families;
mod resolve;

use std::collections::BTreeSet;

use crate::TokenCategory;
pub use resolve::{Declaration, Resolution, ResolvedRule};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectorShape {
    OwnElement,
    LaterVisibleSiblings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    Exact,
    Keyword,
    Token,
    Scale,
    Fraction,
    Arbitrary,
    Integer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValueGrammar {
    pub accepted_kinds: Vec<ValueKind>,
    pub keywords: Vec<(&'static str, &'static str)>,
    pub token_categories: Vec<TokenCategory>,
    pub arbitrary_property: Option<&'static str>,
    pub allows_fraction_slash: bool,
}

impl ValueGrammar {
    pub(crate) fn new(
        kinds: &[ValueKind],
        keywords: &[(&'static str, &'static str)],
        categories: &[TokenCategory],
        arbitrary: Option<&'static str>,
        fraction: bool,
    ) -> Self {
        Self {
            accepted_kinds: kinds.to_vec(),
            keywords: keywords.to_vec(),
            token_categories: categories.to_vec(),
            arbitrary_property: arbitrary,
            allows_fraction_slash: fraction,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Example {
    pub candidate: String,
    pub declarations: Vec<Declaration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    pub id: String,
    pub root: String,
    pub grammar: ValueGrammar,
    /// Properties in declaration order; dynamic values are supplied by the resolver.
    pub emitter: Vec<&'static str>,
    /// Fixed value for an exact spelling; separate from documentation examples.
    pub fixed_value: Option<String>,
    pub negative: bool,
    pub conflict_group: &'static str,
    pub conflict_group_rank: u16,
    pub order_rank: u16,
    pub selector_shape: SelectorShape,
    pub examples: Vec<Example>,
    pub introduced_in: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogError {
    DuplicateIdentifier(String),
    DuplicateRootKind { root: String, kind: ValueKind },
}

#[derive(Clone, Debug)]
pub struct Catalog {
    entries: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn new(entries: Vec<CatalogEntry>) -> Result<Self, CatalogError> {
        let mut ids = BTreeSet::new();
        let mut claims = BTreeSet::new();
        for entry in &entries {
            if !ids.insert(entry.id.clone()) {
                return Err(CatalogError::DuplicateIdentifier(entry.id.clone()));
            }
            for kind in &entry.grammar.accepted_kinds {
                let qualifications: Vec<String> = match kind {
                    ValueKind::Token => entry
                        .grammar
                        .token_categories
                        .iter()
                        .map(|category| format!("{category:?}"))
                        .collect(),
                    ValueKind::Arbitrary => entry
                        .grammar
                        .arbitrary_property
                        .map(|property| vec![property.to_owned()])
                        .unwrap_or_default(),
                    _ => vec![String::new()],
                };
                for qualification in qualifications {
                    if !claims.insert((entry.root.clone(), *kind as u8, qualification)) {
                        return Err(CatalogError::DuplicateRootKind {
                            root: entry.root.clone(),
                            kind: *kind,
                        });
                    }
                }
            }
        }
        let mut entries = entries;
        entries.sort_by(|a, b| {
            a.conflict_group_rank
                .cmp(&b.conflict_group_rank)
                .then(a.order_rank.cmp(&b.order_rank))
                .then(a.root.cmp(&b.root))
                .then(a.id.cmp(&b.id))
        });
        Ok(Self { entries })
    }

    pub fn v1() -> Self {
        Self::new(families::batch_a()).expect("batch A catalog must have unique claims")
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_catalog_is_valid_and_ordered() {
        let catalog = Catalog::v1();
        assert!(!catalog.entries().is_empty());
        assert!(catalog
            .entries()
            .windows(2)
            .all(|w| w[0].conflict_group_rank <= w[1].conflict_group_rank));
        let ranks: BTreeSet<_> = catalog
            .entries()
            .iter()
            .map(|entry| entry.conflict_group_rank)
            .collect();
        assert_eq!(ranks, (1..=14).collect::<BTreeSet<u16>>());
    }
    #[test]
    fn duplicate_identifier_is_rejected() {
        let entry = Catalog::v1().entries()[0].clone();
        assert!(matches!(
            Catalog::new(vec![entry.clone(), entry]),
            Err(CatalogError::DuplicateIdentifier(_))
        ));
    }
    #[test]
    fn duplicate_root_and_value_kind_is_rejected() {
        let entry = Catalog::v1().entries()[0].clone();
        let mut second = entry.clone();
        second.id.push_str(".duplicate");
        assert!(matches!(
            Catalog::new(vec![entry, second]),
            Err(CatalogError::DuplicateRootKind { .. })
        ));
    }
}
