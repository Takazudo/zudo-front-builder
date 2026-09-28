use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;

use crate::ValueStatus;

/// Decodes class spelling before asking Lightning CSS to validate the whole property value.
pub(super) fn validate(property: &str, input: &str) -> Result<(String, ValueStatus), String> {
    let mut value = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if chars.peek() == Some(&'_') => {
                chars.next();
                value.push('_');
            }
            '_' => value.push(' '),
            _ => value.push(ch),
        }
    }
    let lower = value.to_ascii_lowercase();
    if value.trim().is_empty()
        || value.trim_start().starts_with('-')
        || value.contains([';', '{', '}', '&', '>', '~', '!'])
        || value.contains("/*")
        || value.contains("*/")
        || lower.contains("url(")
        || lower.contains("!important")
    {
        return Err("arbitrary value contains forbidden CSS syntax".to_owned());
    }
    let placeholder = match property {
        "z-index" | "flex-grow" | "flex-shrink" => "1",
        "grid-template-columns" | "grid-template-rows" => "1fr",
        "color" | "background-color" | "border-color" | "outline-color" | "accent-color" => "red",
        "font-family" => "sans-serif",
        "font-weight" => "400",
        "opacity" => "0.5",
        "box-shadow" => "none",
        "aspect-ratio" => "1 / 1",
        "transition-property" => "all",
        "transition-duration" => "150ms",
        "transition-timing-function" => "ease",
        "rotate" => "0deg",
        _ => "1px",
    };
    let has_var = lower.contains("var(");
    let checked = if has_var {
        replace_vars(&value, placeholder)?
    } else {
        value.clone()
    };
    let parsed = Property::parse_string(
        PropertyId::from(property),
        &checked,
        ParserOptions::default(),
    )
    .map_err(|_| format!("value is not valid for {property}"))?;
    let unparsed = matches!(parsed, Property::Unparsed(_));
    drop(parsed);
    if unparsed {
        Err(format!("value is not valid for {property}"))
    } else if has_var {
        Ok((value, ValueStatus::CategoryUnverified))
    } else {
        Ok((value, ValueStatus::Verified))
    }
}

fn replace_vars(value: &str, placeholder: &str) -> Result<String, String> {
    let mut result = String::new();
    let mut rest = value;
    while let Some(index) = rest.to_ascii_lowercase().find("var(") {
        result.push_str(&rest[..index]);
        let function = &rest[index..];
        let mut depth = 0_u32;
        let mut end = None;
        for (offset, character) in function.char_indices() {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(offset + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end.ok_or_else(|| "unbalanced var()".to_owned())?;
        result.push_str(placeholder);
        rest = &function[end..];
    }
    result.push_str(rest);
    Ok(result)
}
