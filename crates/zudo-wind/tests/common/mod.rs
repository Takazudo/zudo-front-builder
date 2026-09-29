use std::collections::BTreeMap;
use zudo_wind::{
    BreakpointConfig, CompileInput, DarkModeConfig, Origin, OriginCandidate, TokenConfig,
    WindConfig,
};

pub fn config() -> WindConfig {
    WindConfig {
        tokens: TokenConfig {
            spacing_unit: Some("0.25rem".to_owned()),
            colors: BTreeMap::from([("panel".to_owned(), "#123456".to_owned())]),
            ..TokenConfig::default()
        },
        breakpoints: BTreeMap::from([
            ("sm".to_owned(), BreakpointConfig { min_width_px: 640 }),
            ("2xl".to_owned(), BreakpointConfig { min_width_px: 1280 }),
        ]),
        dark: Some(DarkModeConfig {
            attribute: "data-theme".to_owned(),
            value: "dark".to_owned(),
        }),
        ..WindConfig::default()
    }
}

pub fn input(names: &[&str]) -> CompileInput {
    CompileInput {
        config: config(),
        candidates: names
            .iter()
            .enumerate()
            .map(|(index, text)| OriginCandidate {
                text: (*text).to_owned(),
                origin: Origin::Safelist {
                    owner: "fixture".to_owned(),
                    index,
                },
            })
            .collect(),
    }
}
