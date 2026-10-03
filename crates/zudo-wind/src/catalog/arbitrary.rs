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
    let has_var = crate::value_check::check_variable_references(&value)?;
    let status = crate::value_check::property_value_status(property, &value, has_var)?;
    Ok((value, status))
}
