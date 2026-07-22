# Directive reference corpus

`oracle.json` is generated from the source strings in `cases.json` with the
locked reference stacks:

- `unified@11.0.5`
- `remark-parse@11.0.0`
- `remark-mdx@3.1.1` for MDX cases
- `remark-directive@4.0.0`
- resolved `micromark-extension-directive@4.0.0`
- resolved `mdast-util-directive@3.1.0`

Markdown cases use
`unified().use(remarkParse).use(remarkDirective).parse(source)`. MDX cases add
`remarkMdx` before `remarkDirective`. The committed oracle converts JavaScript
UTF-16 point offsets/columns to UTF-8 byte offsets/columns before comparison,
because the Rust core deliberately keeps markdown-rs-native byte coordinates;
the Wasm integration owns the later UTF-16 conversion.

The Rust comparison removes only established base-parser representation
differences unrelated to directives: absent optional null fields, MDX JSX
attribute positions, and outer base-container positions. Directive nodes,
their complete descendant trees, label data, attributes, recovery, ordering,
and every descendant byte span compare exactly.
