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
    if let Some(hint) = type_hint(&value) {
        return Err(format!(
            "data-type hint `{hint}:` is not supported; zudo-wind v1 has no type-hint grammar, so write the value without it"
        ));
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

/// A Tailwind-style leading `ident:` such as `color:` or `length:`.
fn type_hint(value: &str) -> Option<&str> {
    let (hint, _) = value.split_once(':')?;
    let mut bytes = hint.bytes();
    (bytes.next()?.is_ascii_alphabetic()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    .then_some(hint)
}

/// Insert `_` around each unspaced binary `+`/`-` inside `calc()`, the
/// spacing Tailwind added automatically and CSS requires. Hyphens that belong
/// to identifiers, custom-property names, unary signs and strings are left
/// alone. Returns `None` when nothing needs repair.
pub(super) fn space_calc_operators(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    let mut output = String::with_capacity(input.len() + 8);
    let mut copied = 0;
    let mut search = 0;
    let mut changed = false;
    while let Some(offset) = lower[search..].find("calc(") {
        let start = search + offset;
        let open = start + 4;
        let close = matching_paren(input, open)?;
        output.push_str(&input[copied..=open]);
        let (repaired, region_changed) = repair_math(&input[open + 1..close]);
        changed |= region_changed;
        output.push_str(&repaired);
        copied = close;
        search = close;
    }
    output.push_str(&input[copied..]);
    changed.then_some(output)
}

fn matching_paren(input: &str, open: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (offset, character) in input[open..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn repair_math(region: &str) -> (String, bool) {
    let chars: Vec<char> = region.chars().collect();
    let is_space = |character: char| character == '_' || character == ' ';
    let mut output = String::with_capacity(region.len() + 4);
    let mut changed = false;
    let mut after_value = false;
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        let next = chars.get(index + 1).copied();
        match character {
            '\\' => {
                output.push(character);
                if let Some(next) = next {
                    output.push(next);
                }
                index += 2;
                after_value = true;
                continue;
            }
            '\'' | '"' => {
                output.push(character);
                index += 1;
                while index < chars.len() {
                    output.push(chars[index]);
                    index += 1;
                    if chars[index - 1] == character {
                        break;
                    }
                }
                after_value = true;
                continue;
            }
            _ if is_space(character) => output.push(character),
            '+' | '-' if after_value => {
                if !output.ends_with(is_space) {
                    output.push('_');
                    changed = true;
                }
                output.push(character);
                if !next.is_some_and(is_space) {
                    output.push('_');
                    changed = true;
                }
                after_value = false;
            }
            _ if character.is_ascii_digit()
                || character == '.'
                || (matches!(character, '+' | '-')
                    && next.is_some_and(|next| next.is_ascii_digit() || next == '.')) =>
            {
                output.push(character);
                index += 1;
                while index < chars.len() && (chars[index].is_ascii_digit() || chars[index] == '.')
                {
                    output.push(chars[index]);
                    index += 1;
                }
                while index < chars.len()
                    && (chars[index].is_ascii_alphabetic() || chars[index] == '%')
                {
                    output.push(chars[index]);
                    index += 1;
                }
                after_value = true;
                continue;
            }
            _ if character.is_ascii_alphabetic() || character == '-' => {
                let start = index;
                while index < chars.len()
                    && (chars[index].is_ascii_alphanumeric() || chars[index] == '-')
                {
                    index += 1;
                }
                let name: String = chars[start..index].iter().collect();
                output.push_str(&name);
                if chars.get(index) == Some(&'(') {
                    let rest: String = chars[index..].iter().collect();
                    let close = matching_paren(&rest, 0).unwrap_or(rest.len() - 1);
                    let arguments: String = rest[1..close].to_owned();
                    output.push('(');
                    if name.eq_ignore_ascii_case("var") || name.eq_ignore_ascii_case("env") {
                        output.push_str(&arguments);
                    } else {
                        let (repaired, nested_changed) = repair_math(&arguments);
                        changed |= nested_changed;
                        output.push_str(&repaired);
                    }
                    if close < rest.len() {
                        output.push(')');
                    }
                    index += rest[..=close.min(rest.len() - 1)].chars().count();
                }
                after_value = true;
                continue;
            }
            '(' => {
                let rest: String = chars[index..].iter().collect();
                let close = matching_paren(&rest, 0).unwrap_or(rest.len() - 1);
                let (repaired, nested_changed) = repair_math(&rest[1..close]);
                changed |= nested_changed;
                output.push('(');
                output.push_str(&repaired);
                if close < rest.len() {
                    output.push(')');
                }
                index += rest[..=close.min(rest.len() - 1)].chars().count();
                after_value = true;
                continue;
            }
            _ => {
                output.push(character);
                after_value = false;
            }
        }
        index += 1;
    }
    (output, changed)
}
