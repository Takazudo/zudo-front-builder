use crate::{Candidate, Catalog, ResolvedRule, ValidatedWindConfig, VariantKind};

/// The nine ascending fields fixed by spec 1, revision 2. String comparison is
/// UTF-8 byte order. Scope and catalog order never override CSS specificity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SortKey {
    pub responsive_rank: usize,
    pub dark_rank: u8,
    pub relation_rank: u8,
    pub state_rank: u8,
    pub pseudo_rank: u8,
    pub conflict_group_rank: u16,
    pub scope_rank: u16,
    pub catalog_rank: usize,
    pub raw_candidate: String,
}

pub(crate) fn state_rank(state: &str) -> u8 {
    match state {
        "first" => 1,
        "last" => 2,
        "open" => 3,
        "focus-within" => 4,
        "hover" => 5,
        "focus" => 6,
        "focus-visible" => 7,
        "active" => 8,
        "disabled" => 9,
        _ => unreachable!("validated state"),
    }
}

pub(crate) fn sort_key(
    candidate: &Candidate,
    rule: &ResolvedRule,
    config: &ValidatedWindConfig,
    catalog: &Catalog,
) -> SortKey {
    let scope_rank = match rule.conflict_group {
        "inset" | "sizing" | "padding" | "margin" | "gap" | "overflow" | "border-width"
        | "border-color" | "radius" | "scroll-margin" => rule.order_rank,
        _ => 0,
    };
    // The catalog orders subentries by their listed rank, then root bytes.
    // Use its dense within-group position so the tuple has exactly nine fields.
    let catalog_rank = catalog
        .entries()
        .iter()
        .filter(|entry| entry.conflict_group_rank == rule.conflict_group_rank)
        .position(|entry| entry.id == rule.entry_id)
        .expect("resolved catalog entry");
    let mut key = SortKey {
        responsive_rank: 0,
        dark_rank: 0,
        relation_rank: 0,
        state_rank: 0,
        pseudo_rank: 0,
        conflict_group_rank: rule.conflict_group_rank,
        scope_rank,
        catalog_rank,
        raw_candidate: candidate.raw.clone(),
    };
    for variant in &candidate.variants.0 {
        match &variant.kind {
            VariantKind::Breakpoint { name, max } => {
                let breakpoint = config
                    .breakpoints
                    .ranked()
                    .iter()
                    .find(|entry| &entry.name == name)
                    .expect("configured breakpoint");
                key.responsive_rank = 2 * breakpoint.rank + if *max { 2 } else { 1 };
            }
            VariantKind::Dark => key.dark_rank = 1,
            VariantKind::Relation { peer, state } => {
                key.relation_rank = state_rank(state) + if *peer { 9 } else { 0 }
            }
            VariantKind::State(state) => key.state_rank = state_rank(state),
            VariantKind::PseudoElement(pseudo) => {
                key.pseudo_rank = match pseudo.as_str() {
                    "before" => 1,
                    "after" => 2,
                    "marker" => 3,
                    "placeholder" => 4,
                    "backdrop" => 5,
                    _ => unreachable!("validated pseudo-element"),
                }
            }
        }
    }
    key
}
