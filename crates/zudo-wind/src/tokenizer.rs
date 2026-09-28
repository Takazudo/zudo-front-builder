//! Vocabulary-free, delimiter-aware candidate splitting.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralSplit<'a> {
    pub variants: Vec<&'a str>,
    pub utility: &'a str,
    /// The first top-level slash in the utility, if present.
    pub slash: Option<(&'a str, &'a str)>,
    pub negative: bool,
    pub leading_important: bool,
    pub trailing_important: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SplitError {
    Empty,
    Delimiter,
}

/// Splits one token without consulting configured breakpoints or utility roots.
/// Bracket, parenthesis, quote and escape state is shared by all separators.
pub fn structural_split(text: &str) -> Result<StructuralSplit<'_>, &'static str> {
    split(text).map_err(|error| match error {
        SplitError::Empty => "empty candidate or segment",
        SplitError::Delimiter => "unbalanced delimiter, quote or escape",
    })
}

pub(crate) fn split(text: &str) -> Result<StructuralSplit<'_>, SplitError> {
    if text.is_empty() {
        return Err(SplitError::Empty);
    }
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escape = false;
    let mut colons = Vec::new();
    let mut slashes = Vec::new();
    for (at, ch) in text.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        match ch {
            '[' | '(' => stack.push(ch),
            ']' if stack.pop() != Some('[') => return Err(SplitError::Delimiter),
            ')' if stack.pop() != Some('(') => return Err(SplitError::Delimiter),
            ':' if stack.is_empty() => colons.push(at),
            '/' if stack.is_empty() => slashes.push(at),
            _ => {}
        }
    }
    if escape || quote.is_some() || !stack.is_empty() {
        return Err(SplitError::Delimiter);
    }
    let mut start = 0;
    let mut variants = Vec::with_capacity(colons.len());
    for at in colons {
        if at == start {
            return Err(SplitError::Empty);
        }
        variants.push(&text[start..at]);
        start = at + 1;
    }
    if start == text.len() {
        return Err(SplitError::Empty);
    }
    let utility = &text[start..];
    let slash = slashes
        .into_iter()
        .find(|&at| at >= start)
        .map(|at| (&text[start..at], &text[at + 1..]));
    Ok(StructuralSplit {
        variants,
        utility,
        slash,
        negative: utility.starts_with('-'),
        leading_important: utility.starts_with('!'),
        trailing_important: utility.ends_with('!'),
    })
}
