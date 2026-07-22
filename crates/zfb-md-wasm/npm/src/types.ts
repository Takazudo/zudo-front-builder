// Mirrors the JSON contracts of the `zfb-md-wasm` Rust crate (zfb#1576)
// literally -- see that crate's `src/lib.rs` rustdoc for the authoritative
// shape. Keep this file in lock-step with the crate when the crate's
// options/result shapes change.

/** `zfb_content::facade::PipelineOptions`'s `gfm` sub-object, verbatim. */
export interface GfmOptions {
  strikethrough?: boolean;
  table?: boolean;
  autolinkLiteral?: boolean;
  taskListItem?: boolean;
  footnoteDefinition?: boolean;
}

/** Base syntax selected by `parseToAst`. */
export type ParseDialect = "markdown" | "mdx";

/** Raw-parser pipeline options. Visitor/serializer options are not accepted. */
export interface ParsePipelineOptions {
  gfm?: GfmOptions;
}

/**
 * The distinct, closed options document consumed by `parseToAst`.
 *
 * `filename` must end in lowercase `.md` or `.mdx`. With no explicit
 * `dialect`, `.md` selects CommonMark and `.mdx` selects MDX; omitting the
 * filename uses `<anonymous>.mdx`, hence MDX. An explicit dialect overrides
 * either valid extension but does not waive that extension gate.
 */
export interface ParseToAstOptions {
  filename?: string;
  dialect?: ParseDialect;
  pipeline?: ParsePipelineOptions;
}

/**
 * `MarkdownFeaturesConfig` (see `crates/zfb-md-ast/src/features_config.rs`).
 * Left as an open record here -- the wasm boundary passes it through to the
 * Rust `deny_unknown_fields` deserializer verbatim, which is the
 * authoritative validator for its keys.
 */
export type MarkdownFeaturesConfig = Record<string, unknown>;

/**
 * `zfb_content::facade::CodeHighlightOptions`'s `mode` field -- mirrors the
 * `"inline"` / `"class"` string-literal union in native `zfb.config.ts`
 * (`CodeHighlightConfig.mode`).
 */
export type CodeHighlightMode = "inline" | "class";

/**
 * `zfb_content::facade::CodeHighlightOptions`, verbatim (Highlight Tokens
 * epic zfb#1528, wasm routing sub zfb#1852).
 */
export interface CodeHighlightOptions {
  /** Output mode for fenced-code highlighting. Defaults to `"inline"` --
   * reproduces the pre-existing per-token inline-color behaviour
   * byte-for-byte. */
  mode?: CodeHighlightMode;
  /** Class-name prefix for class-mode role classes (e.g. the default
   * `"hi-"` yields `hi-kw`, `hi-str`, ...). Only meaningful when `mode` is
   * `"class"`. */
  classPrefix?: string;
  /** Per-role class overrides for class mode. Absent or `null` uses
   * `{classPrefix}{role}` for every role. Only meaningful when `mode` is
   * `"class"`. */
  roleClasses?: Partial<Record<HighlightRole, string>> | null;
}

/** `zfb_content::facade::PipelineOptions`, verbatim. */
export interface PipelineOptions {
  /**
   * A syntect theme name. Absent, or explicit `null`, keeps the built-in
   * default theme (`base16-ocean.dark`) -- fenced code is ALWAYS
   * highlighted through this field; there is no "no syntax highlighting"
   * value (this comment previously claimed `null` meant no highlighting,
   * which was never true of the Rust deserializer -- corrected in
   * zfb#1852). Mutually exclusive with `codeHighlight.mode: "class"`.
   */
  theme?: string | null;
  gfm?: GfmOptions;
  cjkFriendly?: boolean;
  hardBreaks?: boolean;
  /**
   * Output mode + class-mode knobs for fenced-code highlighting
   * (Highlight Tokens epic zfb#1528). Absent, `null`, or
   * `{ mode: "inline" }` reproduce the pre-existing inline-color
   * behaviour byte-for-byte. `{ mode: "class" }` is mutually exclusive
   * with a top-level `theme`.
   */
  codeHighlight?: CodeHighlightOptions | null;
  features?: MarkdownFeaturesConfig;
}

/** `jsxRuntime` option values. Consumed only by `compile`. */
export type JsxRuntime = "preact" | "react";

/**
 * The options JSON document shared by `compile` and `renderHtml`. Every
 * field is optional; `{}` selects all defaults. Unknown fields are
 * rejected by the Rust side at both nesting levels (`deny_unknown_fields`).
 */
export interface ZfbMdWasmOptions {
  /**
   * Must end in `.md` or `.mdx`. Drives frontmatter dispatch and
   * diagnostics display. Defaults to `<anonymous>.mdx` for `compile` and
   * `<anonymous>.md` for `renderHtml`.
   */
  filename?: string;
  /** Consumed only by `compile`; `renderHtml` accepts and ignores it. */
  jsxRuntime?: JsxRuntime;
  /** Consumed only by `compile`; `renderHtml` accepts and ignores it. */
  development?: boolean;
  pipeline?: PipelineOptions;
}

/** `source` values a `Diagnostic` can carry. */
export type DiagnosticSource = "options" | "frontmatter" | "markdown" | "compile";

/**
 * One diagnostic entry. `line`/`column` are 1-based. For `"markdown"` /
 * `"frontmatter"` they point into the *original source* (frontmatter lines
 * included). For `"options"` they point into the *options JSON document*.
 * `null` when the underlying error carries no location.
 */
export interface Diagnostic {
  severity: "error";
  source: DiagnosticSource;
  message: string;
  line: number | null;
  column: number | null;
}

/** Result document of `compile`. */
export interface CompileResult {
  /** ES-module JS source on success, `null` on failure. */
  code: string | null;
  /** Parsed YAML frontmatter as JSON, `null` when absent or unextractable. */
  frontmatter: unknown;
  diagnostics: Diagnostic[];
}

/**
 * One point in the original source (unist convention). `line` is 1-based
 * and unit-agnostic. `column` is 1-based and `offset` is 0-based, BOTH in
 * **UTF-16 code units** (zfb#1856, epic zfb#1854) — the indexing
 * `String.prototype.slice`, `mdast-util-to-hast`, and consumer mdast
 * plugins already use, and remark/unist's own convention. This is NOT
 * Unicode scalar values/code points: a scalar value outside the Basic
 * Multilingual Plane (most emoji) is 2 UTF-16 code units but 1 code point,
 * so a surrogate-pair character advances `offset`/`column` by 2, not 1.
 * Pure-ASCII sources have byte offsets == UTF-16 offsets, so this only
 * matters once the source has non-ASCII content.
 */
export interface AstPoint {
  line: number;
  column: number;
  offset: number;
}

/** Start/end span of two {@link AstPoint}s (unist convention). */
export interface AstPosition {
  start: AstPoint;
  end: AstPoint;
}

/**
 * Internal markdown-rs re-parse bookkeeping carried by MDX expression/ESM
 * nodes: `[indexInValue, absoluteSourceOffset]`. Internal, unstable, and
 * (unlike {@link AstPosition}) **UTF-8 BYTE**-based -- markdown-rs uses it
 * to re-resolve sub-positions inside an expression's own re-parse pass.
 * Never slice a string with it; it does not share `position`'s UTF-16 unit
 * contract.
 */
export type MarkdownRsStop = readonly [indexInValue: number, absoluteSourceOffsetBytes: number];

/** GFM alignment for a table column. `null` means no explicit alignment. */
export type TableAlign = "left" | "right" | "center" | null;

/** Explicitness of a `link`/`image` reference (GFM footnotes reuse it too). */
export type ReferenceKind = "shortcut" | "collapsed" | "full";

// ── mdast core node set ─────────────────────────────────────────────────────
// Mirrors `markdown::mdast::Node`'s variants (markdown-rs, `serde(tag =
// "type", rename_all = "camelCase")`) -- see that crate's `src/mdast.rs` for
// the authoritative field shapes. Every variant here always carries
// `position` (requirement 2 of zfb#1828); the one documented exception,
// `mdxJsxAttribute`/its value/expression-attribute siblings, is intentionally
// NOT part of the `MdastNode` union below -- they are attribute records
// embedded in `MdxJsxFlowElement`/`MdxJsxTextElement.attributes`, not
// standalone tree nodes.

export interface Root {
  type: "root";
  position: AstPosition;
  children: MdastNode[];
}
export interface Paragraph {
  type: "paragraph";
  position: AstPosition;
  children: MdastNode[];
}
export interface Heading {
  type: "heading";
  position: AstPosition;
  depth: 1 | 2 | 3 | 4 | 5 | 6;
  children: MdastNode[];
}
export interface ThematicBreak {
  type: "thematicBreak";
  position: AstPosition;
}
export interface Blockquote {
  type: "blockquote";
  position: AstPosition;
  children: MdastNode[];
}
export interface List {
  type: "list";
  position: AstPosition;
  ordered: boolean;
  /**
   * Absent for an unordered list. Rust serializes `None` here by OMITTING
   * the key (`#[serde(skip_serializing_if = "Option::is_none")]`), not as
   * JSON `null` -- hence `?:`, not `| null`.
   */
  start?: number;
  spread: boolean;
  children: MdastNode[];
}
export interface ListItem {
  type: "listItem";
  position: AstPosition;
  spread: boolean;
  /** GFM task-list state; absent (not `null` -- see {@link List.start}) for a plain (non-task) item. */
  checked?: boolean;
  children: MdastNode[];
}
export interface Html {
  type: "html";
  position: AstPosition;
  value: string;
}
export interface Code {
  type: "code";
  position: AstPosition;
  /** Absent when the fence has no language (see {@link List.start}). */
  lang?: string;
  /** Absent when the fence has no meta string (see {@link List.start}). */
  meta?: string;
  value: string;
}
export interface Definition {
  type: "definition";
  position: AstPosition;
  url: string;
  /** Absent when there is no title (see {@link List.start}). */
  title?: string;
  identifier: string;
  /** Absent when there is no label (see {@link List.start}). */
  label?: string;
}
export interface Text {
  type: "text";
  position: AstPosition;
  value: string;
}
export interface Emphasis {
  type: "emphasis";
  position: AstPosition;
  children: MdastNode[];
}
export interface Strong {
  type: "strong";
  position: AstPosition;
  children: MdastNode[];
}
export interface InlineCode {
  type: "inlineCode";
  position: AstPosition;
  value: string;
}
export interface Break {
  type: "break";
  position: AstPosition;
}
export interface Link {
  type: "link";
  position: AstPosition;
  url: string;
  /** Absent when there is no title (see {@link List.start}). */
  title?: string;
  children: MdastNode[];
}
export interface Image {
  type: "image";
  position: AstPosition;
  alt: string;
  url: string;
  /** Absent when there is no title (see {@link List.start}). */
  title?: string;
}
export interface LinkReference {
  type: "linkReference";
  position: AstPosition;
  referenceType: ReferenceKind;
  identifier: string;
  /** Absent when there is no label (see {@link List.start}). */
  label?: string;
  children: MdastNode[];
}
export interface ImageReference {
  type: "imageReference";
  position: AstPosition;
  alt: string;
  referenceType: ReferenceKind;
  identifier: string;
  /** Absent when there is no label (see {@link List.start}). */
  label?: string;
}
/** GFM footnote definition (`[^id]: ...`). */
export interface FootnoteDefinition {
  type: "footnoteDefinition";
  position: AstPosition;
  identifier: string;
  /** Absent when there is no label (see {@link List.start}). */
  label?: string;
  children: MdastNode[];
}
/** GFM footnote reference (`[^id]`). */
export interface FootnoteReference {
  type: "footnoteReference";
  position: AstPosition;
  identifier: string;
  /** Absent when there is no label (see {@link List.start}). */
  label?: string;
}
/** GFM table. */
export interface Table {
  type: "table";
  position: AstPosition;
  align: TableAlign[];
  children: MdastNode[];
}
export interface TableRow {
  type: "tableRow";
  position: AstPosition;
  children: MdastNode[];
}
export interface TableCell {
  type: "tableCell";
  position: AstPosition;
  children: MdastNode[];
}
/** GFM strikethrough (`~~text~~`). */
export interface Delete {
  type: "delete";
  position: AstPosition;
  children: MdastNode[];
}
/**
 * Frontmatter block, IF markdown-rs's own frontmatter construct were ever
 * enabled on this raw-parse tier. In practice this never appears through
 * `parseToAst`: `zfb-md-wasm` strips frontmatter and returns it separately
 * via `ParseToAstResult.frontmatter` before this tree is even parsed.
 * Documented for completeness (it is part of the `Node` enum this export
 * serializes from) and so the catch-all member is never needed for it.
 */
export interface Yaml {
  type: "yaml";
  position: AstPosition;
  value: string;
}

// ── MDX node set ─────────────────────────────────────────────────────────

/** MDX flow expression (`{...}` on its own line). */
export interface MdxFlowExpression {
  type: "mdxFlowExpression";
  position: AstPosition;
  value: string;
  /** Internal/unstable/byte-based -- see {@link MarkdownRsStop}. */
  _markdownRsStops: MarkdownRsStop[];
}
/** MDX text expression (`{...}` inline in phrasing content). */
export interface MdxTextExpression {
  type: "mdxTextExpression";
  position: AstPosition;
  value: string;
  _markdownRsStops: MarkdownRsStop[];
}
/** MDX JSX element as flow (block-level) content. */
export interface MdxJsxFlowElement {
  type: "mdxJsxFlowElement";
  position: AstPosition;
  /** Absent for a JSX fragment (`<>...</>`) -- see {@link List.start}. */
  name?: string;
  attributes: MdxJsxAttributeContent[];
  children: MdastNode[];
}
/** MDX JSX element as phrasing (inline) content. */
export interface MdxJsxTextElement {
  type: "mdxJsxTextElement";
  position: AstPosition;
  /** Absent for a JSX fragment (`<>...</>`) -- see {@link List.start}. */
  name?: string;
  attributes: MdxJsxAttributeContent[];
  children: MdastNode[];
}

/**
 * A JSX attribute's value: a plain string literal, or a `{...}` expression
 * value. Divergence: expression attribute VALUES carry no `position`
 * (mirrors the `mdxJsxAttribute`/`mdxJsxExpressionAttribute` divergence
 * below -- markdown-rs does not model attribute positions anywhere).
 */
export interface MdxJsxAttributeValueExpression {
  type: "mdxJsxAttributeValueExpression";
  value: string;
  _markdownRsStops: MarkdownRsStop[];
}
/**
 * One `name="value"` / `name={expr}` / bare `name` JSX attribute.
 * Divergence: carries no `position` -- markdown-rs does not model attribute
 * positions (zfb#1828 requirement 2's one documented exception).
 */
export interface MdxJsxAttribute {
  type: "mdxJsxAttribute";
  name: string;
  /** Absent for a bare boolean attribute (`<a b />`) -- see {@link List.start}. */
  value?: string | MdxJsxAttributeValueExpression;
}
/**
 * A JSX spread attribute (`{...expr}`). Divergence: carries no `position`,
 * same reason as {@link MdxJsxAttribute}.
 */
export interface MdxJsxExpressionAttribute {
  type: "mdxJsxExpressionAttribute";
  value: string;
  _markdownRsStops: MarkdownRsStop[];
}
/** One entry of `MdxJsxFlowElement`/`MdxJsxTextElement.attributes`. */
export type MdxJsxAttributeContent = MdxJsxAttribute | MdxJsxExpressionAttribute;

/**
 * Catch-all for any mdast node type not enumerated above -- keeps
 * unrecognized/future node types TYPED (requirement 3 of zfb#1828: an opaque
 * `unknown` would silently drop this contract) rather than falling back to
 * `unknown`. `type` is deliberately the general `string` here (not a
 * literal), so TypeScript's discriminated-union narrowing on `type` keeps
 * this member alongside any single literal match; disambiguate with an
 * explicit assertion once a runtime `type` check has confirmed which shape
 * you actually have (see `test/consumer-compatibility-parse-to-ast.ts` for
 * the pattern).
 */
export interface UnknownMdastNode {
  type: string;
  position: AstPosition;
  [key: string]: unknown;
}

/**
 * Any node markdown-rs's raw mdast serialization can produce through
 * `parseToAst` -- the documented mdast core set, the documented MDX set, or
 * {@link UnknownMdastNode} for anything else. Attribute-record shapes
 * ({@link MdxJsxAttributeContent} and its members) are intentionally NOT
 * part of this union -- see their own docs.
 */
export type MdastNode =
  | Root
  | Paragraph
  | Heading
  | ThematicBreak
  | Blockquote
  | List
  | ListItem
  | Html
  | Code
  | Definition
  | Text
  | Emphasis
  | Strong
  | InlineCode
  | Break
  | Link
  | Image
  | LinkReference
  | ImageReference
  | FootnoteDefinition
  | FootnoteReference
  | Table
  | TableRow
  | TableCell
  | Delete
  | Yaml
  | MdxFlowExpression
  | MdxTextExpression
  | MdxJsxFlowElement
  | MdxJsxTextElement
  | UnknownMdastNode;

/** The tree root `parseToAst` returns -- always a `root` node when present. */
export type MdastRoot = Root;

/**
 * Result document of `parseToAst` (zfb#1857, epic zfb#1854). `ast` is the
 * RAW markdown-rs mdast tree (unist-shaped: `type` tag, camelCase fields,
 * `position` on every real tree node -- see {@link MdastNode}), post-
 * frontmatter-strip, PRE-zfb-visitors: no zfb-synthesized node types, and
 * MDX JSX elements / directive-style text survive exactly as markdown-rs
 * parsed them. `position.offset`/`position.column` are UTF-16 code units
 * (see {@link AstPoint}); `position.line` is unit-agnostic.
 *
 * Markdown mode preserves CommonMark HTML/comments, angle autolinks,
 * indented code, and literal braces. MDX mode enables JSX/expressions and
 * keeps markdown-rs's conflicting CommonMark constructs disabled. Both
 * modes apply the same five independent {@link GfmOptions} switches.
 *
 * Documented divergences from remark-parse / remark-mdx in MDX mode:
 * - `mdxJsxAttribute` (and its value/expression-attribute siblings) carry no
 *   `position` -- markdown-rs does not model attribute positions.
 * - Top-level `import`/`export` degrade to paragraphs (no `mdxjsEsm` nodes),
 *   and MDX expressions carry no estree data -- the wasm boundary cannot
 *   host a JS ESM/acorn parser. Consumers needing remark-mdx-equivalent
 *   ESM/estree keep remark for those documents.
 * - `_markdownRsStops` is internal, unstable, and byte-based (not UTF-16) --
 *   see {@link MarkdownRsStop}.
 */
export interface ParseToAstResult {
  /** Serialized raw mdast root on success, `null` on failure. */
  ast: MdastRoot | null;
  /** Parsed YAML frontmatter as JSON, `null` when absent or unextractable. */
  frontmatter: unknown;
  diagnostics: Diagnostic[];
}

/** Result document of `renderHtml`. */
export interface RenderHtmlResult {
  /** HTML fragment on success, `null` on failure. */
  html: string | null;
  frontmatter: unknown;
  diagnostics: Diagnostic[];
}

/**
 * The fixed 18-role semantic taxonomy emitted by `highlightCode`. This union
 * is mechanically checked against Rust's canonical `HiRole::FULL_NAMES` by
 * `zfb`'s role-drift guard; do not add a second unguarded role list.
 */
export type HighlightRole =
  | "escape"
  | "operator"
  | "comment"
  | "string"
  | "number"
  | "constant"
  | "keyword"
  | "function"
  | "type"
  | "namespace"
  | "property"
  | "variable"
  | "tag"
  | "attribute"
  | "punctuation"
  | "inserted"
  | "deleted"
  | "heading";

/** Options for direct arbitrary-code semantic class highlighting. */
export interface HighlightCodeOptions {
  /** Required syntax token, for example `"html"`, `"css"`, or `"javascript"`. */
  language: string;
  /** The only supported direct output mode. Defaults to `"class"`. */
  mode?: "class";
  /** Semantic role class prefix. Defaults to `"hi-"`. */
  classPrefix?: string;
  /** Full-name role overrides, for example `{ keyword: "text-violet-600" }`. */
  roleClasses?: Partial<Record<HighlightRole, string>>;
}

/** Sources used by direct `highlightCode` diagnostics. */
export type HighlightDiagnosticSource = "options" | "highlight" | "internal";

/** A structured direct-highlighting diagnostic. */
export interface HighlightDiagnostic {
  severity: "error" | "warning";
  source: HighlightDiagnosticSource;
  message: string;
  /** Always `null` for semantic/highlight diagnostics; JSON option parse locations are 1-based. */
  line: number | null;
  /** Always `null` for semantic/highlight diagnostics; JSON option parse locations are 1-based. */
  column: number | null;
}

/** Result document returned by {@link highlightCode}. */
export interface HighlightCodeResult {
  /** Complete semantic `<pre><code>` wrapper, or `null` for invalid options/internal errors. */
  html: string | null;
  diagnostics: HighlightDiagnostic[];
}
