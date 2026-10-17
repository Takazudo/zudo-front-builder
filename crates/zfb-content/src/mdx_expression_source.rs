//! Recover expression whitespace omitted by markdown-rs 1.0.0's
//! `partial_mdx_expression::prefix`, at the compile boundary only (#4278).

use markdown::mdast::{AttributeContent, AttributeValue, Node};

#[derive(Clone, Copy)]
enum Container {
    Quote,
    Indent(usize),
}

// A cursor can stop inside a tab consumed partly by a Markdown container.
#[derive(Clone)]
struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
    column: usize,
    virtual_spaces: usize,
}

impl<'a> Cursor<'a> {
    fn new(source: &'a str, offset: usize) -> Self {
        Self {
            bytes: source.as_bytes(),
            offset,
            column: 0,
            virtual_spaces: 0,
        }
    }

    fn space(&mut self) -> bool {
        if self.virtual_spaces > 0 {
            self.virtual_spaces -= 1;
        } else {
            match self.bytes.get(self.offset) {
                Some(b' ') => self.offset += 1,
                Some(b'\t') => {
                    self.offset += 1;
                    self.virtual_spaces = 3 - self.column % 4;
                }
                _ => return false,
            }
        }
        self.column += 1;
        true
    }

    fn indent(&mut self, width: usize) {
        let before = self.clone();
        for _ in 0..width {
            if !self.space() {
                if !matches!(self.bytes.get(self.offset), None | Some(b'\r' | b'\n')) {
                    *self = before;
                }
                break;
            }
        }
    }

    fn container(&mut self, container: Container) {
        match container {
            Container::Indent(width) => self.indent(width),
            Container::Quote => {
                let before = self.clone();
                while self.space() {}
                if self.bytes.get(self.offset) == Some(&b'>') {
                    self.offset += 1;
                    self.column += 1;
                    self.space();
                } else {
                    *self = before;
                }
            }
        }
    }
}

fn list_indent(source: &str, offset: usize) -> usize {
    let line_start = source[..offset].rfind(['\r', '\n']).map_or(0, |i| i + 1);
    let mut cursor = Cursor::new(source, line_start);
    while cursor.offset < offset || cursor.virtual_spaces > 0 {
        if !cursor.space() {
            cursor.offset += 1;
            cursor.column += 1;
        }
    }
    let start_column = cursor.column;
    while cursor.space() {}
    while source
        .as_bytes()
        .get(cursor.offset)
        .is_some_and(u8::is_ascii_digit)
    {
        cursor.offset += 1;
        cursor.column += 1;
    }
    // The AST establishes that this is a list marker (bullet, '.' or ')').
    cursor.offset += 1;
    cursor.column += 1;
    let marker_end = cursor.column;
    while cursor.space() {}
    let padding = cursor.column - marker_end;
    let blank = matches!(
        source.as_bytes().get(cursor.offset),
        None | Some(b'\r' | b'\n')
    );
    marker_end - start_column + if blank || padding > 4 { 1 } else { padding }
}

pub(crate) fn restore(root: &mut Node, source: &str) {
    walk(root, source, &mut Vec::new());
}

fn walk(node: &mut Node, source: &str, containers: &mut Vec<Container>) {
    let container = match node {
        Node::Blockquote(_) => Some(Container::Quote),
        Node::FootnoteDefinition(_) => Some(Container::Indent(4)),
        Node::ListItem(item) => item
            .position
            .as_ref()
            .map(|p| Container::Indent(list_indent(source, p.start.offset))),
        _ => None,
    };
    if let Some(container) = container {
        containers.push(container);
    }
    match node {
        Node::MdxFlowExpression(expression) => recover(
            &mut expression.value,
            &mut expression.stops,
            source,
            containers,
        ),
        Node::MdxTextExpression(expression) => recover(
            &mut expression.value,
            &mut expression.stops,
            source,
            containers,
        ),
        Node::MdxJsxFlowElement(element) => attributes(&mut element.attributes, source, containers),
        Node::MdxJsxTextElement(element) => attributes(&mut element.attributes, source, containers),
        _ => {}
    }
    if let Some(children) = node.children_mut() {
        for child in children {
            walk(child, source, containers);
        }
    }
    if container.is_some() {
        containers.pop();
    }
}

fn attributes(attrs: &mut [AttributeContent], source: &str, containers: &[Container]) {
    for attr in attrs {
        match attr {
            AttributeContent::Expression(expression) => recover(
                &mut expression.value,
                &mut expression.stops,
                source,
                containers,
            ),
            AttributeContent::Property(property) => {
                if let Some(AttributeValue::Expression(expression)) = &mut property.value {
                    recover(
                        &mut expression.value,
                        &mut expression.stops,
                        source,
                        containers,
                    );
                }
            }
        }
    }
}

fn recover(
    value: &mut String,
    stops: &mut Vec<(usize, usize)>,
    source: &str,
    containers: &[Container],
) {
    if !value.contains(['\r', '\n']) {
        return;
    }
    let (Some(&(_, start)), Some(&(last_index, last_offset))) = (stops.first(), stops.last())
    else {
        return;
    };
    let Some(last) = value.get(last_index..) else {
        return;
    };
    // A partially consumed tab is serialized as virtual spaces by markdown-rs.
    // Locate the final retained run after that prefix using its source stop.
    let last_tail = last.trim_start_matches([' ', '\t']);
    let Some(source_tail) = source.get(last_offset..) else {
        return;
    };
    let raw_tail = source_tail.trim_start_matches([' ', '\t']);
    if !raw_tail.starts_with(last_tail) {
        return;
    }
    let mut end = last_offset + source_tail.len() - raw_tail.len() + last_tail.len();
    // The final retained run may be a line ending: indentation before the
    // closing brace has no data token (and therefore no additional stop).
    if last.ends_with(['\r', '\n']) {
        let mut cursor = Cursor::new(source, end);
        for &container in containers {
            cursor.container(container);
        }
        while cursor.space() {}
        if source.as_bytes().get(cursor.offset) == Some(&b'}') {
            end = cursor.offset;
        }
    }
    let Some(raw) = source.get(start..end) else {
        return;
    };
    let mut restored = String::with_capacity(raw.len());
    let mut restored_stops = Vec::new();
    let mut offset = start;
    let mut first = true;
    while offset < end {
        let mut cursor = Cursor::new(source, offset);
        if !first {
            for &container in containers {
                cursor.container(container);
            }
        }
        first = false;
        if cursor.offset > end {
            return;
        }
        restored_stops.push((
            restored.len(),
            cursor
                .offset
                .saturating_sub(usize::from(cursor.virtual_spaces > 0)),
        ));
        restored.extend(std::iter::repeat_n(' ', cursor.virtual_spaces));
        if cursor.virtual_spaces > 0 {
            restored_stops.push((restored.len(), cursor.offset));
        }
        offset = cursor.offset;
        let line_end = source.as_bytes()[offset..end]
            .iter()
            .position(|b| matches!(b, b'\r' | b'\n'))
            .map_or(end, |i| offset + i + 1);
        let line_end = if source.as_bytes().get(line_end - 1) == Some(&b'\r')
            && source.as_bytes().get(line_end) == Some(&b'\n')
            && line_end < end
        {
            line_end + 1
        } else {
            line_end
        };
        restored.push_str(&source[offset..line_end]);
        offset = line_end;
    }
    *value = restored;
    *stops = restored_stops;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expressions(node: &Node, result: &mut Vec<(String, Vec<(usize, usize)>)>) {
        match node {
            Node::MdxFlowExpression(e) => result.push((e.value.clone(), e.stops.clone())),
            Node::MdxTextExpression(e) => result.push((e.value.clone(), e.stops.clone())),
            Node::MdxJsxFlowElement(e) => collect_attrs(&e.attributes, result),
            Node::MdxJsxTextElement(e) => collect_attrs(&e.attributes, result),
            _ => {}
        }
        if let Some(children) = node.children() {
            for child in children {
                expressions(child, result);
            }
        }
    }

    fn collect_attrs(attrs: &[AttributeContent], result: &mut Vec<(String, Vec<(usize, usize)>)>) {
        for attr in attrs {
            match attr {
                AttributeContent::Expression(e) => result.push((e.value.clone(), e.stops.clone())),
                AttributeContent::Property(p) => {
                    if let Some(AttributeValue::Expression(e)) = &p.value {
                        result.push((e.value.clone(), e.stops.clone()));
                    }
                }
            }
        }
    }

    fn recovered(source: &str) -> Vec<(String, Vec<(usize, usize)>)> {
        let mut options = markdown::ParseOptions::mdx();
        options.constructs.gfm_footnote_definition = true;
        let mut tree = markdown::to_mdast(source, &options).unwrap();
        restore(&mut tree, source);
        let mut result = Vec::new();
        expressions(&tree, &mut result);
        result
    }

    #[test]
    fn preserves_authored_bytes_in_containers_and_expression_forms() {
        let expression = "`雪\n  alpha\n\tbeta ${1}\n  `";
        for (head, tail, expected) in [
            ("<P x={", "} />", expression.to_string()),
            ("<P {...{x:", "}} />", format!("...{{x:{expression}}}")),
            ("{<P x={", "} />}", format!("<P x={{{expression}}} />")),
            (
                "text {<P x={",
                "} />} end",
                format!("<P x={{{expression}}} />"),
            ),
            ("<O>\n\n<P x={", "} />\n\n</O>", expression.to_string()),
        ] {
            for (first, continuation) in [
                ("", ""),
                ("- ", "  "),
                ("  - ", "    "),
                ("123. ", "     "),
                ("-   ", "    "),
                ("> ", "> "),
                ("> - ", ">   "),
                ("- > ", "  > "),
                ("- - ", "    "),
                ("[^a]: ", "    "),
                ("> [^a]: ", ">     "),
                ("- [^a]: ", "      "),
                ("-\t", "\t"),
                ("-\t- ", "\t  "),
            ] {
                let source = format!(
                    "{first}{}",
                    format!("{head}{expression}{tail}").replace('\n', &format!("\n{continuation}"))
                );
                assert_eq!(recovered(&source)[0].0, expected, "{source:?}");
            }
        }
    }

    #[test]
    fn preserves_blank_lines_and_closing_indentation() {
        for expression in [
            "`a\n  \n\t\n  z`",
            "\n  `x`\n  ",
            "`a\r\n  b\r\n  `",
            "`a\r  b\r  `",
        ] {
            let source = format!("<P x={{{expression}}} />");
            assert_eq!(recovered(&source)[0].0, expression);
        }
    }

    #[test]
    fn partial_tab_stops_map_retained_source_bytes() {
        let source = "- <P x={`a\n\tb\n\t`} />";
        let result = recovered(source);
        assert_eq!(result[0].0, "`a\n  b\n  `");
        for needle in ['b', '`'] {
            let relative = result[0].0.rfind(needle).unwrap();
            let &(index, offset) = result[0]
                .1
                .iter()
                .rev()
                .find(|(index, _)| *index <= relative)
                .unwrap();
            assert_eq!(offset + relative - index, source.rfind(needle).unwrap());
        }
    }

    #[test]
    fn nested_tab_list_prefix_is_entirely_structural() {
        assert_eq!(
            recovered("-\t- <P x={`a\n\t  b\n\t  `} />")[0].0,
            "`a\nb\n`"
        );
    }
}
