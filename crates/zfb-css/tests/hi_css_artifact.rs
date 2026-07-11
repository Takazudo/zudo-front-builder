//! Textual acceptance tests for the shipped `assets/zfb-hi.css` artifact,
//! exercised through the public `zfb_css::default_hi_css()` accessor.
//!
//! These parse the artifact with naive line-based matching, which assumes the
//! artifact keeps **one declaration per line** and never wraps a value across
//! lines. That holds for this small hand-authored file; if it is ever run
//! through a CSS formatter that reflows declarations, switch to a real parser
//! (`lightningcss`, already a crate dependency) instead.

use zfb_css::default_hi_css;

/// The fixed 18-role taxonomy from issue #1529 — do not add/remove entries
/// without also updating the classifier + this artifact.
const ROLE_SUFFIXES: &[&str] = &[
    "esc", "op", "com", "str", "num", "const", "kw", "fn", "ty", "ns", "prop", "var", "tag",
    "attr", "punct", "ins", "del", "hd",
];

/// Roles that carry no accent rule in base16-ocean and therefore alias
/// `var(--zfb-hi-fg)` rather than a literal color pair (see the artifact's
/// header comment). They intentionally have no `@media` dark override.
const ALIASED_TO_FG: &[&str] = &["op", "ns", "punct"];

/// Split the artifact into the base section (before the dark media query) and
/// the `@media (prefers-color-scheme: dark)` section.
fn split_sections(css: &str) -> (&str, &str) {
    css.split_once("@media (prefers-color-scheme: dark)")
        .expect("artifact must contain a prefers-color-scheme: dark media block")
}

/// Count lines (trimmed) that start with `prefix`.
fn count_lines_starting_with(section: &str, prefix: &str) -> usize {
    section
        .lines()
        .filter(|line| line.trim_start().starts_with(prefix))
        .count()
}

/// Parse every `--zfb-hi-<name>: <value>;` declaration line in `section` into
/// `(name, value)` pairs (both trimmed, trailing `;` stripped from the value).
fn declared_custom_properties(section: &str) -> Vec<(String, String)> {
    section
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("--zfb-hi-") {
                return None;
            }
            let (name, value) = trimmed.split_once(':')?;
            let value = value.trim().trim_end_matches(';').trim();
            Some((name.trim().to_string(), value.to_string()))
        })
        .collect()
}

/// The dark argument of a `light-dark(<light>, <dark>)` value, or `None` for
/// any other value shape (e.g. a `var(...)` alias). Relies on neither argument
/// containing a top-level comma — hex has none and the artifact's `rgb()`
/// tints use space-separated syntax.
fn light_dark_dark_arg(value: &str) -> Option<&str> {
    let inner = value.strip_prefix("light-dark(")?.strip_suffix(')')?;
    let (_light, dark) = inner.split_once(',')?;
    Some(dark.trim())
}

#[test]
fn artifact_is_at_most_6kb() {
    let css = default_hi_css();
    assert!(
        css.len() <= 6 * 1024,
        "zfb-hi.css must stay <= 6 KB, got {} bytes",
        css.len()
    );
}

#[test]
fn wrapped_in_zfb_hi_layer() {
    let css = default_hi_css();
    assert!(
        css.contains("@layer zfb-hi {"),
        "artifact must be wrapped in `@layer zfb-hi`"
    );
}

#[test]
fn does_not_set_color_scheme() {
    let css = default_hi_css();
    // Line-anchored so this doesn't false-positive on the
    // `@media (prefers-color-scheme: dark)` feature query, which also contains
    // the substring `color-scheme:` but is not a `color-scheme` declaration.
    assert!(
        !css.lines()
            .any(|line| line.trim_start().starts_with("color-scheme:")),
        "the artifact must not set `color-scheme` itself — the host page owns it"
    );
}

#[test]
fn each_role_suffix_has_one_custom_property_and_one_class_rule() {
    let css = default_hi_css();
    let (base_section, _dark_section) = split_sections(css);
    for suffix in ROLE_SUFFIXES {
        let prop_prefix = format!("--zfb-hi-{suffix}:");
        let prop_count = count_lines_starting_with(base_section, &prop_prefix);
        assert_eq!(
            prop_count, 1,
            "expected exactly one `{prop_prefix}` declaration in :root, found {prop_count}"
        );

        let class_prefix = format!(".hi-{suffix} {{");
        let class_count = count_lines_starting_with(css, &class_prefix);
        assert_eq!(
            class_count, 1,
            "expected exactly one `.hi-{suffix}` rule, found {class_count}"
        );
    }
}

#[test]
fn fg_bg_have_one_custom_property_and_root_has_one_class_rule() {
    let css = default_hi_css();
    let (base_section, _dark_section) = split_sections(css);
    for suffix in ["fg", "bg"] {
        let prop_prefix = format!("--zfb-hi-{suffix}:");
        let prop_count = count_lines_starting_with(base_section, &prop_prefix);
        assert_eq!(
            prop_count, 1,
            "expected exactly one `{prop_prefix}` declaration in :root, found {prop_count}"
        );
    }
    let root_count = count_lines_starting_with(css, ".hi-root {");
    assert_eq!(
        root_count, 1,
        "expected exactly one `.hi-root` rule, found {root_count}"
    );
}

#[test]
fn aliased_roles_reference_the_fg_property() {
    let css = default_hi_css();
    let (base_section, _dark_section) = split_sections(css);
    let base_props = declared_custom_properties(base_section);
    for suffix in ALIASED_TO_FG {
        let name = format!("--zfb-hi-{suffix}");
        let value = base_props
            .iter()
            .find(|(n, _)| n == &name)
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| panic!("missing `{name}` custom property in :root"));
        assert_eq!(
            value, "var(--zfb-hi-fg)",
            "`{name}` must alias `var(--zfb-hi-fg)` (no accent in base16-ocean), got `{value}`"
        );
    }
}

#[test]
fn dark_media_block_redeclares_every_literal_valued_custom_property() {
    let css = default_hi_css();
    let (base_section, dark_section) = split_sections(css);
    let base_props = declared_custom_properties(base_section);

    // Only literal (`light-dark(...)`) properties carry a dark value that needs
    // a media-block override; `var()`-aliased roles re-resolve through fg and
    // must NOT appear in the dark block.
    let literal_names: Vec<&String> = base_props
        .iter()
        .filter(|(_, v)| !v.contains("var("))
        .map(|(n, _)| n)
        .collect();
    assert!(
        !literal_names.is_empty(),
        "expected at least one literal-valued custom property in :root"
    );

    let dark_names: Vec<String> = declared_custom_properties(dark_section)
        .into_iter()
        .map(|(n, _)| n)
        .collect();

    for name in &literal_names {
        assert!(
            dark_names.contains(name),
            "dark media block does not redeclare literal-valued `{name}`"
        );
    }
    for name in &dark_names {
        assert!(
            literal_names.contains(&name),
            "dark media block declares `{name}`, which is not a literal-valued :root property \
             (aliased roles must not be redeclared)"
        );
    }
}

#[test]
fn base_light_dark_dark_half_matches_media_override() {
    let css = default_hi_css();
    let (base_section, dark_section) = split_sections(css);
    let dark_props = declared_custom_properties(dark_section);

    let mut checked = 0;
    for (name, value) in declared_custom_properties(base_section) {
        let Some(dark_arg) = light_dark_dark_arg(&value) else {
            continue; // var()-aliased or non-light-dark value
        };
        let override_value = dark_props
            .iter()
            .find(|(n, _)| n == &name)
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| {
                panic!("`{name}` has a light-dark() base value but no dark media override")
            });
        assert_eq!(
            dark_arg, override_value,
            "`{name}`: light-dark() dark half `{dark_arg}` disagrees with media override \
             `{override_value}` — the two hand-maintained copies drifted"
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "expected at least one light-dark() property to cross-check"
    );
}

#[test]
fn component_rules_reference_only_custom_properties_no_raw_hex() {
    let css = default_hi_css();
    // The component (`.hi-*`) rules live from `.hi-root` to the end of the
    // file. `#` is used exclusively for hex colors in this artifact (no id
    // selectors), so its absence here proves no raw hex leaked into a
    // component rule.
    let root_idx = css
        .find(".hi-root {")
        .expect("artifact must contain a .hi-root rule");
    let component_region = &css[root_idx..];
    assert!(
        !component_region.contains('#'),
        "component rules must reference only var(--zfb-hi-...), no raw hex colors"
    );

    let color_or_bg_lines = component_region
        .lines()
        .filter(|line| {
            let t = line.trim_start();
            t.starts_with("color:") || t.starts_with("background:")
        })
        .count();
    let var_ref_lines = component_region
        .lines()
        .filter(|line| {
            let t = line.trim_start();
            (t.starts_with("color:") || t.starts_with("background:")) && t.contains("var(--zfb-hi-")
        })
        .count();
    assert!(
        color_or_bg_lines > 0,
        "expected at least one color/background declaration in component rules"
    );
    assert_eq!(
        color_or_bg_lines, var_ref_lines,
        "every component color/background declaration must reference var(--zfb-hi-...)"
    );
}
