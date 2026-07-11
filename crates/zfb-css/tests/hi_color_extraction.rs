//! One-time color-extraction helper for `crates/zfb-css/assets/zfb-hi.css`.
//!
//! Resolves each of the 18 `zfb-hi` role suffixes' representative syntect
//! scope path through the bundled `base16-ocean.light` / `base16-ocean.dark`
//! themes (syntect's own `ThemeSet::load_defaults()` + `Highlighter`) and
//! prints the resolved light/dark hex pairs plus `fg`/`bg`. Regen command:
//!
//! ```text
//! cargo test -p zfb-css --test hi_color_extraction -- --ignored --nocapture
//! ```
//!
//! The printed pairs are pasted by hand into `assets/zfb-hi.css` — this test
//! does not write the artifact itself (see issue #1531). Scope paths mirror
//! the "Primary scope prefixes" column of issue #1529's 18-role taxonomy
//! table, hand-tuned against `theme.scopes` (each rule's actual selectors)
//! so every representative scope resolves through a real theme rule rather
//! than silently falling back to the plain-text default. Three roles (`op`,
//! `ns`, `punct`) genuinely have no accent rule in base16-ocean — that is
//! the theme's own design (operators/namespaces/punctuation inherit plain
//! text color), confirmed by dumping `theme.scopes` directly, not an
//! extraction artifact.
use syntect::highlighting::{Color, Highlighter, ThemeSet};
use syntect::parsing::ScopeStack;

/// One row per `zfb-hi` role suffix: the CSS custom-property suffix plus a
/// representative syntect scope path (outermost -> innermost) that a real
/// tokenizer would push for a typical token of that role.
const ROLE_SCOPES: &[(&str, &str)] = &[
    (
        "esc",
        "source.rust string.quoted.double.rust constant.character.escape.rust",
    ),
    ("op", "source.rust keyword.operator.rust"),
    ("com", "source.rust comment.line.double-slash.rust"),
    ("str", "source.rust string.quoted.double.rust"),
    ("num", "source.rust constant.numeric.rust"),
    ("const", "source.rust constant.language.rust"),
    ("kw", "source.rust keyword.control.rust"),
    (
        "fn",
        "source.rust meta.function.rust entity.name.function.rust",
    ),
    ("ty", "source.rust entity.name.class.rust"),
    ("ns", "source.rust entity.name.namespace.rust"),
    ("prop", "source.rust variable.other.member.rust"),
    ("var", "source.rust variable.other.rust"),
    ("tag", "text.html.basic entity.name.tag.html"),
    ("attr", "text.html.basic entity.other.attribute-name.html"),
    ("punct", "source.rust punctuation.separator.rust"),
    ("ins", "source.diff markup.inserted.diff"),
    ("del", "source.diff markup.deleted.diff"),
    ("hd", "text.html.markdown entity.name.section.markdown"),
];

fn hex(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

#[test]
#[ignore = "verification: one-time extraction helper for assets/zfb-hi.css; run with `cargo test -p zfb-css --test hi_color_extraction -- --ignored --nocapture` and paste the printed pairs by hand"]
fn extract_zfb_hi_colors_from_base16_ocean() {
    let theme_set = ThemeSet::load_defaults();
    let light = theme_set
        .themes
        .get("base16-ocean.light")
        .expect("bundled base16-ocean.light theme present");
    let dark = theme_set
        .themes
        .get("base16-ocean.dark")
        .expect("bundled base16-ocean.dark theme present");
    let hl_light = Highlighter::new(light);
    let hl_dark = Highlighter::new(dark);

    println!("--- zfb-hi.css color extraction (base16-ocean.light / base16-ocean.dark) ---");

    let fg_light = hex(hl_light.get_default().foreground);
    let fg_dark = hex(hl_dark.get_default().foreground);
    println!("fg: light={fg_light} dark={fg_dark}");

    let bg_light = light
        .settings
        .background
        .map(hex)
        .unwrap_or_else(|| "none".to_string());
    let bg_dark = dark
        .settings
        .background
        .map(hex)
        .unwrap_or_else(|| "none".to_string());
    println!("bg: light={bg_light} dark={bg_dark}");

    for (suffix, scope_str) in ROLE_SCOPES {
        let stack: ScopeStack = scope_str.parse().expect("valid scope path");
        let light_style = hl_light.style_for_stack(&stack.scopes);
        let dark_style = hl_dark.style_for_stack(&stack.scopes);
        println!(
            "{suffix}: light={} dark={}",
            hex(light_style.foreground),
            hex(dark_style.foreground)
        );
    }
}
