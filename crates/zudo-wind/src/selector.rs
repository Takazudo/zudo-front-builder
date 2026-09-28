use crate::{
    escape::quoted_string, escape_class_name, Candidate, SelectorShape, ValidatedWindConfig,
    VariantKind,
};

/// Actual CSS specificity: dark/relation tests add zero; self states add one
/// class, pseudo-elements add one type, and child filters add two classes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Specificity {
    pub ids: u8,
    pub classes: u8,
    pub types: u8,
}

pub(crate) struct Selector {
    pub text: String,
    /// Media conditions in canonical order, combined with `and` on emission.
    pub conditions: Vec<String>,
    pub specificity: Specificity,
}

fn state_selector(state: &str) -> String {
    match state {
        "first" => ":first-child".to_owned(),
        "last" => ":last-child".to_owned(),
        "open" => ":is([open], :popover-open)".to_owned(),
        _ => format!(":{state}"),
    }
}

pub(crate) fn build(
    candidate: &Candidate,
    shape: SelectorShape,
    config: &ValidatedWindConfig,
) -> Selector {
    let mut text = format!(".{}", escape_class_name(&candidate.raw));
    let mut conditions = Vec::new();
    let mut hover = false;
    let mut specificity = Specificity {
        classes: 1,
        ..Specificity::default()
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
                conditions.push(if *max {
                    format!("(width < {}px)", breakpoint.min_width_px)
                } else {
                    format!("(min-width: {}px)", breakpoint.min_width_px)
                });
            }
            VariantKind::Dark => {
                let dark = config.dark.as_ref().expect("configured dark variant");
                let test = format!("[{}={}]", dark.attribute, quoted_string(&dark.value));
                text.push_str(&format!(":where({test}, {test} *)"));
            }
            VariantKind::Relation { peer, state } => {
                let (marker, combinator) = if *peer {
                    ("peer", " ~ ")
                } else {
                    ("group", " ")
                };
                text = format!(
                    ":where(.{marker}{}){combinator}{text}",
                    state_selector(state)
                );
                hover |= state == "hover";
            }
            VariantKind::State(state) => {
                text.push_str(&state_selector(state));
                hover |= state == "hover";
                specificity.classes += 1;
            }
            VariantKind::PseudoElement(pseudo) => {
                text.push_str(&format!("::{pseudo}"));
                specificity.types += 1;
            }
        }
    }
    if hover {
        conditions.push("(hover: hover)".to_owned());
    }
    if shape == SelectorShape::LaterVisibleSiblings {
        text.push_str(" > :not([hidden]) ~ :not([hidden])");
        specificity.classes += 2;
    }
    Selector {
        text,
        conditions,
        specificity,
    }
}
