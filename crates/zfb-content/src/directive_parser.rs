//! Generic remark-directive compatible source parser.
//!
//! `markdown` has no extension hook, so directives are claimed from the
//! original UTF-8 source, hidden in a byte-length-preserving overlay, and
//! spliced into a recursive carrier containing the ordinary markdown-rs
//! nodes.  Positions in this module are always UTF-8 byte positions.

use std::collections::BTreeMap;

use markdown::unist::{Point, Position};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_general_category::{get_general_category, GeneralCategory};

use crate::facade::{parse_mdast, ParseMdastOptions};
use crate::pipeline::PipelineError;

/// A byte-positioned mdast node which can hold both markdown-rs nodes and the
/// three directive node kinds.  Variant-specific fields are preserved in
/// `fields`; structural fields stay typed for downstream consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectiveMdastNode {
    #[serde(rename = "type")]
    pub kind: String,
    pub position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<DirectiveMdastNode>>,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

impl DirectiveMdastNode {
    /// Return this node's half-open UTF-8 byte range.
    #[must_use]
    pub fn byte_range(&self) -> std::ops::Range<usize> {
        self.position.start.offset..self.position.end.offset
    }
}

/// Parse a complete source into a fully composed byte-positioned mdast root.
///
/// The selected Markdown/MDX dialect and GFM toggles are exactly the same
/// options accepted by [`crate::facade::parse_mdast`].  Malformed directive
/// candidates remain ordinary source and do not produce a new diagnostic.
pub fn parse_directive_mdast(
    options: ParseMdastOptions,
    source: &str,
) -> Result<DirectiveMdastNode, PipelineError> {
    parse_region(options, source)
}

#[derive(Debug, Clone)]
struct Claim {
    start: usize,
    end: usize,
    node: DirectiveMdastNode,
    block: bool,
}

#[derive(Debug, Clone)]
struct Head {
    name: String,
    label: Option<std::ops::Range<usize>>,
    attributes: BTreeMap<String, String>,
    end: usize,
}

#[derive(Debug, Clone, Copy)]
struct FlowPrefix {
    content: usize,
    blockquote_depth: usize,
}

fn parse_region(
    options: ParseMdastOptions,
    region: &str,
) -> Result<DirectiveMdastNode, PipelineError> {
    let claims = scan_claims(options, region)?;
    let overlay = build_overlay(region, &claims);
    let base = parse_mdast(options, &overlay)?;
    let value = serde_json::to_value(base)
        .map_err(|error| PipelineError::Parse(format!("serialize mdast: {error}")))?;
    let mut root = carrier_from_value(value)?;

    for claim in &claims {
        splice_claim(&mut root, claim, &overlay);
    }
    normalize_parent_positions(&mut root, false);
    if let Some(last) = claims
        .iter()
        .filter(|claim| claim.block)
        .max_by_key(|claim| claim.end)
    {
        if root.position.end.offset == last.end {
            root.position.end = point_at(region, next_line(region, last.end));
        }
    }
    validate_tree(&root, region)?;
    Ok(root)
}

fn scan_claims(options: ParseMdastOptions, region: &str) -> Result<Vec<Claim>, PipelineError> {
    let mut claims = Vec::new();
    let mut offset = 0;
    let mut fenced: Option<(u8, usize)> = None;

    while offset < region.len() {
        // Frontmatter is a protected document-level construct.  In the wasm
        // `node` policy zfb has already recognized the exact YAML range and
        // enables this flag; do not let directive-looking YAML values become
        // directive claims before markdown-rs gets to emit the canonical node.
        if offset == 0 && options.frontmatter {
            if let Some(end) = markdown_frontmatter_end(region) {
                offset = end;
                continue;
            }
        }
        let (line_end, next) = line_bounds(region, offset);
        let line = &region[offset..line_end];
        let prefix = flow_prefix(line);
        let content = prefix.map_or("", |prefix| &line[prefix.content..]);

        if let Some((marker, size)) = fenced {
            if fence_sequence(content, marker) >= size
                && content[fence_sequence(content, marker)..]
                    .bytes()
                    .all(|byte| byte == b' ' || byte == b'\t')
            {
                fenced = None;
            }
            offset = next;
            continue;
        }

        if let Some(marker) = content.as_bytes().first().copied() {
            if (marker == b'`' || marker == b'~') && fence_sequence(content, marker) >= 3 {
                fenced = Some((marker, fence_sequence(content, marker)));
                offset = next;
                continue;
            }
        }

        let colons = content.bytes().take_while(|byte| *byte == b':').count();
        if colons >= 3 {
            let head_start = offset + prefix.expect("nonempty flow content").content;
            if let Some(head) = parse_head(region, head_start + colons, line_end, false) {
                if region[head.end..line_end]
                    .bytes()
                    .all(|byte| byte == b' ' || byte == b'\t')
                {
                    let mut search = next;
                    let mut closing = None;
                    while search < region.len() {
                        let (candidate_end, candidate_next) = line_bounds(region, search);
                        let candidate = &region[search..candidate_end];
                        if let Some(candidate_prefix) = flow_prefix(candidate) {
                            let rest = &candidate[candidate_prefix.content..];
                            let count = rest.bytes().take_while(|byte| *byte == b':').count();
                            if count >= colons
                                && rest[count..]
                                    .bytes()
                                    .all(|byte| byte == b' ' || byte == b'\t')
                            {
                                closing = Some((
                                    search + candidate_prefix.content,
                                    candidate_end,
                                    search,
                                ));
                                break;
                            }
                        }
                        search = candidate_next;
                    }

                    let body_start = next.min(region.len());
                    let body_end = closing.map_or(region.len(), |value| value.2);
                    let node_end = closing.map_or(region.len(), |value| value.1);
                    let mut children = label_children(options, region, head.label.clone(), true)?;
                    if body_start < body_end {
                        let depth = prefix
                            .expect("container has a flow prefix")
                            .blockquote_depth;
                        let mut body_root = parse_region(options, &region[body_start..body_end])?;
                        shift_node(&mut body_root, region, body_start);
                        children.extend(unwrap_blockquote_children(body_root, depth));
                    }
                    let node = directive_node(
                        "containerDirective",
                        head.name,
                        head.attributes,
                        children,
                        region,
                        head_start,
                        node_end,
                    );
                    claims.push(Claim {
                        start: head_start,
                        end: node_end,
                        node,
                        block: true,
                    });
                    offset = if closing.is_some() {
                        next_line(region, node_end)
                    } else {
                        region.len()
                    };
                    continue;
                }
            }
        } else if colons == 2 {
            let head_start = offset + prefix.expect("nonempty flow content").content;
            if let Some(head) = parse_head(region, head_start + 2, line_end, false) {
                if region[head.end..line_end]
                    .bytes()
                    .all(|byte| byte == b' ' || byte == b'\t')
                {
                    let children = label_children(options, region, head.label.clone(), false)?;
                    let node = directive_node(
                        "leafDirective",
                        head.name,
                        head.attributes,
                        children,
                        region,
                        head_start,
                        line_end,
                    );
                    claims.push(Claim {
                        start: head_start,
                        end: line_end,
                        node,
                        block: true,
                    });
                    offset = next;
                    continue;
                }
            }
        }
        offset = next;
    }

    // Phrasing directives are scanned only outside ranges already claimed as
    // flow syntax.  A successful text directive owns its complete range, so
    // nested bracket content is parsed exactly once by `label_children`.
    let mut cursor = if options.frontmatter {
        markdown_frontmatter_end(region).unwrap_or(0)
    } else {
        0
    };
    while cursor < region.len() {
        if let Some(block) = claims
            .iter()
            .find(|claim| claim.block && cursor >= claim.start && cursor < claim.end)
        {
            cursor = block.end;
            continue;
        }
        if let Some(end) = protected_end(options, region, cursor) {
            cursor = end;
            continue;
        }
        if region.as_bytes()[cursor] == b':'
            && text_previous_allows(region, cursor)
            && !is_indented_code_line(region, cursor)
        {
            let line_end = line_bounds(region, cursor).0;
            if let Some(head) = parse_head(region, cursor + 1, region.len(), true) {
                // A second colon after the name makes the text construct fail.
                if head.end <= region.len()
                    && region.as_bytes().get(name_end(region, cursor + 1)) != Some(&b':')
                {
                    let children = label_children(options, region, head.label.clone(), false)?;
                    let node = directive_node(
                        "textDirective",
                        head.name,
                        head.attributes,
                        children,
                        region,
                        cursor,
                        head.end,
                    );
                    claims.push(Claim {
                        start: cursor,
                        end: head.end,
                        node,
                        block: false,
                    });
                    cursor = head.end;
                    continue;
                }
            }
            cursor = line_end.min(cursor + 1).max(cursor + 1);
        } else {
            cursor += region[cursor..].chars().next().map_or(1, char::len_utf8);
        }
    }
    claims.sort_by_key(|claim| claim.start);
    Ok(claims)
}

fn markdown_frontmatter_end(source: &str) -> Option<usize> {
    let (open_end, after_open_offset) = line_bounds(source, 0);
    let opener = source[..open_end]
        .strip_suffix('\r')
        .unwrap_or(&source[..open_end])
        .trim_end_matches([' ', '\t']);
    let marker = match opener {
        "---" => "---",
        "+++" => "+++",
        _ => return None,
    };
    let after_open = &source[after_open_offset..];
    let mut line_start = 0usize;
    while line_start <= after_open.len() {
        let relative_end = after_open.as_bytes()[line_start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|position| line_start + position);
        let line_end = relative_end.unwrap_or(after_open.len());
        let content_end = line_end
            - usize::from(line_end > line_start && after_open.as_bytes()[line_end - 1] == b'\r');
        if after_open[line_start..content_end].trim_end_matches([' ', '\t']) == marker {
            return Some(after_open_offset + relative_end.map_or(line_end, |end| end + 1));
        }
        match relative_end {
            Some(end) => line_start = end + 1,
            None => break,
        }
    }
    None
}

fn parse_head(source: &str, start: usize, limit: usize, multiline: bool) -> Option<Head> {
    let end_name = name_end(source, start);
    if end_name == start {
        return None;
    }
    let name = source[start..end_name].to_string();
    let mut cursor = end_name;
    let mut label = None;
    if source.as_bytes().get(cursor) == Some(&b'[') {
        if let Some(end) = parse_label(source, cursor, limit, multiline) {
            label = Some(cursor..end);
            cursor = end;
        }
    }
    let mut attributes = BTreeMap::new();
    if source.as_bytes().get(cursor) == Some(&b'{') {
        if let Some((end, parsed)) = parse_attributes(source, cursor, limit, multiline) {
            attributes = parsed;
            cursor = end;
        }
    }
    Some(Head {
        name,
        label,
        attributes,
        end: cursor,
    })
}

fn name_end(source: &str, start: usize) -> usize {
    let mut cursor = start;
    let mut previous = None;
    for (relative, ch) in source[start..].char_indices() {
        if ch == '\n'
            || ch == '\r'
            || ch.is_whitespace()
            || (unicode_punctuation(ch) && ch != '-' && ch != '_')
        {
            break;
        }
        previous = Some(ch);
        cursor = start + relative + ch.len_utf8();
    }
    if matches!(previous, Some('-' | '_')) {
        start
    } else {
        cursor
    }
}

fn parse_label(source: &str, start: usize, limit: usize, multiline: bool) -> Option<usize> {
    let mut cursor = start + 1;
    let mut balance = 0usize;
    while cursor < limit {
        let ch = source[cursor..].chars().next()?;
        if !multiline && (ch == '\n' || ch == '\r') {
            return None;
        }
        if ch == '\\' {
            let next = cursor + 1;
            if let Some(escaped) = source.get(next..).and_then(|tail| tail.chars().next()) {
                if escaped == '[' || escaped == ']' || escaped == '\\' {
                    cursor = next + escaped.len_utf8();
                    continue;
                }
            }
        } else if ch == '[' {
            balance += 1;
            if balance > 32 {
                return None;
            }
        } else if ch == ']' {
            if balance == 0 {
                return Some(cursor + 1);
            }
            balance -= 1;
        }
        cursor += ch.len_utf8();
    }
    None
}

fn parse_attributes(
    source: &str,
    start: usize,
    limit: usize,
    multiline: bool,
) -> Option<(usize, BTreeMap<String, String>)> {
    let mut cursor = start + 1;
    let mut pairs: Vec<(String, String)> = Vec::new();
    while cursor < limit {
        cursor = skip_attribute_space(source, cursor, limit, multiline)?;
        let byte = *source.as_bytes().get(cursor)?;
        if byte == b'}' {
            let mut attributes = BTreeMap::new();
            for (name, value) in pairs {
                if name == "class" && attributes.contains_key("class") {
                    attributes.entry(name).and_modify(|old: &mut String| {
                        old.push(' ');
                        old.push_str(&value);
                    });
                } else {
                    attributes.insert(name, value);
                }
            }
            return Some((cursor + 1, attributes));
        }
        if byte == b'#' || byte == b'.' {
            let name = if byte == b'#' { "id" } else { "class" };
            cursor += 1;
            let value_start = cursor;
            while cursor < limit {
                let current = source.as_bytes()[cursor];
                if matches!(current, b'#' | b'.' | b'}' | b' ' | b'\t' | b'\n' | b'\r') {
                    break;
                }
                if matches!(current, b'"' | b'\'' | b'<' | b'=' | b'>' | b'`') {
                    return None;
                }
                cursor += source[cursor..].chars().next()?.len_utf8();
            }
            if cursor == value_start {
                return None;
            }
            pairs.push((
                name.to_string(),
                decode_attribute(&source[value_start..cursor]),
            ));
            continue;
        }

        let name_start = cursor;
        while cursor < limit {
            let ch = source[cursor..].chars().next()?;
            if ch == '\n'
                || ch == '\r'
                || ch.is_whitespace()
                || (unicode_punctuation(ch) && ch != '-' && ch != '.' && ch != ':' && ch != '_')
            {
                break;
            }
            cursor += ch.len_utf8();
        }
        if cursor == name_start {
            return None;
        }
        let name = source[name_start..cursor].to_string();
        cursor = skip_attribute_space(source, cursor, limit, multiline)?;
        if source.as_bytes().get(cursor) != Some(&b'=') {
            pairs.push((name, String::new()));
            continue;
        }
        cursor += 1;
        cursor = skip_attribute_space(source, cursor, limit, multiline)?;
        let first = *source.as_bytes().get(cursor)?;
        if matches!(first, b'<' | b'=' | b'>' | b'`' | b'}' | b'\n' | b'\r') {
            return None;
        }
        let value;
        if first == b'"' || first == b'\'' {
            let marker = first;
            cursor += 1;
            let value_start = cursor;
            while cursor < limit && source.as_bytes()[cursor] != marker {
                if !multiline && matches!(source.as_bytes()[cursor], b'\n' | b'\r') {
                    return None;
                }
                cursor += source[cursor..].chars().next()?.len_utf8();
            }
            if cursor >= limit {
                return None;
            }
            value = decode_attribute(&source[value_start..cursor]);
            cursor += 1;
            if cursor < limit
                && !matches!(
                    source.as_bytes()[cursor],
                    b'}' | b' ' | b'\t' | b'\n' | b'\r'
                )
            {
                return None;
            }
        } else {
            let value_start = cursor;
            while cursor < limit {
                let current = source.as_bytes()[cursor];
                if matches!(current, b'}' | b' ' | b'\t' | b'\n' | b'\r') {
                    break;
                }
                if matches!(current, b'"' | b'\'' | b'<' | b'=' | b'>' | b'`') {
                    return None;
                }
                cursor += source[cursor..].chars().next()?.len_utf8();
            }
            value = decode_attribute(&source[value_start..cursor]);
        }
        pairs.push((name, value));
    }
    None
}

fn skip_attribute_space(
    source: &str,
    mut cursor: usize,
    limit: usize,
    multiline: bool,
) -> Option<usize> {
    while cursor < limit {
        match source.as_bytes()[cursor] {
            b' ' | b'\t' => cursor += 1,
            b'\n' if multiline => cursor += 1,
            b'\r' if multiline => {
                cursor += 1;
                if source.as_bytes().get(cursor) == Some(&b'\n') {
                    cursor += 1;
                }
            }
            b'\n' | b'\r' => return None,
            _ => break,
        }
    }
    Some(cursor)
}

fn decode_attribute(value: &str) -> String {
    html_escape::decode_html_entities(value).into_owned()
}

fn unicode_punctuation(ch: char) -> bool {
    matches!(
        get_general_category(ch),
        GeneralCategory::ClosePunctuation
            | GeneralCategory::ConnectorPunctuation
            | GeneralCategory::CurrencySymbol
            | GeneralCategory::DashPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::MathSymbol
            | GeneralCategory::ModifierSymbol
            | GeneralCategory::OpenPunctuation
            | GeneralCategory::OtherPunctuation
            | GeneralCategory::OtherSymbol
    )
}

fn label_children(
    options: ParseMdastOptions,
    region: &str,
    label: Option<std::ops::Range<usize>>,
    container: bool,
) -> Result<Vec<DirectiveMdastNode>, PipelineError> {
    let Some(label) = label else {
        return Ok(Vec::new());
    };
    let content = (label.start + 1)..(label.end - 1);
    let mut parsed = parse_region(options, &region[content.clone()])?;
    shift_node(&mut parsed, region, content.start);
    let mut phrasing = parsed
        .children
        .take()
        .and_then(|mut nodes| nodes.drain(..).next())
        .and_then(|node| node.children)
        .unwrap_or_default();
    if !container {
        return Ok(phrasing);
    }
    let mut fields = BTreeMap::new();
    fields.insert(
        "data".to_string(),
        serde_json::json!({"directiveLabel": true}),
    );
    Ok(vec![DirectiveMdastNode {
        kind: "paragraph".to_string(),
        position: position_at(region, label.start, label.end),
        children: Some(std::mem::take(&mut phrasing)),
        fields,
    }])
}

fn directive_node(
    kind: &str,
    name: String,
    attributes: BTreeMap<String, String>,
    children: Vec<DirectiveMdastNode>,
    original: &str,
    start: usize,
    end: usize,
) -> DirectiveMdastNode {
    let mut fields = BTreeMap::new();
    fields.insert("name".to_string(), Value::String(name));
    fields.insert(
        "attributes".to_string(),
        serde_json::to_value(attributes).expect("BTreeMap<String, String> serializes"),
    );
    DirectiveMdastNode {
        kind: kind.to_string(),
        position: position_at(original, start, end),
        children: Some(children),
        fields,
    }
}

fn build_overlay(source: &str, claims: &[Claim]) -> String {
    let mut bytes = source.as_bytes().to_vec();
    for claim in claims {
        for (index, byte) in bytes[claim.start..claim.end].iter_mut().enumerate() {
            if *byte != b'\n' && *byte != b'\r' {
                *byte = if index == 0 { b'x' } else { b' ' };
            }
        }
    }
    String::from_utf8(bytes).expect("masking UTF-8 bytes with ASCII preserves UTF-8")
}

fn splice_claim(root: &mut DirectiveMdastNode, claim: &Claim, overlay: &str) {
    if claim.block {
        splice_block_claim(root, claim);
        return;
    }
    if descend_for_claim(root, claim, overlay) {
        return;
    }
    if let Some(children) = root.children.as_mut() {
        splice_children(children, claim, overlay);
    }
}

fn splice_block_claim(node: &mut DirectiveMdastNode, claim: &Claim) {
    let Some(children) = node.children.as_mut() else {
        return;
    };
    for child in children.iter_mut() {
        let range = child.byte_range();
        if range.start <= claim.start && range.end > claim.start && is_flow_parent(&child.kind) {
            splice_block_claim(child, claim);
            return;
        }
    }
    let first = children
        .iter()
        .position(|child| {
            let range = child.byte_range();
            range.start <= claim.start && range.end > claim.start
        })
        .unwrap_or_else(|| {
            children
                .iter()
                .position(|child| child.position.start.offset > claim.start)
                .unwrap_or(children.len())
        });
    let mut last = first;
    while last < children.len() && children[last].position.start.offset < claim.end {
        last += 1;
    }
    if first < children.len() {
        children.splice(first..last.max(first + 1), [claim.node.clone()]);
    } else {
        children.push(claim.node.clone());
    }
}

fn is_flow_parent(kind: &str) -> bool {
    matches!(
        kind,
        "root" | "blockquote" | "footnoteDefinition" | "list" | "listItem" | "mdxJsxFlowElement"
    )
}

fn descend_for_claim(node: &mut DirectiveMdastNode, claim: &Claim, overlay: &str) -> bool {
    let Some(children) = node.children.as_mut() else {
        return false;
    };
    for child in children.iter_mut() {
        let range = child.byte_range();
        if range.start <= claim.start && range.end > claim.start && child.children.is_some() {
            if descend_for_claim(child, claim, overlay) {
                return true;
            }
            if let Some(grandchildren) = child.children.as_mut() {
                splice_children(grandchildren, claim, overlay);
                return true;
            }
        }
    }
    false
}

fn splice_children(children: &mut Vec<DirectiveMdastNode>, claim: &Claim, overlay: &str) {
    let mut first = None;
    let mut last = None;
    for (index, child) in children.iter().enumerate() {
        let range = child.byte_range();
        if range.start < claim.end && range.end > claim.start {
            first.get_or_insert(index);
            last = Some(index);
        }
    }
    if let (Some(first), Some(last)) = (first, last) {
        if first == last && children[first].kind == "text" {
            if let Some(parts) = split_text(&children[first], claim, overlay) {
                children.splice(first..=last, parts);
                return;
            }
        }
        children.splice(first..=last, [claim.node.clone()]);
    } else {
        let index = children
            .iter()
            .position(|node| node.position.start.offset > claim.start)
            .unwrap_or(children.len());
        children.insert(index, claim.node.clone());
    }
}

fn split_text(
    text: &DirectiveMdastNode,
    claim: &Claim,
    overlay: &str,
) -> Option<Vec<DirectiveMdastNode>> {
    text.fields.get("value")?.as_str()?;
    let start = text.position.start.offset;
    let end = text.position.end.offset;
    if start > claim.start || end <= claim.start {
        return None;
    }
    let mut result = Vec::new();
    if start < claim.start {
        result.push(text_node(
            &overlay[start..claim.start],
            position_at(overlay, start, claim.start),
        ));
    }
    result.push(claim.node.clone());
    if claim.end < end {
        result.push(text_node(
            &overlay[claim.end..end],
            position_at(overlay, claim.end, end),
        ));
    }
    Some(result)
}

fn text_node(value: &str, position: Position) -> DirectiveMdastNode {
    let mut fields = BTreeMap::new();
    fields.insert("value".to_string(), Value::String(value.to_string()));
    DirectiveMdastNode {
        kind: "text".to_string(),
        position,
        children: None,
        fields,
    }
}

fn carrier_from_value(value: Value) -> Result<DirectiveMdastNode, PipelineError> {
    let Value::Object(mut object) = value else {
        return Err(PipelineError::Parse(
            "mdast node is not an object".to_string(),
        ));
    };
    let kind = object
        .remove("type")
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| PipelineError::Parse("mdast node has no type".to_string()))?;
    let position = serde_json::from_value(
        object
            .remove("position")
            .ok_or_else(|| PipelineError::Parse(format!("{kind} node has no position")))?,
    )
    .map_err(|error| PipelineError::Parse(format!("invalid {kind} position: {error}")))?;
    let children = match object.remove("children") {
        Some(Value::Array(values)) => Some(
            values
                .into_iter()
                .map(carrier_from_value)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Some(_) => {
            return Err(PipelineError::Parse(format!(
                "{kind} node children are not an array"
            )))
        }
        None => None,
    };
    Ok(DirectiveMdastNode {
        kind,
        position,
        children,
        fields: object.into_iter().collect(),
    })
}

fn normalize_parent_positions(node: &mut DirectiveMdastNode, inside_directive: bool) {
    let inside_directive = inside_directive || node.kind.ends_with("Directive");
    if let Some(children) = node.children.as_mut() {
        for child in children.iter_mut() {
            normalize_parent_positions(child, inside_directive);
        }
        if !children.is_empty() {
            let directive_label = node
                .fields
                .get("data")
                .and_then(Value::as_object)
                .and_then(|data| data.get("directiveLabel"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if inside_directive && node.kind == "paragraph" && !directive_label {
                node.position.start = children[0].position.start.clone();
                node.position.end = children[children.len() - 1].position.end.clone();
            } else if node.kind == "root"
                || children[0].position.start.offset < node.position.start.offset
            {
                node.position.start = children[0].position.start.clone();
            }
            if node.kind == "root"
                || children[children.len() - 1].position.end.offset > node.position.end.offset
            {
                node.position.end = children[children.len() - 1].position.end.clone();
            }
            if node.kind == "listItem"
                && children.len() == 1
                && children[0].kind.ends_with("Directive")
            {
                node.fields.insert("spread".to_string(), Value::Bool(false));
            }
        }
    }
}

fn shift_node(node: &mut DirectiveMdastNode, original: &str, absolute: usize) {
    let start = node.position.start.offset + absolute;
    let end = node.position.end.offset + absolute;
    node.position = position_at(original, start, end);
    if let Some(children) = node.children.as_mut() {
        for child in children {
            shift_node(child, original, absolute);
        }
    }
    shift_field_stops(&mut node.fields, absolute);
}

fn shift_field_stops(fields: &mut BTreeMap<String, Value>, absolute: usize) {
    fn visit(value: &mut Value, key: Option<&str>, absolute: usize) {
        match value {
            Value::Array(items) if key == Some("_markdownRsStops") => {
                for item in items {
                    if let Some(pair) = item.as_array_mut() {
                        if let Some(source_offset) = pair.get_mut(1) {
                            if let Some(value) = source_offset.as_u64() {
                                *source_offset = Value::from(value + absolute as u64);
                            }
                        }
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    visit(item, None, absolute);
                }
            }
            Value::Object(map) => {
                for (nested_key, nested) in map {
                    visit(nested, Some(nested_key), absolute);
                }
            }
            _ => {}
        }
    }

    for (key, value) in fields {
        visit(value, Some(key), absolute);
    }
}

fn validate_tree(node: &DirectiveMdastNode, source: &str) -> Result<(), PipelineError> {
    let range = node.byte_range();
    if range.start > range.end || source.get(range.clone()).is_none() {
        return Err(PipelineError::Parse(format!(
            "{} has invalid byte span {}..{}",
            node.kind, range.start, range.end
        )));
    }
    if let Some(children) = &node.children {
        let mut previous_end = range.start;
        for child in children {
            let child_range = child.byte_range();
            if previous_end > child_range.start {
                return Err(PipelineError::Parse(format!(
                    "sibling offsets are monotonic and non-overlapping under {}",
                    node.kind
                )));
            }
            if child_range.start < range.start || child_range.end > range.end {
                return Err(PipelineError::Parse(format!(
                    "{} span {}..{} escapes {} span {}..{}",
                    child.kind,
                    child_range.start,
                    child_range.end,
                    node.kind,
                    range.start,
                    range.end
                )));
            }
            validate_tree(child, source)?;
            previous_end = child_range.end;
        }
    }
    Ok(())
}

fn position_at(source: &str, start: usize, end: usize) -> Position {
    Position {
        start: point_at(source, start),
        end: point_at(source, end),
    }
}

fn point_at(source: &str, offset: usize) -> Point {
    let before = &source[..offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column_start = before.rfind('\n').map_or(0, |index| index + 1);
    Point::new(line, offset - column_start + 1, offset)
}

fn line_bounds(source: &str, start: usize) -> (usize, usize) {
    match source[start..].find('\n') {
        Some(relative) => {
            let newline = start + relative;
            let end = if newline > start && source.as_bytes()[newline - 1] == b'\r' {
                newline - 1
            } else {
                newline
            };
            (end, newline + 1)
        }
        None => (source.len(), source.len()),
    }
}

fn next_line(source: &str, offset: usize) -> usize {
    if source.as_bytes().get(offset) == Some(&b'\r')
        && source.as_bytes().get(offset + 1) == Some(&b'\n')
    {
        offset + 2
    } else if matches!(source.as_bytes().get(offset), Some(b'\n' | b'\r')) {
        offset + 1
    } else {
        offset
    }
}

fn fence_sequence(content: &str, marker: u8) -> usize {
    content.bytes().take_while(|byte| *byte == marker).count()
}

fn flow_prefix(line: &str) -> Option<FlowPrefix> {
    let bytes = line.as_bytes();
    let mut cursor = bytes.iter().take_while(|byte| **byte == b' ').count();
    if cursor > 3 {
        return None;
    }
    let mut blockquote_depth = 0;
    while bytes.get(cursor) == Some(&b'>') {
        blockquote_depth += 1;
        cursor += 1;
        if matches!(bytes.get(cursor), Some(b' ' | b'\t')) {
            cursor += 1;
        }
    }
    if matches!(bytes.get(cursor), Some(b'-' | b'+' | b'*'))
        && matches!(bytes.get(cursor + 1), Some(b' ' | b'\t'))
    {
        cursor += 2;
        while matches!(bytes.get(cursor), Some(b' ' | b'\t')) {
            cursor += 1;
        }
    } else {
        let digits = bytes[cursor..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits > 0
            && matches!(bytes.get(cursor + digits), Some(b'.' | b')'))
            && matches!(bytes.get(cursor + digits + 1), Some(b' ' | b'\t'))
        {
            cursor += digits + 2;
            while matches!(bytes.get(cursor), Some(b' ' | b'\t')) {
                cursor += 1;
            }
        }
    }
    Some(FlowPrefix {
        content: cursor,
        blockquote_depth,
    })
}

fn unwrap_blockquote_children(
    mut root: DirectiveMdastNode,
    depth: usize,
) -> Vec<DirectiveMdastNode> {
    let mut children = root.children.take().unwrap_or_default();
    for _ in 0..depth {
        if children.len() != 1 || children[0].kind != "blockquote" {
            break;
        }
        children = children.remove(0).children.take().unwrap_or_default();
    }
    children
}

fn is_indented_code_line(source: &str, offset: usize) -> bool {
    let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    source[line_start..offset]
        .bytes()
        .take_while(|byte| *byte == b' ')
        .count()
        >= 4
}

fn text_previous_allows(source: &str, offset: usize) -> bool {
    if offset == 0 {
        return true;
    }
    if source.as_bytes()[offset - 1] == b'\\' {
        let mut slashes = 0;
        let mut cursor = offset;
        while cursor > 0 && source.as_bytes()[cursor - 1] == b'\\' {
            slashes += 1;
            cursor -= 1;
        }
        return slashes % 2 == 0;
    }
    if source.as_bytes()[offset - 1] != b':' {
        return true;
    }
    let mut slashes = 0;
    let mut cursor = offset - 1;
    while cursor > 0 && source.as_bytes()[cursor - 1] == b'\\' {
        slashes += 1;
        cursor -= 1;
    }
    slashes % 2 == 1
}

fn protected_end(options: ParseMdastOptions, source: &str, start: usize) -> Option<usize> {
    let byte = source.as_bytes()[start];
    if options.gfm.autolink_literal && matches!(byte, b'h' | b'H' | b'w' | b'W') {
        if let Some(end) = gfm_autolink_literal_end(source, start) {
            return Some(end);
        }
    }
    if byte == b'[' {
        if let Some(end) = definition_line_end(source, start) {
            return Some(end);
        }
    }
    if byte == b'`' {
        let size = source[start..]
            .bytes()
            .take_while(|candidate| *candidate == b'`')
            .count();
        let marker = "`".repeat(size);
        return source[start + size..]
            .find(&marker)
            .map(|relative| start + size + relative + size)
            .or(Some(start + size));
    }
    if byte == b']' && source.as_bytes().get(start + 1) == Some(&b'(') {
        return balanced_end(source, start + 1, b'(', b')');
    }
    if byte == b'<' {
        if options.dialect == crate::facade::ParseDialect::Markdown {
            if let Some(end) = markdown_html_block_end(source, start) {
                return Some(end);
            }
        }
        return quoted_delimiter_end(source, start + 1, b'>');
    }
    if options.dialect == crate::facade::ParseDialect::Mdx && byte == b'{' {
        return balanced_end(source, start, b'{', b'}');
    }
    None
}

fn gfm_autolink_literal_end(source: &str, start: usize) -> Option<usize> {
    let rest = &source[start..];
    let is_http = rest
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || rest
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"));
    let is_www = rest
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("www."));

    let before = start.checked_sub(1).map(|offset| source.as_bytes()[offset]);
    if (is_http && before.is_some_and(|byte| byte.is_ascii_alphabetic()))
        || (is_www
            && !matches!(
                before,
                None | Some(b'\n' | b'\r' | b' ' | b'\t' | b'(' | b'*' | b'_' | b'[' | b']' | b'~')
            ))
        || (!is_http && !is_www)
    {
        return None;
    }

    Some(
        source[start..]
            .bytes()
            .position(|byte| matches!(byte, b'\n' | b'\r' | b' ' | b'\t'))
            .map_or(source.len(), |relative| start + relative),
    )
}

fn definition_line_end(source: &str, start: usize) -> Option<usize> {
    let line_start = source[..start].rfind('\n').map_or(0, |index| index + 1);
    let prefix = &source[line_start..start];
    if prefix.bytes().filter(|byte| *byte == b' ').count() > 3
        || !prefix.bytes().all(|byte| byte == b' ')
    {
        return None;
    }
    let (line_end, next) = line_bounds(source, start);
    source[start..line_end].find("]:").map(|_| next)
}

fn markdown_html_block_end(source: &str, start: usize) -> Option<usize> {
    let line_start = source[..start].rfind('\n').map_or(0, |index| index + 1);
    if !source[line_start..start]
        .bytes()
        .all(|byte| byte == b' ' || byte == b'\t')
    {
        return None;
    }
    let rest = &source[start..];
    for (opening, closing) in [("<!--", "-->"), ("<?", "?>"), ("<![CDATA[", "]]>")] {
        if rest.starts_with(opening) {
            return rest
                .find(closing)
                .map(|relative| start + relative + closing.len())
                .or(Some(source.len()));
        }
    }
    let lower = rest.to_ascii_lowercase();
    for tag in ["script", "style", "pre", "textarea"] {
        let opening = format!("<{tag}");
        if lower.starts_with(&opening)
            && lower[opening.len()..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_whitespace() || ch == '>' || ch == '/')
        {
            let closing = format!("</{tag}>");
            return lower
                .find(&closing)
                .map(|relative| start + relative + closing.len())
                .or(Some(source.len()));
        }
    }
    let name = source[start + 1..]
        .trim_start_matches('/')
        .split(|ch: char| ch.is_ascii_whitespace() || ch == '>' || ch == '/')
        .next()?
        .to_ascii_lowercase();
    const BLOCK_TAGS: &[&str] = &[
        "address",
        "article",
        "aside",
        "base",
        "basefont",
        "blockquote",
        "body",
        "caption",
        "center",
        "col",
        "colgroup",
        "dd",
        "details",
        "dialog",
        "dir",
        "div",
        "dl",
        "dt",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "frame",
        "frameset",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "head",
        "header",
        "hr",
        "html",
        "iframe",
        "legend",
        "li",
        "link",
        "main",
        "menu",
        "menuitem",
        "nav",
        "noframes",
        "ol",
        "optgroup",
        "option",
        "p",
        "param",
        "search",
        "section",
        "summary",
        "table",
        "tbody",
        "td",
        "tfoot",
        "th",
        "thead",
        "title",
        "tr",
        "track",
        "ul",
    ];
    if !BLOCK_TAGS.contains(&name.as_str()) {
        return None;
    }
    let mut cursor = line_bounds(source, start).1;
    while cursor < source.len() {
        let (end, next) = line_bounds(source, cursor);
        if source[cursor..end].trim().is_empty() {
            return Some(cursor);
        }
        cursor = next;
    }
    Some(source.len())
}

fn balanced_end(source: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = None;
    let mut cursor = start;
    while cursor < source.len() {
        let byte = source.as_bytes()[cursor];
        if let Some(marker) = quote {
            if byte == b'\\' {
                cursor = (cursor + 2).min(source.len());
                continue;
            }
            if byte == marker {
                quote = None;
            }
        } else if byte == b'"' || byte == b'\'' || byte == b'`' {
            quote = Some(byte);
        } else if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(cursor + 1);
            }
        }
        cursor += 1;
    }
    Some(source.len())
}

fn quoted_delimiter_end(source: &str, start: usize, delimiter: u8) -> Option<usize> {
    let mut quote = None;
    let mut cursor = start;
    while cursor < source.len() {
        let byte = source.as_bytes()[cursor];
        if let Some(marker) = quote {
            if byte == marker {
                quote = None;
            }
        } else if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
        } else if byte == delimiter {
            return Some(cursor + 1);
        } else if byte == b'\n' || byte == b'\r' {
            return None;
        }
        cursor += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facade::{GfmOptions, ParseDialect};

    fn options(dialect: ParseDialect) -> ParseMdastOptions {
        ParseMdastOptions {
            dialect,
            gfm: GfmOptions::default(),
            frontmatter: false,
        }
    }

    fn find<'a>(
        node: &'a DirectiveMdastNode,
        kind: &str,
        output: &mut Vec<&'a DirectiveMdastNode>,
    ) {
        if node.kind == kind {
            output.push(node);
        }
        if let Some(children) = &node.children {
            for child in children {
                find(child, kind, output);
            }
        }
    }

    #[test]
    fn parses_nested_container_labels_attributes_and_text() {
        let source =
            ":::note[Hi *x*]{#a .b disabled key=one title=\"a &amp; b\"}\nBody :x[yo].\n:::";
        let root = parse_directive_mdast(options(ParseDialect::Markdown), source).unwrap();
        let container = &root.children.as_ref().unwrap()[0];
        assert_eq!(container.kind, "containerDirective");
        assert_eq!(container.fields["name"], "note");
        assert_eq!(
            container.fields["attributes"],
            serde_json::json!({
                "class": "b", "disabled": "", "id": "a", "key": "one", "title": "a & b"
            })
        );
        assert_eq!(container.byte_range(), 0..74);
        let children = container.children.as_ref().unwrap();
        assert_eq!(
            children[0].fields["data"],
            serde_json::json!({"directiveLabel": true})
        );
        let mut text = Vec::new();
        find(&root, "textDirective", &mut text);
        assert_eq!(text.len(), 1);
        assert_eq!(text[0].byte_range(), 63..69);
    }

    #[test]
    fn protects_gfm_autolink_literals_from_directive_claims() {
        for source in [
            "http://localhost:4323/preview/x",
            "http://127.0.0.1:8080/a.html",
            "www.example.com:8080/x",
            "http://localhost:abc/x",
        ] {
            let root = parse_directive_mdast(options(ParseDialect::Markdown), source).unwrap();
            let mut links = Vec::new();
            find(&root, "link", &mut links);
            assert_eq!(links.len(), 1, "{source:?}");
            let expected_url = if source.starts_with("www.") {
                format!("http://{source}")
            } else {
                source.to_owned()
            };
            assert_eq!(links[0].fields["url"], expected_url, "{source:?}");
            assert_strictly_increasing_sibling_ranges(&root, source);
        }
    }

    #[test]
    fn autolink_protection_honors_before_restrictions_and_ends_at_whitespace() {
        let source = "xhttp://h:8080";
        let root = parse_directive_mdast(options(ParseDialect::Markdown), source).unwrap();
        let mut links = Vec::new();
        find(&root, "link", &mut links);
        assert!(links.is_empty());
        let mut directives = Vec::new();
        find(&root, "textDirective", &mut directives);
        assert_eq!(directives.len(), 1);
        assert_eq!(directives[0].fields["name"], "8080");
        assert_strictly_increasing_sibling_ranges(&root, source);

        let source = "see http://h:8080 :badge[x]";
        let root = parse_directive_mdast(options(ParseDialect::Markdown), source).unwrap();
        let mut links = Vec::new();
        find(&root, "link", &mut links);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].fields["url"], "http://h:8080");
        let mut directives = Vec::new();
        find(&root, "textDirective", &mut directives);
        assert_eq!(directives.len(), 1);
        assert_eq!(directives[0].fields["name"], "badge");
        assert_strictly_increasing_sibling_ranges(&root, source);

        let mut disabled = options(ParseDialect::Markdown);
        disabled.gfm.autolink_literal = false;
        let source = "http://h:8080";
        let root = parse_directive_mdast(disabled, source).unwrap();
        let mut links = Vec::new();
        find(&root, "link", &mut links);
        assert!(links.is_empty());
        let mut directives = Vec::new();
        find(&root, "textDirective", &mut directives);
        assert_eq!(directives.len(), 1);
        assert_eq!(directives[0].fields["name"], "8080");
        assert_strictly_increasing_sibling_ranges(&root, source);
    }

    fn assert_strictly_increasing_sibling_ranges(node: &DirectiveMdastNode, source: &str) {
        if let Some(children) = &node.children {
            for pair in children.windows(2) {
                assert!(
                    pair[0].position.start.offset < pair[1].position.start.offset
                        && pair[0].position.end.offset <= pair[1].position.start.offset,
                    "overlapping sibling ranges {:?} and {:?} in {source:?}",
                    pair[0].byte_range(),
                    pair[1].byte_range()
                );
            }
            for child in children {
                assert_strictly_increasing_sibling_ranges(child, source);
            }
        }
    }

    #[test]
    fn malformed_flow_forms_stay_literal() {
        for source in [":::bad trailing\nbody\n:::", "::a[x] nope"] {
            let root = parse_directive_mdast(options(ParseDialect::Markdown), source).unwrap();
            let mut directives = Vec::new();
            find(&root, "containerDirective", &mut directives);
            find(&root, "leafDirective", &mut directives);
            assert!(directives.is_empty(), "{source:?}");
        }
    }

    #[test]
    fn supports_crlf_cjk_emoji_and_mdx_attribute_masking() {
        let source = "前 :印[絵文字 🦀]{題=\"値 &amp; 次\"} 後\r\n\r\n::葉[子]{真}\r\n";
        let root = parse_directive_mdast(options(ParseDialect::Mdx), source).unwrap();
        let mut text = Vec::new();
        find(&root, "textDirective", &mut text);
        assert_eq!(text.len(), 1);
        assert_eq!(text[0].fields["name"], "印");
        assert_eq!(
            text[0].fields["attributes"],
            serde_json::json!({"題": "値 & 次"})
        );
        let mut leaf = Vec::new();
        find(&root, "leafDirective", &mut leaf);
        assert_eq!(leaf.len(), 1);
        validate_tree(&root, source).unwrap();
    }

    #[test]
    fn rejects_overlapping_sibling_byte_ranges() {
        let source = "abcdef";
        let root = DirectiveMdastNode {
            kind: "root".to_string(),
            position: position_at(source, 0, source.len()),
            children: Some(vec![
                text_node("abcd", position_at(source, 0, 4)),
                text_node("def", position_at(source, 3, 6)),
            ]),
            fields: BTreeMap::new(),
        };

        let error = validate_tree(&root, source).unwrap_err();
        assert!(matches!(
            error,
            PipelineError::Parse(message)
                if message == "sibling offsets are monotonic and non-overlapping under root"
        ));
    }

    #[test]
    fn cjk_directive_name_stops_at_unicode_punctuation() {
        let source = "::漢字[ok]\n\n::漢。字[not a directive]\n\n::漢؝字[also not]\n";
        let root = parse_directive_mdast(options(ParseDialect::Markdown), source).unwrap();
        let mut leaves = Vec::new();
        find(&root, "leafDirective", &mut leaves);
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].fields["name"], "漢字");
    }

    #[test]
    fn markdown_frontmatter_is_protected_from_both_directive_passes() {
        for source in [
            "---  \r\nliteral: ':badge[x]'\r\nflow: ':::note'\r\n---\t\r\n:::body\r\nok\r\n:::\r\n",
            "+++\nliteral = ':badge[x]'\nflow = ':::note'\n+++\n:::body\nok\n:::\n",
        ] {
            let mut parse_options = options(ParseDialect::Markdown);
            parse_options.frontmatter = true;
            let ast = parse_directive_mdast(parse_options, source).expect("frontmatter parses");
            let serialized = serde_json::to_value(ast).unwrap();
            assert_eq!(
                count_kind(&serialized, "containerDirective"),
                1,
                "only the body directive is claimed"
            );
            assert_eq!(count_kind(&serialized, "textDirective"), 0);
        }
    }

    fn count_kind(value: &Value, kind: &str) -> usize {
        match value {
            Value::Array(values) => values.iter().map(|value| count_kind(value, kind)).sum(),
            Value::Object(map) => {
                usize::from(map.get("type").and_then(Value::as_str) == Some(kind))
                    + map
                        .values()
                        .map(|value| count_kind(value, kind))
                        .sum::<usize>()
            }
            _ => 0,
        }
    }
}
