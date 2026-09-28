use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;

use crate::DecimalDimension;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueStatus {
    Verified,
    CategoryUnverified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ValueCategory {
    Color,
    SpacingUnit,
    Spacing,
    Size,
    FontSize,
    LineHeight,
    FontFamily,
    FontWeight,
    LetterSpacing,
    Radius,
    Shadow,
    ZIndex,
    Easing,
}

impl ValueCategory {
    fn property_name(self) -> &'static str {
        match self {
            Self::Color => "color",
            Self::SpacingUnit => "width",
            Self::Spacing => "padding",
            Self::Size => "width",
            Self::FontSize => "font-size",
            Self::LineHeight => "line-height",
            Self::FontFamily => "font-family",
            Self::FontWeight => "font-weight",
            Self::LetterSpacing => "letter-spacing",
            Self::Radius => "border-radius",
            Self::Shadow => "box-shadow",
            Self::ZIndex => "z-index",
            Self::Easing => "transition-timing-function",
        }
    }
}

pub(crate) fn validate_value(category: ValueCategory, value: &str) -> Result<ValueStatus, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("value must not be empty".to_owned());
    }

    let functions = scan_css_value(value)?;
    if functions.iter().any(|function| function.name == "url") {
        return Err("url() is forbidden in token values".to_owned());
    }
    if is_css_wide_keyword(trimmed) {
        return Err("CSS-wide keywords are not token values".to_owned());
    }
    let has_var = functions.iter().any(|function| function.name == "var");
    for function in functions.iter().filter(|function| function.name == "var") {
        if variable_references_reserved_namespace(&function.arguments) {
            return Err("token values cannot reference the reserved --zw- namespace".to_owned());
        }
    }

    if category == ValueCategory::SpacingUnit {
        if has_var {
            return Err("spacingUnit must be a literal dimension or zero".to_owned());
        }
        let dimension = DecimalDimension::parse(trimmed)
            .map_err(|_| "spacingUnit must be one nonnegative dimension or zero".to_owned())?;
        if dimension.magnitude.is_negative() || dimension.unit == "%" {
            return Err("spacingUnit must be a nonnegative non-percentage dimension".to_owned());
        }
    }
    if matches!(category, ValueCategory::Spacing | ValueCategory::Radius) && !has_var {
        let dimension = DecimalDimension::parse(trimmed).map_err(|_| {
            if category == ValueCategory::Radius {
                "radii must be one nonnegative length-percentage".to_owned()
            } else {
                "spacing values must be one nonnegative length-percentage".to_owned()
            }
        })?;
        if dimension.magnitude.is_negative() {
            return Err("length token values must be nonnegative".to_owned());
        }
        if category == ValueCategory::Radius
            && dimension.unit.is_empty()
            && !dimension.magnitude.is_zero()
        {
            return Err("radii must be a length-percentage".to_owned());
        }
    }
    if category == ValueCategory::ZIndex && !has_var && !is_integer(trimmed) {
        return Err("z-index token values must be integers".to_owned());
    }
    if category == ValueCategory::Easing && has_top_level_comma(value) {
        return Err("easing token values must contain one timing function".to_owned());
    }

    let parsed = Property::parse_string(
        PropertyId::from(category.property_name()),
        value,
        ParserOptions::default(),
    )
    .map_err(|_| format!("value is not valid for {}", category.property_name()))?;

    match parsed {
        Property::Unparsed(_) if has_var => Ok(ValueStatus::CategoryUnverified),
        Property::Unparsed(_) => Err(format!(
            "value is not valid for {}",
            category.property_name()
        )),
        _ if has_var => Ok(ValueStatus::CategoryUnverified),
        _ => Ok(ValueStatus::Verified),
    }
}

#[derive(Clone, Debug)]
struct CssFunction {
    name: String,
    arguments: String,
}

fn scan_css_value(value: &str) -> Result<Vec<CssFunction>, String> {
    let mut functions = Vec::new();
    scan_component_range(value, 0, value.len(), &mut functions)?;
    Ok(functions)
}

fn scan_component_range(
    value: &str,
    mut index: usize,
    end: usize,
    functions: &mut Vec<CssFunction>,
) -> Result<(), String> {
    let bytes = value.as_bytes();
    while index < end {
        match bytes[index] {
            b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' => index += 1,
            b'\'' | b'"' => index = skip_string(value, index, end)?,
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_comment(value, index, end)?;
            }
            b';' | b'{' | b'}' => {
                return Err("declaration separators and block delimiters are forbidden".to_owned());
            }
            b'!' => return Err("!important is forbidden in token values".to_owned()),
            b'(' => {
                let close = find_matching_paren(value, index, end)?;
                scan_component_range(value, index + 1, close, functions)?;
                index = close + 1;
            }
            b')' => return Err("unbalanced parenthesis in token value".to_owned()),
            b'[' | b']' => return Err("bracket blocks are not token values".to_owned()),
            b':' | b'&' | b'>' | b'~' => {
                return Err("selector fragments are forbidden in token values".to_owned());
            }
            b'#' => {
                let mut hash_end = index + 1;
                while hash_end < end
                    && (bytes[hash_end].is_ascii_alphanumeric()
                        || bytes[hash_end] == b'-'
                        || bytes[hash_end] == b'_')
                {
                    hash_end += 1;
                }
                let hash = &value[index + 1..hash_end];
                let valid_color_hash = [3, 4, 6, 8].contains(&hash.len())
                    && hash.bytes().all(|byte| byte.is_ascii_hexdigit());
                if !valid_color_hash {
                    return Err("selector fragments are forbidden in token values".to_owned());
                }
                index = hash_end;
            }
            b'.' if bytes.get(index + 1).is_some_and(u8::is_ascii_alphabetic) => {
                return Err("selector fragments are forbidden in token values".to_owned());
            }
            byte if starts_identifier(value, index, end) => {
                let (name, ident_end) = parse_identifier(value, index, end)?;
                let open = skip_comments_only(value, ident_end, end)?;
                if bytes.get(open) == Some(&b'(') {
                    let close = find_matching_paren(value, open, end)?;
                    let arguments = value[open + 1..close].to_owned();
                    functions.push(CssFunction {
                        name: name.to_ascii_lowercase(),
                        arguments: arguments.clone(),
                    });
                    scan_component_range(value, open + 1, close, functions)?;
                    index = close + 1;
                } else {
                    index = ident_end;
                }
                let _ = byte;
            }
            b'\\' => {
                let _ = consume_escape(value, index, end)?;
                return Err("escaped delimiters are not supported in token values".to_owned());
            }
            _ => {
                index += value[index..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(1)
            }
        }
    }
    Ok(())
}

fn skip_comments_only(value: &str, mut index: usize, end: usize) -> Result<usize, String> {
    while index + 1 < end && value.as_bytes()[index] == b'/' && value.as_bytes()[index + 1] == b'*'
    {
        index = skip_comment(value, index, end)?;
    }
    Ok(index)
}

fn starts_identifier(value: &str, index: usize, end: usize) -> bool {
    let byte = value.as_bytes()[index];
    byte.is_ascii_alphabetic()
        || byte == b'_'
        || byte >= 0x80
        || (byte == b'-'
            && value
                .as_bytes()
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_alphabetic() || *next == b'-' || *next == b'_'))
        || (byte == b'\\' && index + 1 < end)
}

fn parse_identifier(value: &str, mut index: usize, end: usize) -> Result<(String, usize), String> {
    let mut decoded = String::new();
    while index < end {
        let byte = value.as_bytes()[index];
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte >= 0x80 {
            let character = value[index..]
                .chars()
                .next()
                .ok_or_else(|| "invalid identifier".to_owned())?;
            decoded.push(character);
            index += character.len_utf8();
        } else if byte == b'\\' {
            let (character, next) = consume_escape(value, index, end)?;
            decoded.push(character);
            index = next;
        } else {
            break;
        }
    }
    if decoded.is_empty() {
        return Err("invalid CSS identifier in token value".to_owned());
    }
    Ok((decoded, index))
}

fn consume_escape(value: &str, index: usize, end: usize) -> Result<(char, usize), String> {
    let bytes = value.as_bytes();
    if bytes.get(index + 1).is_none() || index + 1 >= end {
        return Err("trailing CSS escape in token value".to_owned());
    }
    let mut cursor = index + 1;
    if bytes[cursor].is_ascii_hexdigit() {
        let start = cursor;
        while cursor < end && cursor - start < 6 && bytes[cursor].is_ascii_hexdigit() {
            cursor += 1;
        }
        let scalar = u32::from_str_radix(&value[start..cursor], 16).unwrap_or(0xfffd);
        if cursor < end && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        Ok((char::from_u32(scalar).unwrap_or('\u{fffd}'), cursor))
    } else {
        let character = value[cursor..]
            .chars()
            .next()
            .ok_or_else(|| "invalid CSS escape".to_owned())?;
        Ok((character, cursor + character.len_utf8()))
    }
}

fn skip_string(value: &str, start: usize, end: usize) -> Result<usize, String> {
    let quote = value.as_bytes()[start];
    let mut index = start + 1;
    while index < end {
        match value.as_bytes()[index] {
            byte if byte == quote => return Ok(index + 1),
            b'\\' => index = consume_escape(value, index, end)?.1,
            _ => {
                index += value[index..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(1)
            }
        }
    }
    Err("unterminated string in token value".to_owned())
}

fn skip_comment(value: &str, start: usize, end: usize) -> Result<usize, String> {
    value[start + 2..end]
        .find("*/")
        .map(|offset| start + 2 + offset + 2)
        .ok_or_else(|| "unterminated comment in token value".to_owned())
}

fn find_matching_paren(value: &str, open: usize, end: usize) -> Result<usize, String> {
    let mut depth = 1_usize;
    let mut index = open + 1;
    while index < end {
        match value.as_bytes()[index] {
            b'\'' | b'"' => index = skip_string(value, index, end)?,
            b'/' if value.as_bytes().get(index + 1) == Some(&b'*') => {
                index = skip_comment(value, index, end)?;
            }
            b'\\' => index = consume_escape(value, index, end)?.1,
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(index);
                }
                index += 1;
            }
            _ => {
                index += value[index..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(1)
            }
        }
    }
    Err("unbalanced parenthesis in token value".to_owned())
}

fn variable_references_reserved_namespace(arguments: &str) -> bool {
    let trimmed = arguments.trim_start();
    let Ok((name, _)) = parse_identifier(trimmed, 0, trimmed.len()) else {
        return false;
    };
    name.to_ascii_lowercase().starts_with("--zw-")
}

fn is_css_wide_keyword(value: &str) -> bool {
    ["inherit", "initial", "unset", "revert", "revert-layer"]
        .iter()
        .any(|keyword| value.eq_ignore_ascii_case(keyword))
}

fn is_integer(value: &str) -> bool {
    let digits = value
        .strip_prefix('-')
        .or_else(|| value.strip_prefix('+'))
        .unwrap_or(value);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn has_top_level_comma(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' => match skip_string(value, index, bytes.len()) {
                Ok(next) => index = next,
                Err(_) => return false,
            },
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                match skip_comment(value, index, bytes.len()) {
                    Ok(next) => index = next,
                    Err(_) => return false,
                }
            }
            b'(' => match find_matching_paren(value, index, bytes.len()) {
                Ok(close) => index = close + 1,
                Err(_) => return false,
            },
            b',' => return true,
            _ => {
                index += value[index..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(1)
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{validate_value, ValueCategory, ValueStatus};

    #[test]
    fn every_category_accepts_a_valid_typed_value() {
        for (category, value) in [
            (ValueCategory::Color, "#123456"),
            (ValueCategory::SpacingUnit, "0.25rem"),
            (ValueCategory::Spacing, "12%"),
            (ValueCategory::Size, "42rem"),
            (ValueCategory::FontSize, "1.125rem"),
            (ValueCategory::LineHeight, "1.5"),
            (ValueCategory::FontFamily, "ui-monospace, monospace"),
            (ValueCategory::FontWeight, "600"),
            (ValueCategory::LetterSpacing, "-0.025em"),
            (ValueCategory::Radius, "0.375rem"),
            (ValueCategory::Shadow, "0 1px 2px #000"),
            (ValueCategory::ZIndex, "120"),
            (ValueCategory::Easing, "cubic-bezier(0.2, 0, 0, 1)"),
        ] {
            assert_eq!(
                validate_value(category, value),
                Ok(ValueStatus::Verified),
                "{category:?}: {value}"
            );
        }
    }

    #[test]
    fn variable_values_are_accepted_but_category_unverified() {
        assert_eq!(
            validate_value(ValueCategory::Color, "var(--project-panel)"),
            Ok(ValueStatus::CategoryUnverified)
        );
        assert_eq!(
            validate_value(ValueCategory::FontFamily, "var(--project-font), sans-serif"),
            Ok(ValueStatus::CategoryUnverified)
        );
    }

    #[test]
    fn wrong_categories_and_special_restrictions_are_rejected() {
        for (category, value) in [
            (ValueCategory::Color, "12px"),
            (ValueCategory::SpacingUnit, "12%"),
            (ValueCategory::SpacingUnit, "calc(1rem + 1px)"),
            (ValueCategory::Spacing, "-1px"),
            (ValueCategory::Radius, "1px 2px"),
            (ValueCategory::ZIndex, "1.5"),
            (ValueCategory::Easing, "ease, linear"),
            (ValueCategory::Color, "inherit"),
        ] {
            assert!(
                validate_value(category, value).is_err(),
                "{category:?}: {value}"
            );
        }
    }

    #[test]
    fn declarations_blocks_selectors_urls_and_reserved_variables_are_rejected() {
        for value in [
            "red; color: blue",
            "red { color: blue }",
            "&:hover",
            "url(image.svg)",
            "linear-gradient(red, url(image.svg))",
            "var(--zw-color-panel)",
            "var(--project-panel) #not-a-color",
            "var(--project-panel) url/* comments cannot hide this */(image.svg)",
            "red !important",
        ] {
            assert!(
                validate_value(ValueCategory::Color, value).is_err(),
                "{value}"
            );
        }
        assert!(validate_value(ValueCategory::Color, "U\\52L(image.svg)").is_err());
    }
}
